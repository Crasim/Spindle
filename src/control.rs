//! The control channel: how the CLI and the MCP server (and so other programs) use the
//! Spindle that owns a workspace. Only one Spindle may open a workspace's state, so other
//! programs go through it.
//!
//! The owner listens on 127.0.0.1 (a random port) and writes the port and a random token to
//! `<data>/control/<workspace>.json`, readable only by the user. Requests are JSON lines that
//! carry the token; each gets one JSON line back. Same on Windows and Linux.

use crate::engine::View;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where a workspace's owner says how to reach it.
pub fn endpoint_file(root: &Path) -> PathBuf {
    let key = crate::tools::hash(root.to_string_lossy().as_bytes());
    crate::runtime::data_dir()
        .join("control")
        .join(format!("{key}.json"))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What the owner shares with control handlers.
pub struct Shared {
    root: PathBuf,
    token: String,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    views: tokio::sync::broadcast::Sender<Arc<View>>,
    latest: Mutex<Option<Arc<View>>>,
    handover: bool,
    /// When a client last asked something, for the headless owner's idle exit.
    pub last_used: AtomicU64,
}

/// Start listening for a workspace's owner. Returns the handle used to publish views.
pub fn serve(
    root: &Path,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
    handover: bool,
) -> Result<Arc<Shared>> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("Couldn't open the control port")?;
    let port = listener.local_addr()?.port();
    let mut bytes = [0u8; 24];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
        .map_err(|_| anyhow::anyhow!("No randomness for the control token"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let file = endpoint_file(root);
    std::fs::create_dir_all(file.parent().unwrap())?;
    let body =
        json!({"port": port, "token": token, "pid": std::process::id(), "root": root}).to_string();
    write_private(&file, body.as_bytes())?;
    let (views, _) = tokio::sync::broadcast::channel(256);
    let shared = Arc::new(Shared {
        root: root.to_owned(),
        token,
        tx,
        views,
        latest: Mutex::new(None),
        handover,
        last_used: AtomicU64::new(now()),
    });
    let s = shared.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let s = s.clone();
            std::thread::spawn(move || {
                let _ = handle(&s, stream);
            });
        }
    });
    Ok(shared)
}

/// Write a file only the user can read.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        // %LOCALAPPDATA% is already private to the user.
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

impl Shared {
    /// Called for every view the engine produces.
    pub fn publish(&self, view: &View) {
        let v = Arc::new(view.clone());
        *self.latest.lock().unwrap() = Some(v.clone());
        let _ = self.views.send(v);
    }

    /// Remove the endpoint file when the owner stops.
    pub fn close(&self) {
        if endpoint_record(&self.root).is_some_and(|(_, token)| token == self.token) {
            let _ = std::fs::remove_file(endpoint_file(&self.root));
        }
    }

