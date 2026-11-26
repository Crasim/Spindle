//! The hosted sync server (`spindle sync-server`) and its client.
//!
//! The server only ever holds encrypted, signed records. In **store** mode it keeps them, so
//! a device can catch up when no other device is on. In **relay** mode it keeps nothing but a
//! short mailbox (records waiting for offline devices, deleted after `--mailbox-hours`) and
//! forwards live notices between connected devices. **both** does both.
//!
//! Devices prove who they are by signing each request with their Ed25519 key; the server
//! knows each space's device keys (not its names or contents). An optional account lets a
//! new device join without another device online: the device signs in with a key derived
//! from the password (the password itself never leaves the device) and receives the space
//! key wrapped with another key derived from the same password.

use super::core::{Device, Envelope, MAX_RECORD, b64, hex, sha256, unb64, verify};
use super::transport::{Listing, valid_id};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Json, Router,
    body::Bytes,
    extract::{
        Path, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Store,
    Relay,
    Both,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "store" => Mode::Store,
            "relay" => Mode::Relay,
            "both" => Mode::Both,
            _ => bail!("--mode is store, relay or both"),
        })
    }
    fn stores(self) -> bool {
        matches!(self, Mode::Store | Mode::Both)
    }
}

pub struct Config {
    pub data: PathBuf,
    pub mode: Mode,
    pub mailbox: Duration,
    pub admin: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Account {
    user: String,
    salt_auth: String,
    salt_wrap: String,
    /// SHA-256 of the key derived for signing in; the key itself is never stored.
    auth_hash: String,
    wrapped: String,
}

#[derive(Default, Serialize, Deserialize)]
struct Meta {
    devices: BTreeSet<String>,
    #[serde(default)]
    account: Option<Account>,
    /// Sequence of stored records, in arrival order.
    #[serde(default)]
    log: Vec<String>,
}

struct SpaceData {
    meta: Meta,
    /// Relay mode: records waiting for devices, with when they arrived.
    mailbox: VecDeque<(u64, String, Vec<u8>, Instant)>,
    seq: u64,
    live: broadcast::Sender<String>,
}

struct Pairing {
    created: Instant,
    peers: Vec<tokio::sync::mpsc::UnboundedSender<String>>,
}

pub struct Server {
    cfg: Config,
    spaces: Mutex<BTreeMap<String, SpaceData>>,
    nonces: Mutex<VecDeque<(String, Instant)>>,
    attempts: Mutex<BTreeMap<String, Vec<Instant>>>,
    enroll: Mutex<BTreeMap<String, (String, Instant)>>,
    pairings: Mutex<BTreeMap<String, Pairing>>,
}

type Shared = Arc<Server>;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl Server {
    pub fn new(cfg: Config) -> Result<Shared> {
        std::fs::create_dir_all(&cfg.data)?;
        let mut spaces = BTreeMap::new();
        for entry in std::fs::read_dir(&cfg.data)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let meta_path = entry.path().join("meta.json");
            if safe_name(&name)
                && let Ok(bytes) = std::fs::read(&meta_path)
                && let Ok(meta) = serde_json::from_slice::<Meta>(&bytes)
            {
                let seq = meta.log.len() as u64;
                spaces.insert(
                    name,
                    SpaceData {
                        meta,
                        mailbox: VecDeque::new(),
                        seq,
                        live: broadcast::channel(256).0,
                    },
                );
            }
        }
        Ok(Arc::new(Server {
            cfg,
            spaces: Mutex::new(spaces),
            nonces: Mutex::new(VecDeque::new()),
            attempts: Mutex::new(BTreeMap::new()),
            enroll: Mutex::new(BTreeMap::new()),
            pairings: Mutex::new(BTreeMap::new()),
        }))
    }

