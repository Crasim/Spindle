//! Peer to peer on the local network: no server and no shared folder.
//!
//! Every syncing device listens on TCP (port 47613, or any free port) and announces itself
//! with a small UDP broadcast every few seconds. The announcement carries only a tag derived
//! from the space key (strangers can't tell which space it is), the device ID and the port.
//! A device that hears a member of its own space connects to it directly.
//!
//! A connection starts with a handshake: fresh X25519 keys on both sides, each side signs the
//! transcript with its device key, and each checks that the other is on the space's device
//! list (removed devices are turned away). Everything after that is encrypted with AES-GCM
//! under keys from the X25519 exchange, numbered so nothing can be replayed or reordered.
//! Over that channel a device can list, get and put records, the same three things every
//! transport offers. Records it receives wait in an inbox until the next sync round takes
//! them in (checking every signature, as always), and arriving records wake that round.

use super::core::{Envelope, MAX_RECORD, b64, hex, hkdf32, sha256, unb64, verify};
use super::space::Identity;
use super::transport::{Listing, valid_id};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const PORT: u16 = 47613;
pub const BEACON_PORT: u16 = 47614;
const BEACON_EVERY: Duration = Duration::from_secs(5);
/// A device not heard from for this long is no longer offered as a peer.
const FORGET_AFTER: Duration = Duration::from_secs(10 * 60);
const MAX_FRAME: usize = 64 << 20;
const CONNECT: Duration = Duration::from_secs(3);

/// What the listener needs to know about this device and its space.
#[derive(Clone)]
struct Ctx {
    identity: Identity,
    /// The space's record store on this device.
    dir: PathBuf,
    /// Devices on the space's list (not removed).
    members: Vec<String>,
    tag: String,
    port: u16,
}

static CTX: Mutex<Option<Ctx>> = Mutex::new(None);
/// The listener and the beacon, with the space they serve.
static TASKS: Mutex<Option<(String, Vec<tokio::task::JoinHandle<()>>)>> = Mutex::new(None);
/// Devices heard on the network: device ID → (address, when).
static SEEN: Mutex<BTreeMap<String, (String, Instant)>> = Mutex::new(BTreeMap::new());
/// Set when a peer sent records or a new peer appeared.
/// The last connections in and out, for `/sync check`: what each other device said.
static EVENTS: Mutex<std::collections::VecDeque<String>> =
    Mutex::new(std::collections::VecDeque::new());

/// Remember one connection event (newest last, the last 40 kept).
pub fn note(what: impl Into<String>) {
    let what = what.into();
    if what.is_empty() {
        return;
    }
    let line = format!("{} {what}", chrono::Local::now().format("%H:%M:%S"));
    let mut e = EVENTS.lock().unwrap();
    if e.back() == Some(&line) {
        return;
    }
    e.push_back(line);
    while e.len() > 40 {
        e.pop_front();
    }
}

pub fn events() -> Vec<String> {
    EVENTS.lock().unwrap().iter().cloned().collect()
}

/// Whether this device hears the others' announcements (the beacon port was free).
pub static HEARS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub static WAKE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The tag in announcements: derived from the space's first key, so it stays the same when
/// keys rotate and means nothing to anyone outside the space.
fn tag(identity: &Identity) -> Result<String> {
    let first = identity
        .keys
        .epochs
        .values()
        .next()
        .context("No space key")?;
    Ok(hex(&hkdf32(first, b"spindle-peer", b"beacon-tag")[..12]))
}