    /// The engine's newest view, waiting briefly for the first one after a start.
    pub(crate) fn latest(&self) -> Result<Arc<View>> {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(v) = self.latest.lock().unwrap().clone() {
                return Ok(v);
            }
            ensure!(Instant::now() < until, "Spindle is still starting");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Send an engine request and wait for its reply (see engine `/ext`).
    fn engine(&self, request: Value) -> Result<Value> {
        let mut views = self.views.subscribe();
        let mut tag = [0u8; 8];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut tag).ok();
        let tag: String = tag.iter().map(|b| format!("{b:02x}")).collect();
        self.tx
            .send(format!("/ext {tag} {request}"))
            .map_err(|_| anyhow::anyhow!("Spindle stopped"))?;
        let prefix = format!("ext:{tag}:");
        let until = Instant::now() + Duration::from_secs(15);
        while Instant::now() < until {
            match views.try_recv() {
                Ok(v) => {
                    if let Some(body) = v.notice.strip_prefix(&prefix) {
                        let reply: Value = serde_json::from_str(body)?;
                        if let Some(e) = reply.get("error").and_then(Value::as_str) {
                            bail!("{e}");
                        }
                        return Ok(reply);
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(30))
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => bail!("Spindle stopped"),
            }
        }
        bail!("Spindle didn't answer")
    }

    /// Wait until a chat has nothing running or queued, or is waiting for approval.
    fn wait(&self, chat: u64, seconds: u64) -> Result<Value> {
        let until = Instant::now() + Duration::from_secs(seconds.min(600));
        // Give the engine a moment to pick the task up.
        std::thread::sleep(Duration::from_millis(400));
        loop {
            let v = self.latest()?;
            let s = status(&v, &self.root, chat)?;
            let state = s["state"].as_str().unwrap_or_default();
            if state == "idle" || state == "waiting_for_approval" || Instant::now() >= until {
                return Ok(s);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}

fn handle(s: &Shared, stream: TcpStream) -> Result<()> {
    let mut out = stream.try_clone()?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let mut closing = false;
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(req) if req["token"].as_str() == Some(s.token.as_str()) => {
                s.last_used.store(now(), Ordering::Relaxed);
                match dispatch(s, &req) {
                    Ok(reply) => {
                        closing = matches!(req["op"].as_str(), Some("exit" | "stop"));
                        reply
                    }
                    Err(e) => json!({"error": format!("{e:#}")}),
                }
            }
            Ok(_) => json!({"error": "Not authorized"}),
            Err(e) => json!({"error": format!("Invalid request: {e}")}),
        };
        let written = writeln!(out, "{reply}");
        // Acknowledge first so an idle owner can exit without losing the reply.
        if closing {
            let _ = s.tx.send("/exit".into());
            return written.map_err(Into::into);
        }
        written?;
    }
    Ok(())
}

fn dispatch(s: &Shared, req: &Value) -> Result<Value> {
    let op = req["op"].as_str().unwrap_or_default();
    let chat_arg = || req["chat"].as_u64().context("Which chat? (chat id)");
    let wait = req["wait"].as_u64().unwrap_or(0);
    match op {
        "ping" => Ok(json!({"ok": true, "version": crate::version(), "root": s.root})),
        "sync" => {
            let line = req["line"].as_str().context("Give the sync command")?;
            s.engine(json!({"op": "command", "text": line}))
        }
        "ask" => {
            let text = req["text"].as_str().context("Give the prompt")?;
            let new = s.engine(
                json!({"op": "new", "project": req.get("project"), "model": req.get("model")}),
            )?;
            let chat = new["chat"].as_u64().context("No chat created")?;
            s.engine(json!({"op": "send", "chat": chat, "text": text, "goal": req["goal"].as_bool().unwrap_or(false)}))?;
            if wait > 0 {
                s.wait(chat, wait)
            } else {
                status(&*s.latest()?, &s.root, chat)
            }
        }
        "send" => {
            let chat = chat_arg()?;
            let text = req["text"].as_str().context("Give the message")?;
            s.engine(json!({"op": "send", "chat": chat, "text": text, "goal": req["goal"].as_bool().unwrap_or(false)}))?;
            if wait > 0 {
                s.wait(chat, wait)
            } else {
                status(&*s.latest()?, &s.root, chat)
            }
        }
        "status" => {
            let v = s.latest()?;
            match req["chat"].as_u64() {
                Some(chat) if wait > 0 => s.wait(chat, wait),
                Some(chat) => status(&v, &s.root, chat),
                None => Ok(json!({
                    "busy": v.busy,
                    "job": v.job,
                    "queued": v.state.queue.len(),
                    "waiting_for_approval": v.state.requests.iter().filter(|r| !r.approved).map(|r| json!({"id": r.id, "chat": r.chat, "what": r.call.call.summary()})).collect::<Vec<_>>(),
                })),
            }
        }
        "chats" => {
            let v = s.latest()?;
            let st = &v.state;
            let limit = req["limit"].as_u64().unwrap_or(30) as usize;
            let project = req["project"].as_str().map(PathBuf::from);
            let mut rows: Vec<(u64, Value)> = std::iter::once(st.chat_id)
                .chain(st.chats.keys().copied())
                .filter_map(|id| {
                    let (t, h) = st.thread_data(id)?;
                    if h.is_empty() && !t.started {
                        return None;
                    }
                    if project.as_ref().is_some_and(|p| t.project.as_ref() != Some(p)) {
                        return None;
                    }
                    let state = status(&v, &s.root, id).ok()?["state"].clone();
                    Some((id, json!({
                        "chat": id,
                        "title": t.label(h).chars().take(80).collect::<String>(),
                        "model": if id == st.chat_id { st.model.clone() } else { t.model.clone() },
                        "project": t.project,
                        "messages": h.len(),
                        "state": state,
                    })))
                })
                .collect();
            rows.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
            Ok(json!(
                rows.into_iter()
                    .take(limit)
                    .map(|(_, r)| r)
                    .collect::<Vec<_>>()
            ))
        }
        "show" => {
            let v = s.latest()?;
            Ok(
                json!({"text": transcript(&v, chat_arg()?, req["full"].as_bool().unwrap_or(false))?}),
            )
        }
        "file" => {
            let v = s.latest()?;
            let name = req["name"].as_str().context("Which file?")?;
            file(&v, &s.root, chat_arg()?, name)
        }
        "projects" => Ok(json!(
            crate::projects::library(None)?
                .into_iter()
                .map(|p| json!({"name": p.name, "path": p.path, "description": p.description}))
                .collect::<Vec<_>>()
        )),
        "models" => {
            let v = s.latest()?;
            Ok(json!(
                v.models
                    .iter()
                    .map(|m| json!({
                        "model": m,
                        "size_b": crate::ollama::known_size(m),
                        "vision": v.vision.contains(m),
                        "current": *m == v.state.model,
                    }))
                    .collect::<Vec<_>>()
            ))
        }
        // Approvals come only from someone at a keyboard: the CLI refuses without a terminal,
        // and the MCP server has no approve tool.
        "approve" => {
            let id = req["id"].as_u64().context("Which request? (id)")?;
            let level = req["level"].as_str().unwrap_or("once");
            ensure!(
                matches!(level, "once" | "conversation" | "project"),
                "Level is once, conversation or project"
            );
            s.tx.send(format!("/approve {id} {level}"))
                .map_err(|_| anyhow::anyhow!("Spindle stopped"))?;
            Ok(json!({"ok": true}))
        }
        "deny" => {
            let id = req["id"].as_u64().context("Which request? (id)")?;
            s.tx.send(format!("/deny {id}"))
                .map_err(|_| anyhow::anyhow!("Spindle stopped"))?;
            Ok(json!({"ok": true}))
        }
        // A full exit (the tray's Exit, `spindle service stop`): running work is held.
        "exit" => Ok(json!({"ok": true})),
        // Another window may take over an idle service, never an existing window or terminal.
        "stop" => {
            ensure!(s.handover, "This workspace is already open");
            ensure!(idle(s), "Spindle is busy; try again when its work is done");
            Ok(json!({"ok": true}))
        }
        other => bail!("Unknown operation {other}"),
    }
}

/// Ask a windowless Spindle that owns `root` to stop, and wait for it to let go.
pub fn hand_over(root: &Path) -> Result<()> {
    exit_owner(root, "stop")
}

/// No work running, queued or waiting anywhere: the windowless owner may stop.
pub fn idle(s: &Shared) -> bool {
    s.latest().is_ok_and(|v| {
        !v.busy && v.job.is_none() && v.state.queue.is_empty() && v.state.requests.is_empty()
    })
}

/// Where a chat's own files are (chats outside projects); project chats work in the project.
fn chat_folder(v: &View, root: &Path, chat: u64) -> Option<PathBuf> {
    let (t, _) = v.state.thread_data(chat)?;
    match &t.project {
        Some(p) => Some(p.clone()),
        None => crate::artifacts::folder(&v.state.files_root(root, chat), chat).ok(),
    }
}

/// A chat's state for another program: what it's doing, what it's waiting for, its last
/// answer and the files it made.
pub fn status(v: &View, root: &Path, chat: u64) -> Result<Value> {
    let st = &v.state;
    let (t, h) = st.thread_data(chat).context("No such chat")?;
    let mine = |o: Option<u64>| o.unwrap_or(st.chat_id) == chat;
    let front = st.queue.front().is_some_and(|q| mine(q.origin));
    let queued = st.queue.iter().any(|q| mine(q.origin));
    let waiting: Vec<Value> = st
        .requests
        .iter()
        .filter(|r| r.chat == chat && !r.approved)
        .map(|r| json!({"id": r.id, "what": r.call.call.summary(), "risk": r.risk.label()}))
        .collect();
    let running_tool = st.requests.iter().any(|r| r.chat == chat && r.approved);
    let state = if !waiting.is_empty() {
        "waiting_for_approval"
    } else if (front && v.busy) || running_tool || (front && v.job.is_some()) {
        "running"
    } else if queued {
        if st.held { "held" } else { "queued" }
    } else {
        "idle"
    };
    // The steps since the user's last message, and the last answer.
    let last_user = h
        .iter()
        .enumerate()
        .rev()
        .find(|(i, m)| m.role == "user" && !t.tool_results.contains(i))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let steps: Vec<String> = (last_user..h.len())
        .filter_map(|i| t.turns.get(&i).and_then(|turn| turn.tool.as_ref()))
        .map(|n| format!("{} · {}", n.summary, n.status))
        .collect();
    let answer = h
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let mut files: Vec<String> = t
        .turns
        .values()
        .flat_map(|turn| turn.files.clone())
        .collect();
    files.sort();
    files.dedup();
    Ok(json!({
        "chat": chat,
        "title": t.label(h).chars().take(80).collect::<String>(),
        "state": state,
        "waiting_for_approval": waiting,
        "steps": steps,
        "answer": answer,
        "files": files,
        "folder": chat_folder(v, root, chat),
        "goal_done": t.goal_done,
    }))
}

/// A chat as text, with tool steps folded to one line each unless `full`.
pub fn transcript(v: &View, chat: u64, full: bool) -> Result<String> {
    let (t, h) = v.state.thread_data(chat).context("No such chat")?;
    let mut out = format!("# {}\n", t.label(h));
    for (i, m) in h.iter().enumerate() {
        if m.role == "user" && t.tool_results.contains(&i) {
            if full {
                out.push_str(&format!("\n> {}\n", m.content.replace('\n', "\n> ")));
            }
            continue;
        }
        let who = if m.role == "user" { "You" } else { "Spindle" };
        if !m.content.trim().is_empty() {
            out.push_str(&format!("\n**{who}:** {}\n", m.content.trim()));
        }
        if let Some(n) = t.turns.get(&i).and_then(|turn| turn.tool.as_ref()) {
            out.push_str(&format!("  · {} · {}\n", n.summary, n.status));
        }
    }
    Ok(out)
}

fn file(v: &View, root: &Path, chat: u64, name: &str) -> Result<Value> {
    ensure!(
        !name.contains("..")
            && !name.starts_with('/')
            && !name.contains(':')
            && !name.starts_with('\\'),
        "Give a file name inside the chat's folder"
    );
    let folder = chat_folder(v, root, chat).context("No folder for this chat")?;
    let path = folder.join(name);
    let canon = path.canonicalize().context("No such file")?;
    ensure!(
        canon.starts_with(folder.canonicalize()?),
        "Outside the chat's folder"
    );
    let bytes = std::fs::read(&canon)?;
    ensure!(
        bytes.len() <= 8 * 1024 * 1024,
        "The file is larger than 8 MB"
    );
    Ok(match String::from_utf8(bytes) {
        Ok(text) => json!({"name": name, "text": text}),
        Err(e) => {
            use base64::Engine as _;
            json!({"name": name, "base64": base64::engine::general_purpose::STANDARD.encode(e.into_bytes())})
        }
    })
}

// ---- client side ----

/// Ask the Spindle that owns `root`. Starts a windowless one if none is running.
pub fn request(root: &Path, mut req: Value) -> Result<Value> {
    let (port, token) = match endpoint(root) {
        Some(e) => e,
        None => {
            start_owner(root)?;
            endpoint(root).context("Spindle didn't start")?
        }
    };
    req["token"] = json!(token);
    let attempt = |port: u16, req: &Value| -> Result<Value> {
        let mut stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(Duration::from_secs(660)))?;
        writeln!(stream, "{req}")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        let reply: Value = serde_json::from_str(&line).context("No answer from Spindle")?;
        if let Some(e) = reply.get("error").and_then(Value::as_str) {
            bail!("{e}");
        }
        Ok(reply)
    };
    match attempt(port, &req) {
        Err(e) if e.downcast_ref::<std::io::Error>().is_some() => {
            // A stale endpoint (the owner stopped without cleaning up): start a new one.
            let _ = std::fs::remove_file(endpoint_file(root));
            start_owner(root)?;
            let (port, token) = endpoint(root).context("Spindle didn't start")?;
            req["token"] = json!(token);
            attempt(port, &req)
        }
        other => other,
    }
}

/// Whether something owns `root` and answers.
pub fn ping(root: &Path) -> bool {
    endpoint(root).is_some()
}

/// Request shutdown and wait for the same owner to release the endpoint.
pub fn exit_owner(root: &Path, op: &str) -> Result<()> {
    let Some((port, token)) = endpoint(root) else {
        return Ok(());
    };
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    writeln!(stream, "{}", json!({"token": token, "op": op}))?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let reply: Value = serde_json::from_str(&line).context("No answer from Spindle")?;
    if let Some(e) = reply.get("error").and_then(Value::as_str) {
        bail!("{e}");
    }
    let until = Instant::now() + Duration::from_secs(30);
    while endpoint_record(root).is_some_and(|(_, current)| current == token) {
        ensure!(Instant::now() < until, "Spindle is still shutting down");
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn endpoint_record(root: &Path) -> Option<(u16, String)> {
    let v: Value = serde_json::from_slice(&std::fs::read(endpoint_file(root)).ok()?).ok()?;
    Some((u16::try_from(v["port"].as_u64()?).ok()?, v["token"].as_str()?.to_owned()))
}

fn endpoint(root: &Path) -> Option<(u16, String)> {
    let (port, token) = endpoint_record(root)?;
    // Only if something is listening there.
    TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    )
    .ok()?;
    Some((port, token))
}

/// Start `spindle serve` for a workspace in the background and wait for it to listen.
fn start_owner(root: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("serve")
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW
        cmd.creation_flags(0x0000_0008 | 0x0000_0200 | 0x0800_0000);
    }
    cmd.spawn().context("Couldn't start Spindle")?;
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if endpoint(root).is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    bail!("Spindle didn't start within 20 seconds (is the workspace open somewhere else?)")
}