    fn save_meta(&self, space: &str, meta: &Meta) -> Result<()> {
        let dir = self.cfg.data.join(space);
        std::fs::create_dir_all(&dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        serde_json::to_writer(&mut tmp, meta)?;
        tmp.persist(dir.join("meta.json"))?;
        Ok(())
    }

    /// Check a signed request: a registered device, a fresh time, an unused nonce, and a
    /// signature over the method, path, time, nonce and body.
    fn auth(
        &self,
        space: &str,
        method: &Method,
        uri: &Uri,
        h: &HeaderMap,
        body: &[u8],
    ) -> Result<String, Response> {
        let get = |k: &str| {
            h.get(k)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned()
        };
        let (device, time, nonce, sig) = (
            get("x-spindle-device"),
            get("x-spindle-time"),
            get("x-spindle-nonce"),
            get("x-spindle-sig"),
        );
        let now = super::core::Hlc::now_ms();
        let t: u64 = time
            .parse()
            .map_err(|_| err(StatusCode::UNAUTHORIZED, "Sign the request"))?;
        if now.abs_diff(t) > 5 * 60_000 || nonce.len() < 16 || nonce.len() > 64 {
            return Err(err(
                StatusCode::UNAUTHORIZED,
                "Request too old (check the device clock)",
            ));
        }
        let msg = signed_message(method.as_str(), uri.path(), &time, &nonce, body);
        let ok = unb64(&sig).is_ok_and(|s| verify(&device, msg.as_bytes(), &s));
        if !ok {
            return Err(err(StatusCode::UNAUTHORIZED, "Bad signature"));
        }
        {
            let spaces = self.spaces.lock().unwrap();
            let known = spaces
                .get(space)
                .is_some_and(|s| s.meta.devices.contains(&device));
            if !known {
                return Err(err(
                    StatusCode::FORBIDDEN,
                    "This device isn't registered for that space",
                ));
            }
        }
        let mut seen = self.nonces.lock().unwrap();
        while seen
            .front()
            .is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(600))
        {
            seen.pop_front();
        }
        let key = format!("{device}:{nonce}");
        if seen.iter().any(|(n, _)| *n == key) {
            return Err(err(StatusCode::UNAUTHORIZED, "Request replayed"));
        }
        seen.push_back((key, Instant::now()));
        Ok(device)
    }

    fn expire(&self, data: &mut SpaceData) {
        while data
            .mailbox
            .front()
            .is_some_and(|(_, _, _, at)| at.elapsed() > self.cfg.mailbox)
        {
            data.mailbox.pop_front();
        }
    }
}

pub fn signed_message(method: &str, path: &str, time: &str, nonce: &str, body: &[u8]) -> String {
    format!(
        "spindle-sync|{method}|{path}|{time}|{nonce}|{}",
        hex(&sha256(body))
    )
}

pub fn router(server: Shared) -> Router {
    Router::new()
        .route("/v1/health", get(|| async { Json(json!({"ok": true})) }))
        .route("/v1/spaces", post(create_space))
        .route("/v1/spaces/{space}/devices", post(add_device))
        .route(
            "/v1/spaces/{space}/devices/{device}",
            axum::routing::delete(remove_device),
        )
        .route(
            "/v1/spaces/{space}/records",
            get(list_records).put(put_records),
        )
        .route("/v1/spaces/{space}/get", post(get_records))
        .route("/v1/spaces/{space}/account", put(set_account))
        .route("/v1/spaces/{space}/enroll", post(enroll))
        .route("/v1/spaces/{space}/live", get(live))
        .route("/v1/accounts/{user}", get(account_salts))
        .route("/v1/accounts/{user}/login", post(login))
        .route("/v1/pair/{id}", get(pair))
        .with_state(server)
}

/// Run the server until the process ends.
pub async fn serve(cfg: Config, listen: &str) -> Result<()> {
    let mode = cfg.mode;
    let server = Server::new(cfg)?;
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("Couldn't listen on {listen}"))?;
    eprintln!(
        "Spindle sync server ({}) listening on {}",
        match mode {
            Mode::Store => "store",
            Mode::Relay => "relay",
            Mode::Both => "store and relay",
        },
        listener.local_addr()?
    );
    // Expire mailboxes and pairings in the background.
    let s = server.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            for d in s.spaces.lock().unwrap().values_mut() {
                s.expire(d);
            }
            s.pairings
                .lock()
                .unwrap()
                .retain(|_, p| p.created.elapsed() < Duration::from_secs(600));
        }
    });
    axum::serve(listener, router(server)).await?;
    Ok(())
}

#[derive(Deserialize)]
struct CreateSpace {
    space: String,
    device: String,
}