/// Start (or keep) listening for this space, with the latest device list.
pub fn serve(identity: &Identity, dir: &Path, members: Option<Vec<String>>) -> Result<()> {
    let tag = tag(identity)?;
    let mut tasks = TASKS.lock().unwrap();
    let running = tasks
        .as_ref()
        .is_some_and(|(s, h)| *s == identity.keys.space && h.iter().all(|t| !t.is_finished()));
    {
        let mut c = CTX.lock().unwrap();
        let members = members
            .or_else(|| c.as_ref().map(|c| c.members.clone()))
            .unwrap_or_default();
        let port = c.as_ref().map(|c| c.port).unwrap_or(0);
        *c = Some(Ctx {
            identity: identity.clone(),
            dir: dir.to_owned(),
            members,
            tag: tag.clone(),
            port,
        });
    }
    if running {
        return Ok(());
    }
    if let Some((_, old)) = tasks.take() {
        old.iter().for_each(|t| t.abort());
    }
    // The usual port, else the one this device used last time (so the others' saved
    // address still works after a restart), else any free one, remembered for next time.
    let saved_at = crate::runtime::data_dir().join("peer-port");
    let saved: Option<u16> = std::fs::read_to_string(&saved_at)
        .ok()
        .and_then(|s| s.trim().parse().ok());
    let listener = std::net::TcpListener::bind(("0.0.0.0", PORT))
        .or_else(|e| match saved {
            Some(p) if p != PORT => std::net::TcpListener::bind(("0.0.0.0", p)),
            _ => Err(e),
        })
        .or_else(|_| std::net::TcpListener::bind(("0.0.0.0", 0)))?;
    if let Ok(a) = listener.local_addr()
        && Some(a.port()) != saved
    {
        let _ = std::fs::create_dir_all(crate::runtime::data_dir());
        let _ = std::fs::write(&saved_at, a.port().to_string());
    }
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    note(format!("listening on port {port}"));
    if let Some(c) = CTX.lock().unwrap().as_mut() {
        c.port = port;
    }
    let listener = tokio::net::TcpListener::from_std(listener)?;
    let accept = tokio::spawn(async move {
        while let Ok((s, from)) = listener.accept().await {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(300), answer(s, from)).await;
            });
        }
    });
    let beacon = tokio::spawn(beacon());
    *tasks = Some((identity.keys.space.clone(), vec![accept, beacon]));
    Ok(())
}

/// Stop listening (peer to peer turned off, or sync left).
pub fn stop() {
    if let Some((_, t)) = TASKS.lock().unwrap().take() {
        t.iter().for_each(|t| t.abort());
    }
    *CTX.lock().unwrap() = None;
    SEEN.lock().unwrap().clear();
}

/// The port this device listens on, once it does.
pub fn port() -> Option<u16> {
    CTX.lock()
        .unwrap()
        .as_ref()
        .map(|c| c.port)
        .filter(|p| *p != 0)
}

/// Members of this space heard on the network recently: (device, address).
pub fn discovered() -> Vec<(String, String)> {
    let mut seen = SEEN.lock().unwrap();
    seen.retain(|_, (_, t)| t.elapsed() < FORGET_AFTER);
    let members = CTX
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.members.clone())
        .unwrap_or_default();
    seen.iter()
        .filter(|(d, _)| members.contains(d))
        .map(|(d, (a, _))| (d.clone(), a.clone()))
        .collect()
}

fn heard(device: &str, addr: String) {
    let me = CTX
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.identity.device.id.clone());
    if me.as_deref() == Some(device) {
        return;
    }
    let mut seen = SEEN.lock().unwrap();
    let new = seen.get(device).is_none_or(|(a, _)| *a != addr);
    if new {
        note(format!("heard {} at {addr}", short(device)));
    }
    seen.insert(device.to_owned(), (addr, Instant::now()));
    if new {
        WAKE.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn udp() -> Result<tokio::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    // Several Spindle workspaces on one machine can all hear announcements.
    s.set_reuse_address(true)?;
    #[cfg(all(unix, not(target_os = "solaris")))]
    s.set_reuse_port(true)?;
    s.set_broadcast(true)?;
    s.set_nonblocking(true)?;
    s.bind(&std::net::SocketAddr::from(([0, 0, 0, 0], BEACON_PORT)).into())?;
    Ok(tokio::net::UdpSocket::from_std(s.into())?)
}

