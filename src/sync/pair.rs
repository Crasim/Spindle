//! Adding a device: the existing device shows a QR code (a `spindle://pair` link) or a short
//! code with an address; the new device connects through the sync server's pairing
//! rendezvous or straight over the LAN.
//!
//! Both sides run SPAKE2 with the code (or the QR's secret), so the code can't be guessed
//! offline and neither the server nor anyone on the network learns the keys. The new device
//! then shows a number from 1 to 100, derived from the agreed key and never sent, and the
//! person types it on the existing device; only then does it send the space's keys,
//! encrypted with the agreed key. Someone who guessed the code still needs to see the new
//! device's screen. A pairing expires after ten minutes, is used once, and stops after five
//! wrong codes or three wrong numbers.

use super::core::{Member, SpaceKeys, b64, hex, hkdf32, unb64};
use anyhow::{Context, Result, anyhow, bail, ensure};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const MAX_HANDSHAKE_FRAME: usize = 1 << 20;
const MAX_WELCOME_FRAME: usize = 64 << 20;

const TRIES: usize = 5;
/// Wrong numbers allowed on the existing device before the pairing stops.
const NUMBER_TRIES: usize = 3;
const EXPIRES: Duration = Duration::from_secs(600);
const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn code(n: usize) -> Result<String> {
    let bytes = crate::vault::random::<16>()?;
    Ok(bytes[..n]
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect())
}

/// Where the new device should connect.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Invite {
    /// Rendezvous ID on the server.
    pub id: String,
    /// The SPAKE2 password: the long QR secret, or the short typed code.
    pub secret: String,
    /// Sync server for the rendezvous, if the space has one.
    pub relay: Option<String>,
    /// This device's addresses for a direct connection (home network, Tailscale, …),
    /// comma-separated, tried in order.
    pub addr: Option<String>,
}

impl Invite {
    /// The `spindle://pair` link the QR code carries.
    pub fn link(&self) -> String {
        let mut s = format!("spindle://pair?v=1&id={}&s={}", self.id, self.secret);
        if let Some(r) = &self.relay {
            s.push_str(&format!("&relay={}", encode(r)));
        }
        if let Some(a) = &self.addr {
            s.push_str(&format!("&addr={a}"));
        }
        s
    }

    /// The short code to type: `ID-CODE` (with the server or address given separately).
    pub fn typed(&self) -> String {
        let s = &self.secret;
        if s.len() == 8 {
            format!("{}-{}-{}", self.id.to_uppercase(), &s[..4], &s[4..])
        } else {
            format!("{}-{s}", self.id.to_uppercase())
        }
    }