async fn create_space(
    State(s): State<Shared>,
    h: HeaderMap,
    Json(req): Json<CreateSpace>,
) -> Response {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if s.cfg.admin.is_empty() || !constant_eq(token.as_bytes(), s.cfg.admin.as_bytes()) {
        return err(
            StatusCode::UNAUTHORIZED,
            "The server's admin token is needed to add a space",
        );
    }
    if !safe_name(&req.space) || unb64_hex_len(&req.device) != 32 {
        return err(StatusCode::BAD_REQUEST, "Invalid space or device");
    }
    let mut spaces = s.spaces.lock().unwrap();
    if spaces.contains_key(&req.space) {
        return err(StatusCode::CONFLICT, "That space already exists");
    }
    let meta = Meta {
        devices: [req.device].into(),
        ..Default::default()
    };
    if let Err(e) = s.save_meta(&req.space, &meta) {
        return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    spaces.insert(
        req.space,
        SpaceData {
            meta,
            mailbox: VecDeque::new(),
            seq: 0,
            live: broadcast::channel(256).0,
        },
    );
    Json(json!({"ok": true})).into_response()
}

fn unb64_hex_len(hex_key: &str) -> usize {
    super::core::unhex(hex_key).map(|k| k.len()).unwrap_or(0)
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[derive(Deserialize)]
struct DeviceReq {
    device: String,
}

async fn add_device(
    State(s): State<Shared>,
    Path(space): Path<String>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = s.auth(&space, &method, &uri, &h, &body) {
        return r;
    }
    let Ok(req) = serde_json::from_slice::<DeviceReq>(&body) else {
        return err(StatusCode::BAD_REQUEST, "Expected {device}");
    };
    if unb64_hex_len(&req.device) != 32 {
        return err(StatusCode::BAD_REQUEST, "Invalid device key");
    }
    let mut spaces = s.spaces.lock().unwrap();
    let d = spaces.get_mut(&space).unwrap();
    d.meta.devices.insert(req.device);
    let _ = s.save_meta(&space, &d.meta);
    Json(json!({"ok": true})).into_response()
}

async fn remove_device(
    State(s): State<Shared>,
    Path((space, device)): Path<(String, String)>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
) -> Response {
    let me = match s.auth(&space, &method, &uri, &h, b"") {
        Ok(d) => d,
        Err(r) => return r,
    };
    if me == device {
        return err(StatusCode::BAD_REQUEST, "Remove a device from another one");
    }
    let mut spaces = s.spaces.lock().unwrap();
    let d = spaces.get_mut(&space).unwrap();
    d.meta.devices.remove(&device);
    let _ = s.save_meta(&space, &d.meta);
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct Since {
    #[serde(default)]
    since: String,
}

async fn list_records(
    State(s): State<Shared>,
    Path(space): Path<String>,
    Query(q): Query<Since>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
) -> Response {
    if let Err(r) = s.auth(&space, &method, &uri, &h, b"") {
        return r;
    }
    let since: u64 = q.since.parse().unwrap_or(0);
    let mut spaces = s.spaces.lock().unwrap();
    let d = spaces.get_mut(&space).unwrap();
    s.expire(d);
    let mut ids: Vec<String> = vec![];
    if s.cfg.mode.stores() {
        ids.extend(d.meta.log.iter().skip(since as usize).cloned());
    } else {
        ids.extend(
            d.mailbox
                .iter()
                .filter(|(seq, ..)| *seq > since)
                .map(|(_, id, ..)| id.clone()),
        );
    }
    Json(Listing {
        ids,
        cursor: d.seq.to_string(),
    })
    .into_response()
}

#[derive(Deserialize)]
struct Ids {
    ids: Vec<String>,
}

async fn get_records(
    State(s): State<Shared>,
    Path(space): Path<String>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = s.auth(&space, &method, &uri, &h, &body) {
        return r;
    }
    let Ok(req) = serde_json::from_slice::<Ids>(&body) else {
        return err(StatusCode::BAD_REQUEST, "Expected {ids}");
    };
    let mut out = vec![];
    let dir = s.cfg.data.join(&space).join("records");
    let spaces = s.spaces.lock().unwrap();
    let d = spaces.get(&space).unwrap();
    for id in req.ids.iter().filter(|i| valid_id(i)).take(500) {
        if let Some((_, _, bytes, _)) = d.mailbox.iter().find(|(_, x, ..)| x == id) {
            out.push(b64(bytes));
        } else if s.cfg.mode.stores()
            && let Ok(bytes) = std::fs::read(dir.join(&id[..2]).join(id))
        {
            out.push(b64(&bytes));
        }
    }
    Json(json!({ "records": out })).into_response()
}

#[derive(Deserialize)]
struct PutReq {
    records: Vec<PutRecord>,
}
#[derive(Deserialize)]
struct PutRecord {
    id: String,
    data: String,
}

async fn put_records(
    State(s): State<Shared>,
    Path(space): Path<String>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = s.auth(&space, &method, &uri, &h, &body) {
        return r;
    }
    let Ok(req) = serde_json::from_slice::<PutReq>(&body) else {
        return err(StatusCode::BAD_REQUEST, "Expected {records}");
    };
    let mut spaces = s.spaces.lock().unwrap();
    let d = spaces.get_mut(&space).unwrap();
    let mut new_ids = vec![];
    for r in req.records.iter().take(500) {
        let Ok(bytes) = unb64(&r.data) else { continue };
        if bytes.len() > MAX_RECORD || !valid_id(&r.id) || hex(&sha256(&bytes)) != r.id {
            return err(StatusCode::BAD_REQUEST, "A record doesn't match its ID");
        }
        // Only signed records from this space's devices.
        let Ok(e) = Envelope::parse(&bytes) else {
            return err(StatusCode::BAD_REQUEST, "Not a Spindle record");
        };
        if e.space != space || e.verified().is_err() || !d.meta.devices.contains(&e.device) {
            return err(
                StatusCode::FORBIDDEN,
                "A record isn't signed by a device of this space",
            );
        }
        let known = d.meta.log.contains(&r.id) || d.mailbox.iter().any(|(_, x, ..)| *x == r.id);
        if known {
            continue;
        }
        d.seq += 1;
        if s.cfg.mode.stores() {
            let dir = s.cfg.data.join(&space).join("records").join(&r.id[..2]);
            if std::fs::create_dir_all(&dir)
                .and_then(|_| std::fs::write(dir.join(&r.id), &bytes))
                .is_err()
            {
                return err(StatusCode::INTERNAL_SERVER_ERROR, "Couldn't save a record");
            }
            d.meta.log.push(r.id.clone());
        } else if !s.cfg.mailbox.is_zero() {
            d.mailbox
                .push_back((d.seq, r.id.clone(), bytes, Instant::now()));
        } else {
            // A pure relay: live devices get the record itself in the notice.
            let _ = d
                .live
                .send(json!({"type":"record","id": r.id, "data": r.data}).to_string());
        }
        new_ids.push(r.id.clone());
    }
    if s.cfg.mode.stores() && !new_ids.is_empty() {
        let _ = s.save_meta(&space, &d.meta);
    }
    if !new_ids.is_empty() {
        let _ = d.live.send(
            json!({"type":"records","ids": new_ids, "cursor": d.seq.to_string()}).to_string(),
        );
    }
    Json(json!({"ok": true, "stored": new_ids.len()})).into_response()
}

#[derive(Deserialize)]
struct AccountReq {
    user: String,
    salt_auth: String,
    salt_wrap: String,
    auth_key: String,
    wrapped: String,
}

async fn set_account(
    State(s): State<Shared>,
    Path(space): Path<String>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = s.auth(&space, &method, &uri, &h, &body) {
        return r;
    }
    let Ok(req) = serde_json::from_slice::<AccountReq>(&body) else {
        return err(StatusCode::BAD_REQUEST, "Expected the account fields");
    };
    let user = req.user.trim().to_lowercase();
    if user.is_empty() || user.len() > 128 {
        return err(StatusCode::BAD_REQUEST, "Choose a user name");
    }
    let mut spaces = s.spaces.lock().unwrap();
    let taken = spaces
        .iter()
        .any(|(id, d)| *id != space && d.meta.account.as_ref().is_some_and(|a| a.user == user));
    if taken {
        return err(
            StatusCode::CONFLICT,
            "That user name is taken on this server",
        );
    }
    let Ok(auth) = unb64(&req.auth_key) else {
        return err(StatusCode::BAD_REQUEST, "Invalid key");
    };
    let d = spaces.get_mut(&space).unwrap();
    d.meta.account = Some(Account {
        user,
        salt_auth: req.salt_auth,
        salt_wrap: req.salt_wrap,
        auth_hash: hex(&sha256(&auth)),
        wrapped: req.wrapped,
    });
    let _ = s.save_meta(&space, &d.meta);
    Json(json!({"ok": true})).into_response()
}

/// The salts for a user. Unknown users get salts derived from the name, so the answer
/// doesn't reveal who has an account.
async fn account_salts(State(s): State<Shared>, Path(user): Path<String>) -> Response {
    let user = user.trim().to_lowercase();
    let spaces = s.spaces.lock().unwrap();
    let found = spaces
        .values()
        .find_map(|d| d.meta.account.as_ref().filter(|a| a.user == user));
    match found {
        Some(a) => {
            Json(json!({"salt_auth": a.salt_auth, "salt_wrap": a.salt_wrap})).into_response()
        }
        None => {
            let fake = |label: &str| {
                b64(&sha256(format!("{label}\0{user}\0{}", s.cfg.admin).as_bytes())[..16])
            };
            Json(json!({"salt_auth": fake("auth"), "salt_wrap": fake("wrap")})).into_response()
        }
    }
}

#[derive(Deserialize)]
struct LoginReq {
    auth_key: String,
}

async fn login(
    State(s): State<Shared>,
    Path(user): Path<String>,
    Json(req): Json<LoginReq>,
) -> Response {
    let user = user.trim().to_lowercase();
    {
        let mut tries = s.attempts.lock().unwrap();
        let list = tries.entry(user.clone()).or_default();
        list.retain(|t| t.elapsed() < Duration::from_secs(900));
        if list.len() >= 5 {
            return err(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many sign-in attempts; try again in 15 minutes",
            );
        }
        list.push(Instant::now());
    }
    let Ok(key) = unb64(&req.auth_key) else {
        return err(StatusCode::BAD_REQUEST, "Invalid key");
    };
    let spaces = s.spaces.lock().unwrap();
    let found = spaces.iter().find_map(|(id, d)| {
        d.meta
            .account
            .as_ref()
            .filter(|a| {
                a.user == user && constant_eq(a.auth_hash.as_bytes(), hex(&sha256(&key)).as_bytes())
            })
            .map(|a| (id.clone(), a.wrapped.clone()))
    });
    let Some((space, wrapped)) = found else {
        return err(StatusCode::UNAUTHORIZED, "Wrong user name or password");
    };
    s.attempts.lock().unwrap().remove(&user);
    let token = hex(&crate::vault::random::<16>().unwrap_or_default());
    s.enroll
        .lock()
        .unwrap()
        .insert(token.clone(), (space.clone(), Instant::now()));
    Json(json!({"space": space, "wrapped": wrapped, "enroll": token})).into_response()
}

#[derive(Deserialize)]
struct EnrollReq {
    token: String,
    device: String,
}

/// After signing in, a new device registers its key with the one-time token.
async fn enroll(
    State(s): State<Shared>,
    Path(space): Path<String>,
    Json(req): Json<EnrollReq>,
) -> Response {
    let ok = {
        let mut tokens = s.enroll.lock().unwrap();
        match tokens.remove(&req.token) {
            Some((sp, at)) => sp == space && at.elapsed() < Duration::from_secs(600),
            None => false,
        }
    };
    if !ok || unb64_hex_len(&req.device) != 32 {
        return err(StatusCode::UNAUTHORIZED, "Sign in again");
    }
    let mut spaces = s.spaces.lock().unwrap();
    let Some(d) = spaces.get_mut(&space) else {
        return err(StatusCode::NOT_FOUND, "No such space");
    };
    d.meta.devices.insert(req.device);
    let _ = s.save_meta(&space, &d.meta);
    Json(json!({"ok": true})).into_response()
}

async fn live(
    State(s): State<Shared>,
    Path(space): Path<String>,
    method: Method,
    uri: Uri,
    h: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let device = match s.auth(&space, &method, &uri, &h, b"") {
        Ok(d) => d,
        Err(r) => return r,
    };
    let rx = s
        .spaces
        .lock()
        .unwrap()
        .get(&space)
        .unwrap()
        .live
        .subscribe();
    let tx = s.spaces.lock().unwrap().get(&space).unwrap().live.clone();
    ws.on_upgrade(move |socket| live_loop(socket, device, rx, tx))
}

/// Forward notices to the device, and relay its frames to the others (the channel phase 2's
/// remote requests ride on).
async fn live_loop(
    mut socket: WebSocket,
    device: String,
    mut rx: broadcast::Receiver<String>,
    tx: broadcast::Sender<String>,
) {
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(text) => {
                    // Relay frames aren't echoed to their sender, and go only where addressed.
                    if let Ok(v) = serde_json::from_str::<Value>(&text)
                        && v["type"] == "relay"
                        && (v["from"] == device.as_str() || (v["to"] != "*" && v["to"] != device.as_str()))
                    {
                        continue;
                    }
                    if socket.send(Message::Text(text.into())).await.is_err() { break; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Tell the device to catch up with a normal sync.
                    let _ = socket.send(Message::Text(json!({"type":"records","ids":[]}).to_string().into())).await;
                }
                Err(_) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(t))) => {
                    if t.len() > 1 << 20 { continue; }
                    let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                    if v["type"] == "relay" {
                        let _ = tx.send(json!({
                            "type": "relay",
                            "from": device,
                            "to": v["to"].as_str().unwrap_or("*"),
                            "payload": v["payload"],
                        }).to_string());
                    }
                }
                Some(Ok(Message::Ping(p))) => { let _ = socket.send(Message::Pong(p)).await; }
                Some(Ok(_)) => {}
                _ => break,
            }
        }
    }
}