/// Announce this device and listen for the others.
async fn beacon() {
    let sock = udp();
    HEARS.store(sock.is_ok(), std::sync::atomic::Ordering::Relaxed);
    if let Err(e) = &sock {
        note(format!("can't hear other devices' announcements (UDP {BEACON_PORT}): {e}"));
    }
    let Ok(sock) = sock else {
        // Can't hear announcements (the port is taken by something else): announce only.
        loop {
            if let Ok(s) = tokio::net::UdpSocket::bind("0.0.0.0:0").await {
                let _ = s.set_broadcast(true);
                if let Some(msg) = announcement() {
                    let _ = s
                        .send_to(msg.as_bytes(), ("255.255.255.255", BEACON_PORT))
                        .await;
                }
            }
            tokio::time::sleep(BEACON_EVERY).await;
        }
    };
    let mut buf = vec![0u8; 2048];
    let mut next = Instant::now();
    loop {
        if Instant::now() >= next {
            if let Some(msg) = announcement() {
                let _ = sock
                    .send_to(msg.as_bytes(), ("255.255.255.255", BEACON_PORT))
                    .await;
            }
            next = Instant::now() + BEACON_EVERY;
        }
        let wait = next.saturating_duration_since(Instant::now());
        if let Ok(Ok((n, from))) = tokio::time::timeout(wait, sock.recv_from(&mut buf)).await {
            let Ok(v) = serde_json::from_slice::<Value>(&buf[..n]) else {
                continue;
            };
            let mine = CTX.lock().unwrap().as_ref().map(|c| c.tag.clone());
            if v["spindle"] != "peer1" || mine.is_none() || v["tag"].as_str() != mine.as_deref() {
                continue;
            }
            let (Some(device), Some(port)) = (v["device"].as_str(), v["port"].as_u64()) else {
                continue;
            };
            if port == 0 || port > u16::MAX as u64 {
                continue;
            }
            let addr = std::net::SocketAddr::new(from.ip(), port as u16).to_string();
            heard(device, addr);
        }
    }
}

/// A device id, short enough to read.
pub fn short(device: &str) -> String {
    device.chars().take(8).collect()
}

fn announcement() -> Option<String> {
    let c = CTX.lock().unwrap();
    let c = c.as_ref().filter(|c| c.port != 0)?;
    Some(
        json!({"spindle": "peer1", "tag": c.tag, "device": c.identity.device.id, "port": c.port})
            .to_string(),
    )
}

// ------------------------------------------------------------------ the channel

/// An authenticated, encrypted connection to another device of the space.
pub struct Conn {
    stream: TcpStream,
    send: [u8; 32],
    recv: [u8; 32],
    sent: u64,
    got: u64,
    /// The device on the other end.
    #[allow(dead_code)]
    pub device: String,
}

async fn write_raw(s: &mut TcpStream, data: &[u8]) -> Result<()> {
    ensure!(data.len() <= MAX_FRAME, "Message too large");
    s.write_all(&(data.len() as u32).to_be_bytes()).await?;
    s.write_all(data).await?;
    s.flush().await?;
    Ok(())
}

async fn read_raw(s: &mut TcpStream) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len)
        .await
        .context("The other device left")?;
    let len = u32::from_be_bytes(len) as usize;
    ensure!(len <= MAX_FRAME, "Message too large");
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn read_json(s: &mut TcpStream) -> Result<Value> {
    Ok(serde_json::from_slice(&read_raw(s).await?)?)
}

impl Conn {
    pub async fn send(&mut self, v: &Value) -> Result<()> {
        let aad = self.sent.to_be_bytes();
        self.sent += 1;
        let sealed = crate::vault::seal(&self.send, &aad, serde_json::to_vec(v)?)?;
        write_raw(&mut self.stream, &sealed).await
    }

    pub async fn recv(&mut self) -> Result<Value> {
        let sealed = read_raw(&mut self.stream).await?;
        let aad = self.got.to_be_bytes();
        self.got += 1;
        let plain = crate::vault::open(&self.recv, &aad, &sealed)
            .map_err(|_| anyhow!("Damaged message from the other device"))?;
        Ok(serde_json::from_slice(&plain)?)
    }

    async fn ask(&mut self, v: Value) -> Result<Value> {
        self.send(&v).await?;
        let r = self.recv().await?;
        if let Some(e) = r["error"].as_str() {
            bail!("{e}");
        }
        Ok(r)
    }
}

/// Both sides' fresh keys and nonces, hashed: what each side signs.
fn transcript(hi_client: &Value, hi_server: &Value) -> [u8; 32] {
    let mut data = b"spindle-peer-v1\0".to_vec();
    data.extend(hi_client.to_string().as_bytes());
    data.push(0);
    data.extend(hi_server.to_string().as_bytes());
    sha256(&data)
}

fn keys(shared: &[u8; 32], th: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    (
        hkdf32(shared, th, b"spindle-peer client to server"),
        hkdf32(shared, th, b"spindle-peer server to client"),
    )
}

fn fresh() -> Result<(x25519_dalek::StaticSecret, String, String)> {
    let secret = x25519_dalek::StaticSecret::from(crate::vault::random::<32>()?);
    let public = b64(x25519_dalek::PublicKey::from(&secret).as_bytes());
    Ok((secret, public, hex(&crate::vault::random::<16>()?)))
}