    /// Read a link, or a typed code with the server URL or LAN address.
    pub fn parse(input: &str, place: Option<&str>) -> Result<Self> {
        let input = input.trim();
        if let Some(q) = input.strip_prefix("spindle://pair?") {
            let mut inv = Invite::default();
            for kv in q.split('&') {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                match k {
                    "id" => inv.id = v.into(),
                    "s" => inv.secret = v.into(),
                    "relay" => inv.relay = Some(decode(v)),
                    "addr" => inv.addr = Some(v.into()),
                    _ => {}
                }
            }
            ensure!(
                !inv.id.is_empty() && !inv.secret.is_empty(),
                "That pairing link is incomplete"
            );
            return Ok(inv);
        }
        let clean: String = input
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_uppercase();
        let (id, secret) = clean
            .split_once('-')
            .context("Type the code as shown, like ABCD-EFGH-JKLM")?;
        let secret = secret.replace('-', "");
        ensure!(
            id.len() == 4 && secret.len() == 8,
            "Type the code as shown, like ABCD-EFGH-JKLM"
        );
        let place =
            place.context("Also give the sync server's address, or the other device's address")?;
        let (relay, addr) = if place.starts_with("http://") || place.starts_with("https://") {
            (Some(place.trim_end_matches('/').to_owned()), None)
        } else {
            (None, Some(place.to_owned()))
        };
        Ok(Invite {
            id: id.to_lowercase(),
            secret,
            relay,
            addr,
        })
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A framed channel to the other device: the server's rendezvous or a direct TCP connection.
enum Channel {
    Ws(
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ),
    Tcp(BufReader<tokio::net::TcpStream>),
}

impl Channel {
    async fn send(&mut self, v: &Value) -> Result<()> {
        let text = v.to_string();
        match self {
            Channel::Ws(ws) => ws
                .send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
                .await
                .map_err(|e| anyhow!("{e}")),
            Channel::Tcp(s) => {
                s.get_mut()
                    .write_all(format!("{text}\n").as_bytes())
                    .await?;
                Ok(())
            }
        }
    }
    async fn recv(&mut self) -> Result<Value> {
        self.recv_with_limit(MAX_HANDSHAKE_FRAME).await
    }

    async fn recv_with_limit(&mut self, max: usize) -> Result<Value> {
        let text = match self {
            Channel::Ws(ws) => loop {
                match ws.next().await {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                        break t.to_string();
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => bail!("{e}"),
                    None => bail!("The other device left"),
                }
            },
            Channel::Tcp(s) => {
                let mut line = String::new();
                let n = s.take((max + 1) as u64).read_line(&mut line).await?;
                ensure!(n <= max, "The pairing message exceeds the {} MiB limit", max >> 20);
                ensure!(n > 0 && line.ends_with('\n'), "The other device left");
                line
            }
        };
        ensure!(text.len() <= max, "The pairing message exceeds the {} MiB limit", max >> 20);
        Ok(serde_json::from_str(&text)?)
    }
}

async fn rendezvous(relay: &str, id: &str) -> Result<Channel> {
    let url = format!("{}/v1/pair/{id}", relay.trim_end_matches('/'))
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let (ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .context("Couldn't reach the sync server for pairing")?;
    Ok(Channel::Ws(ws))
}

/// SPAKE2, then a check that both sides got the same key. Returns the key.
async fn agree(ch: &mut Channel, invite: &Invite, host: bool) -> Result<[u8; 32]> {
    use spake2::{Ed25519Group, Identity, Password, Spake2};
    let pw = Password::new(invite.secret.as_bytes());
    let (a, b) = (
        Identity::new(b"spindle-host"),
        Identity::new(b"spindle-joiner"),
    );
    let (state, msg) = if host {
        Spake2::<Ed25519Group>::start_a(&pw, &a, &b)
    } else {
        Spake2::<Ed25519Group>::start_b(&pw, &a, &b)
    };
    // The new device speaks first: through the server's rendezvous, the existing device may
    // be connected long before anyone else is there to hear it.
    let theirs = if host {
        let t = ch.recv().await?;
        ch.send(&json!({"t": "spake", "msg": b64(&msg)})).await?;
        t
    } else {
        ch.send(&json!({"t": "spake", "msg": b64(&msg)})).await?;
        ch.recv().await?
    };
    ensure!(theirs["t"] == "spake", "Unexpected pairing message");
    let shared = state
        .finish(&unb64(theirs["msg"].as_str().unwrap_or(""))?)
        .map_err(|_| anyhow!("Pairing failed"))?;
    let key = hkdf32(&shared, invite.id.as_bytes(), b"spindle-pair");
    let mac = |who: &str| hex(&hkdf32(&key, who.as_bytes(), b"spindle-pair-confirm")[..16]);
    let (mine, other) = if host {
        ("host", "joiner")
    } else {
        ("joiner", "host")
    };
    let got = if host {
        let g = ch.recv().await?;
        ch.send(&json!({"t": "confirm", "mac": mac(mine)})).await?;
        g
    } else {
        ch.send(&json!({"t": "confirm", "mac": mac(mine)})).await?;
        ch.recv().await?
    };
    ensure!(
        got["t"] == "confirm" && got["mac"] == mac(other).as_str(),
        "WRONG_CODE"
    );
    Ok(key)
}

/// The number from 1 to 100 the new device shows and the person types on the existing one.
/// Both sides work it out from the agreed key; it never crosses the network.
pub fn number(key: &[u8; 32]) -> u32 {
    let d = hkdf32(key, b"number", b"spindle-pair-number");
    u32::from_be_bytes([d[0], d[1], d[2], d[3]]) % 100 + 1
}

fn seal(key: &[u8; 32], v: &Value) -> Result<String> {
    Ok(b64(&crate::vault::seal(
        key,
        b"spindle-pair",
        serde_json::to_vec(v)?,
    )?))
}

fn open(key: &[u8; 32], s: &str) -> Result<Value> {
    Ok(serde_json::from_slice(&crate::vault::open(
        key,
        b"spindle-pair",
        &unb64(s)?,
    )?)?)
}

/// What the person sees on the existing device while pairing.
#[derive(Clone, Debug, Default)]
pub struct HostView {
    #[cfg_attr(not(test), allow(dead_code))]
    pub invite: Invite,
    /// The QR link and the typed code (with where to connect).
    pub link: String,
    pub typed: String,
    /// The new device is connected and showing its number; type it here.
    pub asking: bool,
    pub joiner: Option<String>,
    pub status: String,
    pub done: Option<Result<Member, String>>,
}

pub struct Host {
    pub view: Arc<Mutex<HostView>>,
    answers: tokio::sync::mpsc::UnboundedSender<Option<u32>>,
    task: tokio::task::JoinHandle<()>,
}

impl Host {
    /// The number the person typed from the new device's screen, or `None` to refuse.
    pub fn answer(&self, number: Option<u32>) {
        let _ = self.answers.send(number);
    }
    pub fn cancel(self) {
        self.task.abort();
    }
}

/// This machine's address on the LAN (no packets are sent).
pub(crate) fn lan_ip() -> Option<std::net::IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

/// Connect to the first of a comma-separated list of addresses that answers: all are tried
/// at once, so an address that isn't reachable from here doesn't hold things up. When none
/// answers, says what happened at each address:
/// "refused" (reached the computer, nothing listening: the pairing ended, or the wrong
/// port) or "no answer" (a firewall, or a network this device isn't on).
pub(crate) async fn connect_first(list: &str) -> Result<tokio::net::TcpStream, String> {
    let mut tries = tokio::task::JoinSet::new();
    for (i, a) in list
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .enumerate()
    {
        let a = a.to_owned();
        tries.spawn(async move {
            // Earlier addresses get a head start, so the home network wins when both work.
            tokio::time::sleep(Duration::from_millis(150 * i as u64)).await;
            let r = match tokio::time::timeout(
                Duration::from_secs(4),
                tokio::net::TcpStream::connect(&a),
            )
            .await
            {
                Ok(Ok(s)) => Ok(s),
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                    Err("refused".to_owned())
                }
                Ok(Err(e)) => Err(e.kind().to_string()),
                Err(_) => Err("no answer".to_owned()),
            };
            (a, r)
        });
    }
    let mut failed = vec![];
    while let Some(r) = tries.join_next().await {
        if let Ok((a, r)) = r {
            match r {
                Ok(s) => return Ok(s),
                Err(why) => failed.push(format!("{a} ({why})")),
            }
        }
    }
    Err(failed.join(", "))
}

/// Every address other devices might reach this one at: the home network first, then any
/// others (Tailscale's 100.x, a VPN, a second network card). IPv4 only, no loopback or
/// link-local addresses.
pub(crate) fn local_ips() -> Vec<std::net::IpAddr> {
    let mut ips: Vec<std::net::IpAddr> = lan_ip().into_iter().collect();
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        let ip = i.ip();
        let usable = match ip {
            std::net::IpAddr::V4(v4) => {
                !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified()
            }
            std::net::IpAddr::V6(_) => false,
        };
        if usable && !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    ips
}

/// Addresses at a port, comma-separated, as links and invites carry them.
pub(crate) fn addresses(port: u16) -> Option<String> {
    let list: Vec<String> = local_ips()
        .into_iter()
        .map(|ip| std::net::SocketAddr::new(ip, port).to_string())
        .collect();
    (!list.is_empty()).then(|| list.join(","))
}

/// Start pairing on the existing device. `keys` are sent once the person types the number;
/// `records` (what this device has) go along so the new device starts complete.
#[cfg(test)]
pub async fn host(
    keys: SpaceKeys,
    links: Value,
    records: Vec<Vec<u8>>,
    relay: Option<String>,
    lan: bool,
    short: bool,
) -> Result<Host> {
    start(keys, links, records, relay, lan, short)
}

/// The same as [`host`], callable from inside the engine (needs a running Tokio runtime).
pub fn start(
    keys: SpaceKeys,
    links: Value,
    records: Vec<Vec<u8>>,
    relay: Option<String>,
    lan: bool,
    short: bool,
) -> Result<Host> {
    let listener = if lan {
        let l = std::net::TcpListener::bind("0.0.0.0:0")?;
        l.set_nonblocking(true)?;
        Some(tokio::net::TcpListener::from_std(l)?)
    } else {
        None
    };
    let addr = match &listener {
        Some(l) => addresses(l.local_addr()?.port()),
        None => None,
    };
    ensure!(
        relay.is_some() || addr.is_some(),
        "Add a sync server, or allow pairing on this network"
    );
    let invite = Invite {
        id: code(4)?.to_lowercase(),
        secret: if short {
            code(8)?
        } else {
            b64(&crate::vault::random::<16>()?).replace(['+', '/'], "x")
        },
        relay: relay.clone(),
        addr: addr.clone(),
    };
    // Typed: the server, or the addresses one per line to choose from.
    let typed_where = relay
        .clone()
        .or(addr.as_ref().map(|a| a.replace(',', " or ")))
        .unwrap_or_default();
    let view = Arc::new(Mutex::new(HostView {
        link: invite.link(),
        typed: format!(
            "{} at {typed_where}",
            Invite {
                secret: invite.secret.clone(),
                ..invite.clone()
            }
            .typed()
        ),
        invite: invite.clone(),
        status: "Waiting for the other device".into(),
        ..Default::default()
    }));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Option<u32>>();
    let v = view.clone();
    let task = tokio::spawn(async move {
        let result = tokio::time::timeout(
            EXPIRES,
            host_loop(v.clone(), invite, listener, keys, links, records, rx),
        )
        .await;
        let mut g = v.lock().unwrap();
        g.done = Some(match result {
            Ok(Ok(m)) => Ok(m),
            Ok(Err(e)) => Err(format!("{e:#}")),
            Err(_) => Err("The pairing expired".into()),
        });
        g.status = match &g.done {
            Some(Ok(m)) => format!("Added {}", m.name),
            Some(Err(e)) => e.clone(),
            None => String::new(),
        };
    });
    Ok(Host {
        view,
        answers: tx,
        task,
    })
}

async fn host_loop(
    view: Arc<Mutex<HostView>>,
    invite: Invite,
    listener: Option<tokio::net::TcpListener>,
    keys: SpaceKeys,
    links: Value,
    records: Vec<Vec<u8>>,
    mut answers: tokio::sync::mpsc::UnboundedReceiver<Option<u32>>,
) -> Result<Member> {
    for attempt in 1..=TRIES {
        // Whichever way the new device arrives first.
        let mut ch = match (&invite.relay, &listener) {
            (Some(r), Some(l)) => tokio::select! {
                c = rendezvous(r, &invite.id) => c?,
                a = l.accept() => Channel::Tcp(BufReader::new(a?.0)),
            },
            (Some(r), None) => rendezvous(r, &invite.id).await?,
            (None, Some(l)) => Channel::Tcp(BufReader::new(l.accept().await?.0)),
            (None, None) => bail!("Nowhere to pair"),
        };
        let key = match agree(&mut ch, &invite, true).await {
            Ok(k) => k,
            Err(e) if e.to_string() == "WRONG_CODE" || attempt < TRIES => {
                view.lock().unwrap().status = format!("Wrong code ({attempt} of {TRIES} tries)");
                continue;
            }
            Err(e) => return Err(e),
        };
        let hello = ch.recv().await?;
        let who = open(&key, hello["sealed"].as_str().unwrap_or(""))?;
        let member = Member {
            id: who["id"].as_str().context("No device ID")?.into(),
            name: who["name"]
                .as_str()
                .unwrap_or("New device")
                .chars()
                .take(64)
                .collect(),
            x25519: who["x25519"].as_str().context("No device key")?.into(),
            secrets: who["secrets"].as_bool().unwrap_or(true),
            added: super::core::Hlc::now_ms(),
            removed: None,
        };
        {
            let mut g = view.lock().unwrap();
            g.asking = true;
            g.joiner = Some(member.name.clone());
            g.status = format!("Type the number {} shows", member.name);
        }
        // Answers typed before the new device arrived don't count.
        while answers.try_recv().is_ok() {}
        let want = number(&key);
        let mut misses = 0;
        loop {
            match answers.recv().await.flatten() {
                Some(n) if n == want => break,
                Some(_) if misses + 1 < NUMBER_TRIES => {
                    misses += 1;
                    view.lock().unwrap().status = format!(
                        "That's not the number {} shows ({misses} of {NUMBER_TRIES} tries)",
                        member.name
                    );
                }
                Some(_) => {
                    let _ = ch.send(&json!({"t": "refused"})).await;
                    bail!("Wrong number {NUMBER_TRIES} times; start pairing again");
                }
                None => {
                    let _ = ch.send(&json!({"t": "refused"})).await;
                    bail!("Pairing cancelled");
                }
            }
        }
        view.lock().unwrap().status = "Sending the keys…".into();
        let mut k = keys.clone();
        if !member.secrets {
            k.secrets.clear();
        }
        let payload = json!({ "keys": k, "links": links, "records": records.iter().map(|r| b64(r)).collect::<Vec<_>>() });
        ch.send(&json!({"t": "welcome", "sealed": seal(&key, &payload)?}))
            .await?;
        let ack = ch.recv().await.unwrap_or(Value::Null);
        ensure!(ack["t"] == "thanks", "The other device didn't finish");
        return Ok(member);
    }
    bail!("Too many wrong codes; start pairing again")
}

/// What the new device receives.
pub struct Joined {
    pub keys: SpaceKeys,
    pub links: Value,
    pub records: Vec<Vec<u8>>,
}

/// Join from the new device. `on_number` is called with the number to show; the person types
/// it on the other device.
pub async fn join(
    invite: &Invite,
    me: &super::core::Device,
    secrets: bool,
    on_number: impl Fn(u32),
) -> Result<Joined> {
    let mut ch = match (&invite.addr, &invite.relay) {
        (Some(a), relay) => match connect_first(a).await {
            Ok(s) => Channel::Tcp(BufReader::new(s)),
            Err(tried) => match relay.as_deref() {
                Some(r) => rendezvous(r, &invite.id).await?,
                None => bail!(
                    "Couldn't reach the other device: {tried}. \"No answer\" usually means a firewall (allow Spindle on all networks, including Tailscale's) or that this device isn't on that network; \"refused\" means the pairing there has ended."
                ),
            },
        },
        (None, Some(r)) => rendezvous(r, &invite.id).await?,
        (None, None) => bail!("The pairing has nowhere to connect"),
    };
    let key = agree(&mut ch, invite, false).await.map_err(|e| {
        if e.to_string() == "WRONG_CODE" {
            anyhow!("Wrong code")
        } else {
            e
        }
    })?;
    on_number(number(&key));
    let me_json =
        json!({ "id": me.id, "name": me.name, "x25519": me.x_public()?, "secrets": secrets });
    ch.send(&json!({"t": "hello", "sealed": seal(&key, &me_json)?}))
        .await?;
    // Only the number-confirmed welcome carries the existing sync snapshot.
    let welcome = tokio::time::timeout(EXPIRES, ch.recv_with_limit(MAX_WELCOME_FRAME))
        .await
        .context("Nobody typed the number on the other device")??;
    ensure!(
        welcome["t"] == "welcome",
        "The other device didn't accept the number; start pairing again"
    );
    let payload = open(&key, welcome["sealed"].as_str().unwrap_or(""))?;
    let keys: SpaceKeys = serde_json::from_value(payload["keys"].clone())?;
    let records = payload["records"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r.as_str().and_then(|s| unb64(s).ok()))
        .collect();
    let _ = ch.send(&json!({"t": "thanks"})).await;
    Ok(Joined {
        keys,
        links: payload["links"].clone(),
        records,
    })
}

/// A QR code for the terminal.
pub fn qr_text(link: &str) -> Result<String> {
    let code = qrcode::QrCode::new(link.as_bytes())?;
    Ok(code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build())
}

/// A QR code as rows of dark (true) and light modules, for drawing in the window.
pub fn qr_modules(link: &str) -> Result<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::new(link.as_bytes())?;
    let width = code.width();
    Ok((
        width,
        code.to_colors()
            .iter()
            .map(|c| *c == qrcode::Color::Dark)
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::super::core::Device;
    use super::*;

    #[test]
    fn links_and_typed_codes_round_trip() {
        let inv = Invite {
            id: "abcd".into(),
            secret: "EFGHJKLM".into(),
            relay: Some("https://sync.example:8443".into()),
            addr: Some("192.168.1.42:43117".into()),
        };
        assert_eq!(Invite::parse(&inv.link(), None).unwrap(), inv);
        let typed = Invite::parse("abcd-efgh-jklm", Some("192.168.1.42:43117")).unwrap();
        assert_eq!(typed.secret, "EFGHJKLM");
        assert_eq!(typed.addr.as_deref(), Some("192.168.1.42:43117"));
        assert!(Invite::parse("abcd-efgh-jklm", None).is_err());
        assert!(qr_text(&inv.link()).unwrap().lines().count() > 10);
    }

    #[tokio::test]
    async fn pairing_over_the_lan_hands_over_keys_after_confirmation() {
        let keys = SpaceKeys::generate().unwrap();
        // A populated pool can exceed the small pre-authentication frame limit.
        let records = vec![vec![b'r'; MAX_HANDSHAKE_FRAME]];
        let host = host(
            keys.clone(),
            json!([]),
            records.clone(),
            None,
            true,
            true,
        )
        .await
        .unwrap();
        let mut invite = host.view.lock().unwrap().invite.clone();
        // The LAN address might be a real interface; connect over loopback in the test.
        let port = invite
            .addr
            .as_ref()
            .unwrap()
            .rsplit(':')
            .next()
            .unwrap()
            .to_owned();
        // Two addresses, the first unreachable (like a home IP seen from outside): the next works.
        invite.addr = Some(format!("127.0.0.1:1,127.0.0.1:{port}"));
        let phone = Device::generate("Phone").unwrap();
        let seen = Arc::new(Mutex::new(0));
        let s2 = seen.clone();
        // Typed before the new device arrives: ignored.
        host.answer(Some(1));
        let joiner = tokio::spawn({
            let invite = invite.clone();
            async move { join(&invite, &phone, false, move |n| *s2.lock().unwrap() = n).await }
        });
        // Wait for the question, then type the number the new device shows, after one typo.
        wait_asking(&host).await;
        let shown = *seen.lock().unwrap();
        assert!((1..=100).contains(&shown));
        host.answer(Some(if shown == 100 { 1 } else { shown + 1 }));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(host.view.lock().unwrap().status.contains("1 of 3"));
        host.answer(Some(shown));
        let joined = joiner.await.unwrap().unwrap();
        assert_eq!(joined.keys.space, keys.space);
        assert!(
            joined.keys.secrets.is_empty(),
            "a device that may not hold API keys doesn't get the key"
        );
        assert_eq!(joined.records, records);
        for _ in 0..100 {
            if host.view.lock().unwrap().done.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let done = host.view.lock().unwrap().done.clone().unwrap().unwrap();
        assert_eq!(done.name, "Phone");
    }

    async fn wait_asking(host: &Host) {
        for _ in 0..100 {
            if host.view.lock().unwrap().asking {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the new device never arrived");
    }

    #[tokio::test]
    async fn three_wrong_numbers_get_nothing() {
        let keys = SpaceKeys::generate().unwrap();
        let host = host(keys, json!([]), vec![], None, true, true)
            .await
            .unwrap();
        let mut invite = host.view.lock().unwrap().invite.clone();
        let port = invite
            .addr
            .as_ref()
            .unwrap()
            .rsplit(':')
            .next()
            .unwrap()
            .to_owned();
        invite.addr = Some(format!("127.0.0.1:{port}"));
        let seen = Arc::new(Mutex::new(0));
        let s2 = seen.clone();
        let phone = Device::generate("Phone").unwrap();
        let joiner = tokio::spawn(async move {
            join(&invite, &phone, true, move |n| *s2.lock().unwrap() = n).await
        });
        wait_asking(&host).await;
        let wrong = *seen.lock().unwrap() % 100 + 1;
        for _ in 0..3 {
            host.answer(Some(wrong));
        }
        let e = joiner.await.unwrap().err().unwrap();
        assert!(e.to_string().contains("didn't accept"), "{e}");
        tokio::time::sleep(Duration::from_millis(50)).await;
        let done = host.view.lock().unwrap().done.clone().unwrap();
        assert!(done.unwrap_err().contains("Wrong number 3 times"));
    }

    #[tokio::test]
    async fn a_wrong_code_gets_nothing() {
        let keys = SpaceKeys::generate().unwrap();
        let host = host(keys, json!([]), vec![], None, true, true)
            .await
            .unwrap();
        let mut invite = host.view.lock().unwrap().invite.clone();
        let port = invite
            .addr
            .as_ref()
            .unwrap()
            .rsplit(':')
            .next()
            .unwrap()
            .to_owned();
        invite.addr = Some(format!("127.0.0.1:{port}"));
        invite.secret = "WRONGCOD".into();
        let phone = Device::generate("Phone").unwrap();
        let e = join(&invite, &phone, true, |_| {}).await.err().unwrap();
        assert!(e.to_string().contains("Wrong code"), "{e}");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(host.view.lock().unwrap().status.contains("1 of 5"));
    }

    #[tokio::test]
    async fn pairing_through_the_sync_server() {
        let dir = tempfile::tempdir().unwrap();
        let server = super::super::hosted::Server::new(super::super::hosted::Config {
            data: dir.path().into(),
            mode: super::super::hosted::Mode::Relay,
            mailbox: Duration::ZERO,
            admin: String::new(),
        })
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, super::super::hosted::router(server))
                .await
                .unwrap()
        });
        let keys = SpaceKeys::generate().unwrap();
        let host = host(
            keys.clone(),
            json!([]),
            vec![],
            Some(url.clone()),
            false,
            true,
        )
        .await
        .unwrap();
        let typed = host.view.lock().unwrap().invite.typed();
        // The person types the code and the server address.
        let invite = Invite::parse(&typed, Some(&url)).unwrap();
        let lap = Device::generate("Laptop").unwrap();
        let seen = Arc::new(Mutex::new(0));
        let s2 = seen.clone();
        let joiner = tokio::spawn(async move {
            join(&invite, &lap, true, move |n| *s2.lock().unwrap() = n).await
        });
        wait_asking(&host).await;
        host.answer(Some(*seen.lock().unwrap()));
        let joined = joiner.await.unwrap().unwrap();
        assert_eq!(joined.keys.space, keys.space);
        assert!(!joined.keys.secrets.is_empty());
    }
}