/// Pairing rendezvous: the first two connections to an ID are piped together. The server
/// sees only the pairing protocol's messages (SPAKE2 and encrypted frames).
async fn pair(State(s): State<Shared>, Path(id): Path<String>, ws: WebSocketUpgrade) -> Response {
    if !safe_name(&id) || id.len() < 4 {
        return err(StatusCode::BAD_REQUEST, "Invalid pairing ID");
    }
    ws.on_upgrade(move |socket| pair_loop(s, id, socket))
}

async fn pair_loop(s: Shared, id: String, mut socket: WebSocket) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let other = {
        let mut map = s.pairings.lock().unwrap();
        let p = map.entry(id.clone()).or_insert_with(|| Pairing {
            created: Instant::now(),
            peers: vec![],
        });
        if p.peers.len() >= 2 || p.created.elapsed() > Duration::from_secs(600) {
            None
        } else {
            p.peers.push(tx);
            Some(p.peers.len() - 1)
        }
    };
    let Some(me) = other else {
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    loop {
        tokio::select! {
            m = rx.recv() => match m {
                Some(text) => if socket.send(Message::Text(text.into())).await.is_err() { break },
                None => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(t))) if t.len() < 1 << 20 => {
                    let map = s.pairings.lock().unwrap();
                    if let Some(peer) = map.get(&id).and_then(|p| p.peers.get(1 - me)) {
                        let _ = peer.send(t.to_string());
                    }
                }
                Some(Ok(_)) => {}
                _ => break,
            }
        }
    }
    // A pairing is single use.
    s.pairings.lock().unwrap().remove(&id);
}