fn shared(secret: &x25519_dalek::StaticSecret, theirs: &Value) -> Result<[u8; 32]> {
    let bytes: [u8; 32] = unb64(theirs.as_str().unwrap_or(""))?
        .try_into()
        .map_err(|_| anyhow!("Invalid key from the other device"))?;
    let s = secret.diffie_hellman(&x25519_dalek::PublicKey::from(bytes));
    ensure!(s.was_contributory(), "Invalid key from the other device");
    Ok(*s.as_bytes())
}

fn signed_by(device: &str, label: &[u8], th: &[u8; 32], sig: &Value) -> bool {
    let mut msg = label.to_vec();
    msg.extend_from_slice(th);
    unb64(sig.as_str().unwrap_or("")).is_ok_and(|s| verify(device, &msg, &s))
}

fn sign(identity: &Identity, label: &[u8], th: &[u8; 32]) -> Result<String> {
    let mut msg = label.to_vec();
    msg.extend_from_slice(th);
    Ok(b64(&identity.device.sign(&msg)?))
}

/// Who is connecting: this device, the space's device list, and where it listens.
#[derive(Clone)]
pub struct Me {
    pub identity: Identity,
    pub members: Vec<String>,
    pub port: u16,
}

/// Connect to another device of the space. `expect` is the device that should answer, when
/// known; any member is accepted otherwise (an address typed by hand).
pub async fn connect(me: &Me, addr: &str, expect: Option<&str>) -> Result<Conn> {
    // A device can have several addresses (home network, Tailscale, and older ones from
    // before it restarted on another port): try each until the right device answers. An
    // old address may now belong to another device, or to this one: not an error, just
    // the wrong place.
    let own: Vec<String> = if me.port == 0 {
        vec![]
    } else {
        super::pair::local_ips()
            .into_iter()
            .map(|ip| std::net::SocketAddr::new(ip, me.port).to_string())
            .chain([format!("127.0.0.1:{}", me.port)])
            .collect()
    };
    let mut last = None;
    for a in addr.split(',').map(str::trim).filter(|a| !a.is_empty()) {
        if own.iter().any(|o| o == a) {
            continue;
        }
        let r = connect_one(me, a, expect).await;
        note(match &r {
            Ok(c) => format!("→ {a}: connected to {}", short(&c.device)),
            Err(e) if offline(e) => String::new(),
            Err(e) => format!("→ {a}: {e:#}"),
        });
        match r {
            Ok(c) => return Ok(c),
            Err(e) if offline(&e) || e.to_string().starts_with("WRONG_DEVICE") => {}
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("OFFLINE")))
}

async fn connect_one(me: &Me, addr: &str, expect: Option<&str>) -> Result<Conn> {
    let ctx = Ctx {
        tag: tag(&me.identity)?,
        identity: me.identity.clone(),
        members: me.members.clone(),
        port: me.port,
        dir: PathBuf::new(),
    };
    let mut s = match tokio::time::timeout(CONNECT, TcpStream::connect(addr)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            note(format!("→ {addr}: can't connect ({e})"));
            bail!("OFFLINE")
        }
        Err(_) => {
            note(format!("→ {addr}: no answer in {}s (asleep, closed, or a firewall)", CONNECT.as_secs()));
            bail!("OFFLINE")
        }
    };
    s.set_nodelay(true).ok();
    let (secret, eph, nonce) = fresh()?;
    let hi = json!({"t": "hi", "v": 1, "tag": ctx.tag, "device": ctx.identity.device.id,
        "eph": eph, "nonce": nonce, "port": ctx.port});
    write_raw(&mut s, hi.to_string().as_bytes()).await?;
    let reply = tokio::time::timeout(Duration::from_secs(10), read_json(&mut s))
        .await
        .map_err(|_| anyhow!("The other device didn't answer"))??;
    if let Some(e) = reply["error"].as_str() {
        // An old address now used by another Spindle (another person's, or another space).
        if e.starts_with("Not the same") && expect.is_some() {
            bail!("WRONG_DEVICE: {e}");
        }
        bail!("{e}");
    }
    let theirs = reply["hi"].clone();
    let device = theirs["device"].as_str().unwrap_or("").to_owned();
    ensure!(
        expect.is_none_or(|d| d == device),
        "WRONG_DEVICE: a different device answered at {addr}"
    );
    ensure!(
        ctx.members.contains(&device),
        "The device at {addr} isn't one of yours"
    );
    let th = transcript(&hi, &theirs);
    ensure!(
        signed_by(&device, b"server", &th, &reply["sig"]),
        "The device at {addr} couldn't prove who it is"
    );
    write_raw(
        &mut s,
        json!({"sig": sign(&ctx.identity, b"client", &th)?})
            .to_string()
            .as_bytes(),
    )
    .await?;
    let (c2s, s2c) = keys(&shared(&secret, &theirs["eph"])?, &th);
    let mut conn = Conn {
        stream: s,
        send: c2s,
        recv: s2c,
        sent: 0,
        got: 0,
        device,
    };
    let ok = conn.recv().await?;
    ensure!(
        ok["ok"] == true,
        "The other device turned the connection down"
    );
    Ok(conn)
}