// ------------------------------------------------------------------ the client

/// A device's connection to a sync server.
#[derive(Clone)]
pub struct Client {
    pub base: String,
    pub space: String,
    device: Device,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base: &str, space: &str, device: &Device) -> Result<Self> {
        Ok(Client {
            base: base.trim_end_matches('/').to_owned(),
            space: space.to_owned(),
            device: device.clone(),
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(60))
                .build()?,
        })
    }

    fn signed(&self, method: Method, path: &str, body: Vec<u8>) -> Result<reqwest::RequestBuilder> {
        let time = super::core::Hlc::now_ms().to_string();
        let nonce = hex(&crate::vault::random::<16>()?);
        let path_only = path.split('?').next().unwrap_or(path);
        let msg = signed_message(method.as_str(), path_only, &time, &nonce, &body);
        let sig = b64(&self.device.sign(msg.as_bytes())?);
        let m = reqwest::Method::from_bytes(method.as_str().as_bytes())?;
        Ok(self
            .http
            .request(m, format!("{}{path}", self.base))
            .header("x-spindle-device", &self.device.id)
            .header("x-spindle-time", time)
            .header("x-spindle-nonce", nonce)
            .header("x-spindle-sig", sig)
            .header("content-type", "application/json")
            .body(body))
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let bytes = body
            .map(|b| serde_json::to_vec(&b))
            .transpose()?
            .unwrap_or_default();
        let r = self
            .signed(method, path, bytes)?
            .send()
            .await
            .with_context(|| format!("Couldn't reach the sync server at {}", self.base))?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        ensure!(
            status.is_success(),
            "Sync server: {} ({status})",
            v["error"].as_str().unwrap_or("request failed")
        );
        Ok(v)
    }

    /// Register a new space on the server with the admin token.
    pub async fn create_space(&self, admin: &str) -> Result<()> {
        let r = self
            .http
            .post(format!("{}/v1/spaces", self.base))
            .bearer_auth(admin)
            .json(&json!({"space": self.space, "device": self.device.id}))
            .send()
            .await
            .with_context(|| format!("Couldn't reach the sync server at {}", self.base))?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        ensure!(
            status.is_success(),
            "Sync server: {}",
            v["error"].as_str().unwrap_or("request failed")
        );
        Ok(())
    }

    pub async fn add_device(&self, device: &str) -> Result<()> {
        self.call(
            Method::POST,
            &format!("/v1/spaces/{}/devices", self.space),
            Some(json!({"device": device})),
        )
        .await
        .map(|_| ())
    }

    pub async fn remove_device(&self, device: &str) -> Result<()> {
        self.call(
            Method::DELETE,
            &format!("/v1/spaces/{}/devices/{device}", self.space),
            None,
        )
        .await
        .map(|_| ())
    }

    pub async fn list(&self, since: &str) -> Result<Listing> {
        let v = self
            .call(
                Method::GET,
                &format!("/v1/spaces/{}/records?since={since}", self.space),
                None,
            )
            .await?;
        Ok(serde_json::from_value(v)?)
    }

    pub async fn get(&self, ids: &[String]) -> Result<Vec<Vec<u8>>> {
        let v = self
            .call(
                Method::POST,
                &format!("/v1/spaces/{}/get", self.space),
                Some(json!({"ids": ids})),
            )
            .await?;
        v["records"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r.as_str())
            .map(unb64)
            .collect()
    }

    pub async fn put(&self, records: &[(String, Vec<u8>)]) -> Result<()> {
        let list: Vec<Value> = records
            .iter()
            .map(|(id, b)| json!({"id": id, "data": b64(b)}))
            .collect();
        self.call(
            Method::PUT,
            &format!("/v1/spaces/{}/records", self.space),
            Some(json!({"records": list})),
        )
        .await
        .map(|_| ())
    }

    /// Save an account so a new device can join by signing in. The password stays here:
    /// one key derived from it signs in, another wraps the space key.
    pub async fn set_account(
        &self,
        user: &str,
        password: &str,
        space_key: &super::core::SpaceKeys,
    ) -> Result<()> {
        ensure!(
            password.chars().count() >= 12,
            "Use a password of at least 12 characters"
        );
        let salt_auth = crate::vault::random::<16>()?;
        let salt_wrap = crate::vault::random::<16>()?;
        let auth = crate::vault::derive(password, &salt_auth);
        let wrap_key = crate::vault::derive(password, &salt_wrap);
        let wrapped = b64(&crate::vault::seal(
            &wrap_key,
            b"spindle-account",
            serde_json::to_vec(space_key)?,
        )?);
        self.call(
            Method::PUT,
            &format!("/v1/spaces/{}/account", self.space),
            Some(json!({
                "user": user,
                "salt_auth": b64(&salt_auth),
                "salt_wrap": b64(&salt_wrap),
                "auth_key": b64(&auth),
                "wrapped": wrapped,
            })),
        )
        .await
        .map(|_| ())
    }

    /// Sign in on a new device: returns the space's keys and registers this device.
    pub async fn join_with_account(
        base: &str,
        user: &str,
        password: &str,
        device: &Device,
    ) -> Result<super::core::SpaceKeys> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?;
        let base = base.trim_end_matches('/');
        let salts: Value = http
            .get(format!("{base}/v1/accounts/{}", url_part(user)))
            .send()
            .await
            .with_context(|| format!("Couldn't reach the sync server at {base}"))?
            .json()
            .await?;
        let salt = |k: &str| -> Result<[u8; 16]> {
            unb64(salts[k].as_str().context("Server didn't send salts")?)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid salt"))
        };
        let auth = crate::vault::derive(password, &salt("salt_auth")?);
        let wrap_key = crate::vault::derive(password, &salt("salt_wrap")?);
        let r = http
            .post(format!("{base}/v1/accounts/{}/login", url_part(user)))
            .json(&json!({"auth_key": b64(&auth)}))
            .send()
            .await?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        ensure!(
            status.is_success(),
            "{}",
            v["error"].as_str().unwrap_or("Sign-in failed")
        );
        let keys: super::core::SpaceKeys = serde_json::from_slice(&crate::vault::open(
            &wrap_key,
            b"spindle-account",
            &unb64(v["wrapped"].as_str().unwrap_or(""))?,
        )?)?;
        let space = v["space"].as_str().unwrap_or("");
        ensure!(space == keys.space, "The server sent another space's key");
        let r = http
            .post(format!("{base}/v1/spaces/{space}/enroll"))
            .json(&json!({"token": v["enroll"], "device": device.id}))
            .send()
            .await?;
        ensure!(
            r.status().is_success(),
            "The server didn't register this device"
        );
        Ok(keys)
    }

    /// Open the live channel: notices of new records, and relay frames.
    pub async fn live(
        &self,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let path = format!("/v1/spaces/{}/live", self.space);
        let signed = self.signed(Method::GET, &path, vec![])?.build()?;
        let ws_url = format!("{}{path}", self.base)
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        let mut req = ws_url.into_client_request()?;
        for (k, v) in signed.headers() {
            if k.as_str().starts_with("x-spindle-") {
                req.headers_mut().insert(k.clone(), v.clone());
            }
        }
        let (ws, _) = tokio_tungstenite::connect_async(req).await?;
        Ok(ws)
    }
}