/// The listening side of one connection.
async fn answer(mut s: TcpStream, from: std::net::SocketAddr) -> Result<()> {
    s.set_nodelay(true).ok();
    let hi = tokio::time::timeout(Duration::from_secs(10), read_json(&mut s)).await??;
    let Some(ctx) = CTX.lock().unwrap().clone() else {
        return Ok(());
    };
    if hi["t"] != "hi" || hi["tag"].as_str() != Some(ctx.tag.as_str()) {
        note(format!("← {from}: turned away, not this sync space"));
        let _ = write_raw(&mut s, br#"{"error":"Not the same Spindle space"}"#).await;
        return Ok(());
    }
    let device = hi["device"].as_str().unwrap_or("").to_owned();
    if !ctx.members.contains(&device) {
        note(format!(
            "← {from}: turned away {}, not on this device's list ({} devices known)",
            short(&device),
            ctx.members.len()
        ));
        let _ = write_raw(
            &mut s,
            json!({"error": format!(
                "{} turned this device away: it isn't on {}'s device list (removed there, or added after its last sync)",
                ctx.identity.device.name, ctx.identity.device.name
            )})
            .to_string()
            .as_bytes(),
        )
        .await;
        return Ok(());
    }
    let (secret, eph, nonce) = fresh()?;
    let mine =
        json!({"t": "hi", "v": 1, "device": ctx.identity.device.id, "eph": eph, "nonce": nonce});
    let th = transcript(&hi, &mine);
    write_raw(
        &mut s,
        json!({"hi": mine, "sig": sign(&ctx.identity, b"server", &th)?})
            .to_string()
            .as_bytes(),
    )
    .await?;
    let proof = tokio::time::timeout(Duration::from_secs(10), read_json(&mut s)).await??;
    ensure!(
        signed_by(&device, b"client", &th, &proof["sig"]),
        "The connecting device couldn't prove who it is"
    );
    let (c2s, s2c) = keys(&shared(&secret, &hi["eph"])?, &th);
    let mut conn = Conn {
        stream: s,
        send: s2c,
        recv: c2s,
        sent: 0,
        got: 0,
        device: device.clone(),
    };
    conn.send(&json!({"ok": true})).await?;
    note(format!("← {from}: {} connected", short(&device)));
    // It told us where it listens: a peer to sync with, too.
    if let Some(port) = hi["port"]
        .as_u64()
        .filter(|p| *p > 0 && *p <= u16::MAX as u64)
    {
        heard(
            &device,
            std::net::SocketAddr::new(from.ip(), port as u16).to_string(),
        );
    }
    loop {
        let req = match conn.recv().await {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };
        let reply = handle(&ctx.dir, &req).unwrap_or_else(|e| json!({"error": format!("{e:#}")}));
        conn.send(&reply).await?;
    }
}

fn records_dir(dir: &Path) -> PathBuf {
    dir.join("records")
}

fn inbox(dir: &Path) -> PathBuf {
    dir.join("inbox")
}

fn handle(dir: &Path, req: &Value) -> Result<Value> {
    match req["op"].as_str() {
        Some("list") => {
            let mut ids = list_dir(&records_dir(dir));
            ids.extend(list_dir(&inbox(dir)));
            ids.sort();
            ids.dedup();
            Ok(json!({"ids": ids}))
        }
        Some("get") => {
            let ids: Vec<String> = serde_json::from_value(req["ids"].clone())?;
            ensure!(ids.len() <= 1000, "Too many at once");
            let mut out = vec![];
            for id in ids.iter().filter(|i| valid_id(i)) {
                for path in [
                    records_dir(dir).join(&id[..2]).join(format!("{id}.json")),
                    inbox(dir).join(&id[..2]).join(format!("{id}.json")),
                ] {
                    if let Ok(m) = std::fs::metadata(&path)
                        && m.len() as usize <= MAX_RECORD
                    {
                        out.push(b64(&std::fs::read(&path)?));
                        break;
                    }
                }
            }
            Ok(json!({"records": out}))
        }
        Some("put") => {
            let recs: Vec<(String, String)> = serde_json::from_value(req["records"].clone())?;
            ensure!(recs.len() <= 1000, "Too many at once");
            let mut n = 0;
            for (id, data) in recs {
                let bytes = unb64(&data)?;
                ensure!(valid_id(&id) && bytes.len() <= MAX_RECORD, "Invalid record");
                // Only records that are what they claim to be; the sync round checks the
                // signature and the device list when it takes them in.
                let e = Envelope::parse(&bytes)?;
                ensure!(e.id()? == id, "Invalid record");
                if records_dir(dir)
                    .join(&id[..2])
                    .join(format!("{id}.json"))
                    .exists()
                {
                    continue;
                }
                let shard = inbox(dir).join(&id[..2]);
                std::fs::create_dir_all(&shard)?;
                let path = shard.join(format!("{id}.json"));
                if !path.exists() {
                    let mut tmp = tempfile::NamedTempFile::new_in(&shard)?;
                    std::io::Write::write_all(&mut tmp, &bytes)?;
                    let _ = tmp.persist_noclobber(&path);
                    n += 1;
                }
            }
            if n > 0 {
                WAKE.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(json!({"ok": true}))
        }
        _ => bail!("Unknown request"),
    }
}

fn list_dir(dir: &Path) -> Vec<String> {
    let mut ids = vec![];
    if let Ok(shards) = std::fs::read_dir(dir) {
        for shard in shards.flatten() {
            if let Ok(files) = std::fs::read_dir(shard.path()) {
                for f in files.flatten() {
                    let name = f.file_name().to_string_lossy().into_owned();
                    if let Some(id) = name.strip_suffix(".json")
                        && valid_id(id)
                    {
                        ids.push(id.to_owned());
                    }
                }
            }
        }
    }
    ids
}

/// Records other devices sent since the last round: taken out of the inbox for the space to
/// check and keep.
pub fn take_inbox(dir: &Path) -> Vec<Vec<u8>> {
    let mut out = vec![];
    let root = inbox(dir);
    for id in list_dir(&root) {
        let path = root.join(&id[..2]).join(format!("{id}.json"));
        if let Ok(b) = std::fs::read(&path) {
            out.push(b);
        }
        let _ = std::fs::remove_file(&path);
    }
    out
}

// ------------------------------------------------------------------ as a transport

/// Another device of the space, as a transport. One connection is kept for a sync round.
#[derive(Clone)]
pub struct Client {
    pub addr: String,
    pub device: Option<String>,
    me: Me,
    conn: std::sync::Arc<tokio::sync::Mutex<Option<Conn>>>,
}

impl Client {
    pub fn new(me: Me, addr: &str, device: Option<&str>) -> Self {
        Self {
            addr: addr.to_owned(),
            device: device.map(str::to_owned),
            me,
            conn: Default::default(),
        }
    }

    async fn ask(&self, v: Value) -> Result<Value> {
        let mut g = self.conn.lock().await;
        if g.is_none() {
            *g = Some(connect(&self.me, &self.addr, self.device.as_deref()).await?);
        }
        match g.as_mut().unwrap().ask(v).await {
            Ok(r) => Ok(r),
            Err(e) => {
                *g = None;
                Err(e)
            }
        }
    }

    pub async fn list(&self) -> Result<Listing> {
        let r = self.ask(json!({"op": "list"})).await?;
        Ok(Listing {
            ids: serde_json::from_value(r["ids"].clone())?,
            cursor: String::new(),
        })
    }

    pub async fn get(&self, ids: &[String]) -> Result<Vec<Vec<u8>>> {
        let mut out = vec![];
        for batch in ids.chunks(500) {
            let r = self.ask(json!({"op": "get", "ids": batch})).await?;
            for s in r["records"].as_array().into_iter().flatten() {
                out.push(unb64(s.as_str().unwrap_or(""))?);
            }
        }
        Ok(out)
    }

    pub async fn put(&self, records: &[(String, Vec<u8>)]) -> Result<()> {
        let recs: Vec<(String, String)> =
            records.iter().map(|(id, b)| (id.clone(), b64(b))).collect();
        self.ask(json!({"op": "put", "records": recs})).await?;
        Ok(())
    }
}

/// Whether an error only means the other device isn't there right now.
pub fn offline(e: &anyhow::Error) -> bool {
    e.to_string() == "OFFLINE"
}

#[cfg(test)]
mod tests {
    use super::super::core::Kind;
    use super::super::space::Space;
    use super::super::space::tests::join;
    use super::super::transport::{Progress, Transport, sync};
    use super::*;

    /// The listener serves the desktop; the laptop connects as a client. A stranger from
    /// another space and a device holding the keys but not on the list are turned away.
    #[tokio::test]
    async fn members_sync_directly_and_strangers_are_turned_away() {
        let dir = tempfile::tempdir().unwrap();
        let desk_dir = dir.path().join("desk");
        let mut desk = Space::create("Desktop", Some(desk_dir.clone())).unwrap();
        let mut lap = join(&desk, "Laptop", true);
        let entry = lap.member_entry().unwrap();
        desk.add_member(entry).unwrap();
        desk.put(Kind::Settings, "prefs", json!({"theme": "dark"}))
            .unwrap();
        desk.save().unwrap();
        let members: Vec<String> = desk.members().into_iter().map(|m| m.id).collect();
        serve(&desk.id, &desk_dir, Some(members.clone())).unwrap();
        let addr = format!("127.0.0.1:{}", port().unwrap());

        let me = Me {
            identity: lap.id.clone(),
            members: members.clone(),
            port: 0,
        };
        let t = Transport::Peer(Client::new(me.clone(), &addr, Some(&desk.id.device.id)));
        let mut p = Progress::default();
        let o = sync(&mut lap, &t, &mut p).await.unwrap();
        assert!(o.received > 0);
        assert_eq!(lap.value(Kind::Settings, "prefs").unwrap()["theme"], "dark");
        // The laptop's change goes to the desktop's inbox, and wakes it.
        lap.put(Kind::Settings, "prefs", json!({"theme": "light"}))
            .unwrap();
        WAKE.store(false, std::sync::atomic::Ordering::Relaxed);
        let o = sync(&mut lap, &t, &mut p).await.unwrap();
        assert!(o.sent > 0);
        assert!(WAKE.load(std::sync::atomic::Ordering::Relaxed));
        let arrived = take_inbox(&desk_dir);
        assert!(!arrived.is_empty());
        desk.receive(arrived).unwrap();
        assert_eq!(
            desk.value(Kind::Settings, "prefs").unwrap()["theme"],
            "light"
        );
        assert!(take_inbox(&desk_dir).is_empty());

        // Expecting a different device at that address: the wrong place, not an error (an
        // old address after a restart); with a good address after it, that one is used.
        let e = Client::new(me.clone(), &addr, Some(&lap.id.device.id))
            .list()
            .await
            .err()
            .unwrap();
        assert!(offline(&e), "{e}");
        let stale = format!("127.0.0.1:1,{addr}");
        assert!(
            Client::new(me.clone(), &stale, Some(&desk.id.device.id))
                .list()
                .await
                .is_ok()
        );
        // A stranger with another space.
        let other = Space::create("Stranger", None).unwrap();
        let e = Client::new(
            Me {
                identity: other.id.clone(),
                members: vec![],
                port: 0,
            },
            &addr,
            None,
        )
        .list()
        .await
        .err()
        .unwrap();
        assert!(e.to_string().contains("Not the same"), "{e}");
        // Holding the keys but not on the device list (removed, or never added).
        let outsider = join(&desk, "Outsider", true);
        let e = Client::new(
            Me {
                identity: outsider.id.clone(),
                members,
                port: 0,
            },
            &addr,
            None,
        )
        .list()
        .await
        .err()
        .unwrap();
        assert!(e.to_string().contains("device list"), "{e}");
        // Nobody listening.
        let e = Client::new(me, "127.0.0.1:1", None)
            .list()
            .await
            .err()
            .unwrap();
        assert!(offline(&e));
        stop();
    }
}