fn url_part(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::core::Kind;
    use super::super::space::{Identity, Space};
    use super::super::transport::{Progress, Transport, sync};
    use super::*;
    use futures_util::StreamExt;

    async fn start(mode: Mode, mailbox: Duration) -> (String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (url, _) = start_in(dir.path(), mode, mailbox).await;
        (url, dir)
    }

    async fn start_in(
        dir: &std::path::Path,
        mode: Mode,
        mailbox: Duration,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let server = Server::new(Config {
            data: dir.into(),
            mode,
            mailbox,
            admin: "admin-token".into(),
        })
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router(server)).await.unwrap() });
        (format!("http://{addr}"), task)
    }

    fn joined(founder: &mut Space, name: &str) -> Space {
        let s = Space::open(
            Identity {
                keys: founder.keys_for(true),
                device: Device::generate(name).unwrap(),
                secrets: true,
            },
            None,
        )
        .unwrap();
        founder.add_member(s.member_entry().unwrap()).unwrap();
        s
    }

    #[tokio::test]
    async fn store_mode_keeps_records_for_a_device_that_was_offline() {
        let (url, _dir) = start(Mode::Store, Duration::ZERO).await;
        let mut a = Space::create("Desktop", None).unwrap();
        let ca = Client::new(&url, &a.id.keys.space, &a.id.device).unwrap();
        ca.create_space("admin-token").await.unwrap();
        // Without the admin token, nobody can make spaces.
        assert!(ca.create_space("wrong").await.is_err());
        let mut b = joined(&mut a, "Laptop");
        ca.add_device(&b.id.device.id).await.unwrap();
        a.put(Kind::Settings, "prefs", serde_json::json!({"theme":"dark"}))
            .unwrap();
        let ta = Transport::Hosted(ca.clone());
        let mut pa = Progress::default();
        sync(&mut a, &ta, &mut pa).await.unwrap();
        // The desktop goes away; the laptop catches up from the server.
        let tb = Transport::Hosted(Client::new(&url, &b.id.keys.space, &b.id.device).unwrap());
        let mut pb = Progress::default();
        let o = sync(&mut b, &tb, &mut pb).await.unwrap();
        assert!(o.received >= 2);
        assert_eq!(b.value(Kind::Settings, "prefs").unwrap()["theme"], "dark");
        // A device the server doesn't know is refused.
        let stranger = Device::generate("Stranger").unwrap();
        let cs = Client::new(&url, &a.id.keys.space, &stranger).unwrap();
        assert!(cs.list("").await.is_err());
    }

    #[tokio::test]
    async fn store_survives_restart_without_writing_plaintext() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let (first, task) = start_in(dir.path(), Mode::Store, Duration::ZERO).await;
        let mut desktop = Space::create("Desktop", None)?;
        let client = Client::new(&first, &desktop.id.keys.space, &desktop.id.device)?;
        client.create_space("admin-token").await?;
        let mut laptop = joined(&mut desktop, "Laptop");
        client.add_device(&laptop.id.device.id).await?;
        let marker = "workspace-content-known-only-to-the-paired-devices";
        desktop.put(Kind::Settings, "fixture", json!({"text": marker}))?;
        sync(&mut desktop, &Transport::Hosted(client), &mut Progress::default()).await?;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let files = dir.path().join(&desktop.id.keys.space).join("records");
        let mut count = 0;
        for bucket in std::fs::read_dir(files)? {
            for entry in std::fs::read_dir(bucket?.path())? {
                count += 1;
                let bytes = std::fs::read(entry?.path())?;
                assert!(!bytes.windows(marker.len()).any(|window| window == marker.as_bytes()));
            }
        }
        assert!(count > 0);
        let (second, task) = start_in(dir.path(), Mode::Store, Duration::ZERO).await;
        let client = Client::new(&second, &laptop.id.keys.space, &laptop.id.device)?;
        sync(&mut laptop, &Transport::Hosted(client), &mut Progress::default()).await?;
        assert_eq!(laptop.value(Kind::Settings, "fixture").unwrap()["text"], marker);
        task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn relay_mode_keeps_only_a_mailbox_and_pushes_live_notices() {
        let (url, dir) = start(Mode::Relay, Duration::from_secs(3600)).await;
        let mut a = Space::create("Desktop", None).unwrap();
        let ca = Client::new(&url, &a.id.keys.space, &a.id.device).unwrap();
        ca.create_space("admin-token").await.unwrap();
        let mut b = joined(&mut a, "Laptop");
        ca.add_device(&b.id.device.id).await.unwrap();
        let cb = Client::new(&url, &b.id.keys.space, &b.id.device).unwrap();
        let mut live = cb.live().await.unwrap();
        a.put(Kind::Settings, "prefs", serde_json::json!({"theme":"dark"}))
            .unwrap();
        sync(
            &mut a,
            &Transport::Hosted(ca.clone()),
            &mut Progress::default(),
        )
        .await
        .unwrap();
        // The laptop hears about it at once.
        let msg = tokio::time::timeout(Duration::from_secs(5), live.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(msg.to_text().unwrap().contains("\"records\""));
        sync(
            &mut b,
            &Transport::Hosted(cb.clone()),
            &mut Progress::default(),
        )
        .await
        .unwrap();
        assert_eq!(b.value(Kind::Settings, "prefs").unwrap()["theme"], "dark");
        // Nothing was written to disk but the space's device list.
        let records = dir.path().join(&a.id.keys.space).join("records");
        assert!(!records.exists());
    }

    #[tokio::test]
    async fn a_new_device_joins_by_signing_in() {
        let (url, _dir) = start(Mode::Both, Duration::from_secs(3600)).await;
        let mut a = Space::create("Desktop", None).unwrap();
        let ca = Client::new(&url, &a.id.keys.space, &a.id.device).unwrap();
        ca.create_space("admin-token").await.unwrap();
        ca.set_account("test-account", "correct horse battery", &a.id.keys)
            .await
            .unwrap();
        a.put(Kind::Skill, "notes", serde_json::json!({"text":"hi"}))
            .unwrap();
        sync(
            &mut a,
            &Transport::Hosted(ca.clone()),
            &mut Progress::default(),
        )
        .await
        .unwrap();

        let lap = Device::generate("Laptop").unwrap();
        assert!(
            Client::join_with_account(&url, "test-account", "wrong password!", &lap)
                .await
                .is_err()
        );
        let keys = Client::join_with_account(&url, "test-account", "correct horse battery", &lap)
            .await
            .unwrap();
        assert_eq!(keys.space, a.id.keys.space);
        let mut b = Space::open(
            Identity {
                keys,
                device: lap.clone(),
                secrets: true,
            },
            None,
        )
        .unwrap();
        let cb = Client::new(&url, &b.id.keys.space, &lap).unwrap();
        sync(&mut b, &Transport::Hosted(cb), &mut Progress::default())
            .await
            .unwrap();
        assert_eq!(b.value(Kind::Skill, "notes").unwrap()["text"], "hi");
        // Too many wrong passwords lock sign-in for a while.
        for _ in 0..5 {
            let _ = Client::join_with_account(&url, "test-account", "nope nope nope", &lap).await;
        }
        let e = Client::join_with_account(&url, "test-account", "correct horse battery", &lap)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("Too many"), "{e}");
    }
}
