//! Peer-addressed build and script requests carried by the existing encrypted space.
//! Only the original sender may publish a request and only its chosen host may answer.
use crate::{
    state::State,
    sync::{
        adapter::SyncState,
        core::{self, Kind},
        space::Space,
    },
    tools::Call,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

const PREFIX: &str = "remote-job-v1/";
const TTL: i64 = 24 * 3600;
const MAX_SOURCE: usize = 256 * 1024;
const MAX_OUTPUT: usize = 7 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    from: String,
    to: String,
    created: i64,
    nonce: String,
    args: Vec<String>,
    entry: String,
    files: BTreeMap<String, String>,
    operation: Operation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum Operation {
    Inventory,
    Tool { id: String, command: String },
    Build,
    Python,
    Apk { package: String, activity: String },
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Receipt {
    ok: bool,
    text: String,
    extension: String,
    bytes: String,
}

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Remote {
    pub allowed: BTreeSet<String>,
    sent: BTreeMap<String, Request>,
    answers: BTreeMap<String, Receipt>,
    claims: BTreeMap<String, i64>,
    #[serde(skip)]
    running: Option<String>,
    received: BTreeMap<String, Receipt>,
    waiting: BTreeMap<String, (u64, String, u32, u64)>,
    #[serde(skip)]
    pending: BTreeMap<String, Request>,
    #[serde(skip)]
    pub platforms: BTreeMap<String, String>,
}
fn signature(r: &Request) -> Result<String> {
    Ok(core::hex(&core::sha256(&serde_json::to_vec(r)?)))
}
fn fresh(r: &Request, now: i64) -> bool {
    r.created >= now - TTL && r.created <= now + 300
}
fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
impl Request {
    fn validate(&self) -> Result<()> {
        ensure!(self.files.len() <= 16, "Select up to 16 source files");
        match &self.operation {
            Operation::Inventory => ensure!(
                self.entry.is_empty() && self.files.is_empty() && self.args.is_empty(),
                "Inventory takes no files or arguments"
            ),
            Operation::Tool { id, command } => ensure!(
                self.entry.is_empty()
                    && id.len() == 24
                    && id.bytes().all(|b| b.is_ascii_hexdigit())
                    && !command.is_empty()
                    && command.len() <= 100,
                "Select a tool ID and command from the target inventory"
            ),
            _ => ensure!(
                self.files.contains_key(&self.entry),
                "Select a source entry"
            ),
        }
        ensure!(
            self.files.values().map(String::len).sum::<usize>() <= MAX_SOURCE,
            "Remote source exceeds 256 KiB"
        );
        ensure!(
            self.args.len() <= 16
                && self
                    .args
                    .iter()
                    .all(|s| s.len() <= 200 && !s.contains('\0')),
            "Pass at most 16 short arguments"
        );
        for path in self.files.keys() {
            ensure!(
                crate::native::stage_path(path)? == *path,
                "Use portable relative source paths"
            );
            let ext = path.rsplit('.').next().unwrap_or("");
            ensure!(
                match self.operation {
                    Operation::Inventory => false,
                    Operation::Tool { .. } => true,
                    Operation::Build if self.entry.ends_with(".rs") => ext == "rs",
                    Operation::Build =>
                        matches!(ext, "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hh" | "hxx"),
                    Operation::Python => ext == "py",
                    Operation::Apk { .. } => ext == "java",
                },
                "File type does not match the remote operation"
            );
        }
        Ok(())
    }
}

pub fn export(sync: &SyncState, space: &mut Space) -> Result<usize> {
    let me = &sync.identity.device.id;
    let mut count = 0;
    let mut put = |key: String, v: serde_json::Value| -> Result<()> {
        count += usize::from(
            space
                .put(Kind::Settings, &format!("{PREFIX}{key}"), v)?
                .is_some(),
        );
        Ok(())
    };
    put(
        format!("host/{me}"),
        serde_json::json!(std::env::consts::OS),
    )?;
    for (id, r) in &sync.remote.sent {
        if fresh(r, crate::automation::now()) {
            put(format!("request/{id}"), serde_json::to_value(r)?)?;
        }
    }
    for (id, r) in &sync.remote.answers {
        put(format!("answer/{id}"), serde_json::to_value(r)?)?;
    }
    Ok(count)
}

pub fn import(sync: &mut SyncState, space: &Space) -> Result<usize> {
    let me = &sync.identity.device.id;
    let remote = &mut sync.remote;
    remote.pending.clear();
    remote.platforms.clear();
    remote.allowed.retain(|id| space.is_member(id));
    let mut count = 0;
    let now = crate::automation::now();
    for (_, e, b) in space.store.records.iter().filter_map(|(rid, e)| {
        if e.kind != Kind::Settings {
            return None;
        }
        Some((rid, e, space.store.bodies.get(rid)?))
    }) {
        let Some(key) = b.id.strip_prefix(PREFIX) else {
            continue;
        };
        if b.deleted || !space.is_member(&e.device) {
            continue;
        }
        if let Some(host) = key.strip_prefix("host/") {
            if host == e.device
                && let Some(os) = b
                    .value
                    .as_str()
                    .filter(|v| matches!(*v, "windows" | "linux" | "android" | "macos"))
            {
                remote.platforms.insert(host.into(), os.into());
            }
        } else if let Some(id) = key.strip_prefix("request/") {
            if !valid_id(id)
                || !remote.allowed.contains(&e.device)
                || remote.answers.contains_key(id)
                || remote.pending.len() >= 8
            {
                continue;
            }
            let Ok(r) = serde_json::from_value::<Request>(b.value.clone()) else {
                continue;
            };
            if r.from == e.device
                && r.to == *me
                && fresh(&r, now)
                && r.validate().is_ok()
                && signature(&r)? == id
            {
                if remote.claims.contains_key(id) {
                    if remote.running.as_deref() != Some(id) {
                        remote.answers.insert(
                            id.into(),
                            Receipt {
                                text: "Host interrupted before a result was saved".into(),
                                ..Default::default()
                            },
                        );
                    }
                } else {
                    remote.pending.insert(id.into(), r);
                }
            }
        } else if let Some(id) = key.strip_prefix("answer/") {
            if remote.received.contains_key(id)
                || !remote.sent.get(id).is_some_and(|r| r.to == e.device)
            {
                continue;
            }
            let Ok(reply) = serde_json::from_value::<Receipt>(b.value.clone()) else {
                continue;
            };
            if reply.text.len() <= 12000
                && reply.bytes.len() <= MAX_OUTPUT.div_ceil(3) * 4
                && matches!(reply.extension.as_str(), "" | "exe" | "elf" | "apk")
            {
                remote.received.insert(id.into(), reply);
                count += 1;
            }
        }
    }
    Ok(count)
}

pub fn command(state: &mut State, root: &Path, args: &str) -> Result<String> {
    let mut words = args.split_whitespace();
    let op = words.next().unwrap_or("status");
    let devices = crate::sync::live::devices(state);
    let origin = crate::engine::tool_folder(state, state.chat_id, Some(root))
        .context("Open a conversation first")?
        .0;
    let sync = state
        .space
        .as_mut()
        .context("Pair devices in Settings > Sync first")?;
    if op == "devices" {
        return Ok(devices
            .into_iter()
            .filter(|(_, _, _, removed)| !*removed)
            .map(|(id, name, _, _)| {
                format!(
                    "{name}  {}  {id}",
                    sync.remote
                        .platforms
                        .get(&id)
                        .map(String::as_str)
                        .unwrap_or("unknown OS")
                )
            })
            .collect::<Vec<_>>()
            .join("\n"));
    }
    if op == "status" {
        return Ok(sync
            .remote
            .sent
            .iter()
            .map(|(id, r)| {
                let status = sync
                    .remote
                    .received
                    .get(id)
                    .map(|v| format!("{}: {}", if v.ok { "Finished" } else { "Failed" }, v.text))
                    .unwrap_or_else(|| {
                        if fresh(r, crate::automation::now()) {
                            "Waiting for host".into()
                        } else {
                            "Expired".into()
                        }
                    });
                format!("{id}  {}  {status}", r.entry)
            })
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let who=words.next().context("Use /remote devices, status, inventory DEVICE, tool DEVICE ID COMMAND [INPUTS] -- [ARGS], build DEVICE ENTRY [FILES], run DEVICE SCRIPT [FILES], apk DEVICE PACKAGE ACTIVITY ENTRY [FILES], allow DEVICE, deny DEVICE, or save JOB")?;
    if op == "save" {
        let id = sync
            .remote
            .sent
            .keys()
            .filter(|id| id.starts_with(who))
            .collect::<Vec<_>>();
        ensure!(id.len() == 1, "Choose a unique job ID from /remote status");
        let id = id[0];
        let receipt = sync
            .remote
            .received
            .get(id)
            .context("The host has not returned a result")?;
        ensure!(
            receipt.ok
                && !receipt.bytes.is_empty()
                && matches!(receipt.extension.as_str(), "apk" | "exe" | "elf"),
            "This job has no executable output"
        );
        let bytes = core::unb64(&receipt.bytes)?;
        #[cfg(target_os = "android")]
        return crate::android::download_apk(
            &format!("remote-{}.{}", &id[..16], receipt.extension),
            &bytes,
        );
        #[cfg(not(target_os = "android"))]
        {
            let mut tmp = tempfile::NamedTempFile::new_in(&origin)?;
            tmp.write_all(&bytes)?;
            let path = origin.join(format!("remote-{}.{}", &id[..16], receipt.extension));
            tmp.persist_noclobber(&path)?;
            return Ok(format!("Saved {}", crate::projects::display_path(&path)));
        }
    }
    let found = devices
        .iter()
        .filter(|(id, name, me, removed)| !me && !removed && (id.starts_with(who) || name == who))
        .collect::<Vec<_>>();
    ensure!(
        found.len() == 1,
        "Choose a unique active peer ID from /remote devices"
    );
    let to = found[0].0.clone();
    if op == "allow" {
        sync.remote.allowed.insert(to);
        return Ok("Sandboxed jobs from this peer enabled on this device".into());
    }
    if op == "deny" {
        sync.remote.allowed.remove(&to);
        sync.remote.pending.retain(|_, r| r.from != to);
        return Ok("Future jobs from this peer disabled".into());
    }
    ensure!(!sync.paused, "Resume sync before sending a job");
    let operation = match op {
        "inventory" => Operation::Inventory,
        "tool" => Operation::Tool {
            id: words.next().context("Tool ID required")?.into(),
            command: words.next().context("Tool command required")?.into(),
        },
        "build" => Operation::Build,
        "run" => Operation::Python,
        "apk" => Operation::Apk {
            package: words.next().context("APK package required")?.into(),
            activity: words.next().context("Full Activity class required")?.into(),
        },
        _ => bail!("Unknown remote operation"),
    };
    let entry = if matches!(operation, Operation::Inventory | Operation::Tool { .. }) {
        String::new()
    } else {
        words
            .next()
            .context("Select a relative source file")?
            .to_owned()
    };
    let rest: Vec<_> = words.collect();
    let split = rest.iter().position(|s| *s == "--").unwrap_or(rest.len());
    let args = rest
        .iter()
        .skip(split + 1)
        .map(|s| (*s).to_owned())
        .collect();
    let r = snapshot(
        sync,
        &origin,
        to,
        entry,
        &rest[..split]
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<Vec<_>>(),
        operation,
        args,
    )?;
    let id = signature(&r)?;
    sync.remote.sent.insert(id.clone(), r);
    sync.wanted = Some(crate::sync::adapter::Want::Now);
    Ok(format!("Queued remote job {id}"))
}

fn snapshot(
    sync: &SyncState,
    origin: &Path,
    to: String,
    entry: String,
    inputs: &[String],
    operation: Operation,
    args: Vec<String>,
) -> Result<Request> {
    let mut files = BTreeMap::new();
    let mut remaining = MAX_SOURCE;
    for name in std::iter::once(entry.as_str())
        .filter(|p| !p.is_empty())
        .chain(inputs.iter().map(String::as_str))
    {
        ensure!(
            files.len() < 16 && crate::native::stage_path(name)? == name,
            "Select up to 16 relative source files"
        );
        let (path, inside) = crate::tools::resolve(&origin, name)?;
        ensure!(
            inside && path.is_file(),
            "Source must be inside this conversation's workspace"
        );
        let mut data = String::new();
        fs::File::open(path)?
            .take(remaining as u64 + 1)
            .read_to_string(&mut data)?;
        ensure!(data.len() <= remaining, "Remote source exceeds 256 KiB");
        remaining -= data.len();
        ensure!(
            files.insert(name.into(), data).is_none(),
            "Duplicate source file"
        );
    }
    let r = Request {
        args,
        from: sync.identity.device.id.clone(),
        to,
        created: crate::automation::now(),
        nonce: core::hex(&crate::vault::random::<16>()?),
        entry,
        files,
        operation,
    };
    r.validate()?;
    Ok(r)
}

pub fn snapshot_call(state: &State, root: &Path, call: &Call) -> Result<(Request, String)> {
    let Call::RemoteJob {
        device,
        operation,
        path,
        inputs,
        args,
        package,
        activity,
        tool,
        command,
    } = call
    else {
        bail!("Expected a remote job")
    };
    let sync = state
        .space
        .as_ref()
        .context("Set up sync and pair a destination device in Settings › Sync before requesting remote work.")?;
    ensure!(!sync.paused, "Sync is paused. Resume it in Settings › Sync before requesting remote work.");
    ensure!(
        sync.remote.platforms.contains_key(device) && device != &sync.identity.device.id,
        "Choose a different device from the paired destinations in Settings › Sync."
    );
    let op = match operation.as_str() {
        "inventory" => Operation::Inventory,
        "tool" => Operation::Tool {
            id: tool.clone(),
            command: command.clone(),
        },
        "build" => Operation::Build,
        "run" => Operation::Python,
        "apk" => Operation::Apk {
            package: package.clone(),
            activity: activity.clone(),
        },
        _ => bail!("Choose inventory, tool, build, run or apk"),
    };
    let req = snapshot(
        sync,
        root,
        device.clone(),
        path.clone(),
        inputs,
        op,
        args.clone(),
    )?;
    let preview = format!(
        "Send these source files to the encrypted sync pool for {} on {} ({}). Pool members can read the inputs and output.\n{}\nArguments: {}",
        if operation == "tool" {
            format!("tool {tool} command {command}")
        } else {
            operation.clone()
        },
        device,
        &sync.remote.platforms[device],
        req.files.keys().cloned().collect::<Vec<_>>().join("\n"),
        serde_json::to_string(args)?
    );
    Ok((req, preview))
}

pub fn submit_call(state: &mut State, p: &crate::state::Pending, req: Request) -> Result<String> {
    let sync = state
        .space
        .as_mut()
        .context("This workspace left the sync pool. Reconnect it in Settings › Sync.")?;
    ensure!(!sync.paused, "Sync is paused. Resume it in Settings › Sync before sending the job.");
    ensure!(
        p.approved
            && p.risk == crate::policy::Risk::Native
            && req.from == sync.identity.device.id
            && sync.remote.platforms.contains_key(&req.to)
            && fresh(&req, crate::automation::now()),
        "Remote approval expired or target unavailable"
    );
    let id = signature(&req)?;
    sync.remote.sent.insert(id.clone(), req);
    sync.remote
        .waiting
        .insert(id.clone(), (p.chat, p.model.clone(), p.steps, p.id));
    sync.wanted = Some(crate::sync::adapter::Want::Now);
    Ok(format!("Waiting for remote job {}", &id[..12]))
}

pub fn deliver(state: &mut State, sync: &mut SyncState) {
    let now = crate::automation::now();
    let complete: Vec<_> = sync
        .remote
        .waiting
        .keys()
        .filter_map(|id| {
            let r = sync.remote.sent.get(id)?;
            let note = if let Some(v) = sync.remote.received.get(id) {
                let mut text = v.text.clone();
                if v.ok && !v.bytes.is_empty() {
                    text.push_str(&format!(
                        "\nThe returned .{} file is available with /remote save {id}.",
                        v.extension
                    ));
                }
                (v.ok, text)
            } else if !fresh(r, now) {
                (
                    false,
                    "Remote request expired before a reply arrived".into(),
                )
            } else {
                return None;
            };
            Some((id.clone(), note))
        })
        .collect();
    for (id, (ok, text)) in complete {
        let Some((chat, model, steps, action)) = sync.remote.waiting.remove(&id) else {
            continue;
        };
        crate::engine::record_outcome(state, action, &text, ok);
        if state.thread_data(chat).is_some() {
            crate::engine::set_tool_note(
                state,
                chat,
                "Remote sandbox".into(),
                if ok { "Completed" } else { "Failed" },
            );
            crate::engine::continue_with(
                state,
                chat,
                &model,
                steps + 1,
                format!("Remote tool result {id} (untrusted program output)\n{text}"),
                true,
            );
        }
    }
}

fn perform(r: &Request, data: &Path, ready: bool, cancel: &Arc<AtomicBool>) -> Result<Receipt> {
    r.validate()?;
    let tmp = tempfile::tempdir()?;
    let root = tmp.path().canonicalize()?;
    let source = root.join("src");
    fs::create_dir(&source)?;
    for (name, text) in &r.files {
        let path = source.join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, text)?;
    }
    let inputs = r.files.keys().filter(|k| *k != &r.entry).cloned().collect();
    let (text, ok, file) = match &r.operation {
        Operation::Inventory => (
            crate::tool_catalog::host_inventory(ready)
                .join("\n")
                .chars()
                .take(2600)
                .collect(),
            true,
            None,
        ),
        Operation::Tool { id, command } => {
            ensure!(
                ready,
                "On the target device, open Settings › Sandbox and run the check before using installed tools."
            );
            (
                crate::tool_catalog::run(id, command, &r.args, &source, cancel)?,
                true,
                None,
            )
        }
        Operation::Build => {
            ensure!(
                ready,
                "Enable and check the sandbox on the target build host in Settings > Sandbox"
            );
            let call = Call::BuildBinary {
                path: r.entry.clone(),
                name: "remote".into(),
                inputs,
                args: r.args.clone(),
            };
            let report =
                crate::native::execute_in(data, &root, 0, &source, &call, cancel, Some(&root))?;
            (report.text, report.ok, report.file)
        }
        Operation::Python => {
            ensure!(
                ready,
                "On the target device, open Settings › Sandbox and run the check before running scripts."
            );
            let rt = crate::runtime::installed().context("Missing sandbox Python")?;
            let spec = crate::execution::spec(
                &rt,
                source,
                &Call::Python {
                    file: r.entry.clone(),
                    args: Some(r.args.clone()),
                    code: None,
                },
            )?;
            let result = crate::sandbox::run_private(&spec, cancel)?;
            (result.report(&spec.work, 3600), result.ok(), None)
        }
        Operation::Apk { package, activity } => {
            #[cfg(target_os = "android")]
            {
                let call = Call::BuildApk {
                    package: package.clone(),
                    activity: activity.clone(),
                    label: "Remote build".into(),
                    sources: r.files.keys().cloned().collect(),
                };
                let name = crate::android_apk::build(&call, &source, &root, 1, cancel)?;
                (
                    "Android builder signed and verified the APK".into(),
                    true,
                    Some(root.join(name)),
                )
            }
            #[cfg(not(target_os = "android"))]
            {
                let _ = (package, activity);
                bail!("Choose an Android device for the on-device Java APK builder");
            }
        }
    };
    let mut reply = Receipt {
        ok,
        text,
        ..Default::default()
    };
    if let Some(path) = file {
        ensure!(
            fs::metadata(&path)?.len() <= MAX_OUTPUT as u64,
            "Build passed but exceeds the 7 MiB sync transfer limit"
        );
        reply.extension = path
            .extension()
            .and_then(|s| s.to_str())
            .context("Missing output extension")?
            .into();
        reply.bytes = core::b64(&fs::read(path)?);
    }
    Ok(reply)
}

impl crate::engine::Engine {
    pub(crate) fn start_remote(&mut self) -> Result<()> {
        if self.jobs.running.is_some() {
            return Ok(());
        }
        let ready = crate::execution::ready(&self.state);
        let Some(sync) = self.state.space.as_mut().filter(|s| !s.paused) else {
            return Ok(());
        };
        let Some((id, r)) = sync.remote.pending.pop_first() else {
            return Ok(());
        };
        if !sync.remote.allowed.contains(&r.from)
            || !fresh(&r, crate::automation::now())
            || sync.remote.answers.contains_key(&id)
        {
            return Ok(());
        }
        let space = sync.identity.keys.space.clone();
        // Persist the claim before launching. A crash cannot cause a second execution.
        sync.remote.claims.insert(id.clone(), r.created);
        sync.remote.running = Some(id.clone());
        if let Some(store) = &self.store {
            store.save(&self.state)?;
        }
        self.jobs.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.jobs.cancel.clone();
        self.jobs.running = Some(format!("Remote job · {}", r.entry));
        self.jobs.started =
            Some(tokio::task::spawn_blocking(move || {
                let result = perform(&r, &crate::runtime::data_dir(), ready, &cancel)
                    .unwrap_or_else(|e| Receipt {
                        text: format!("{e:#}"),
                        ..Default::default()
                    });
                crate::execution::JobDone::Remote { space, id, result }
            }));
        Ok(())
    }
}
pub fn finish(state: &mut State, space: &str, id: String, result: Receipt) -> String {
    if let Some(s) = state
        .space
        .as_mut()
        .filter(|s| s.identity.keys.space == space)
    {
        let notice = format!(
            "Remote job {} {}",
            &id[..12],
            if result.ok { "finished" } else { "failed" }
        );
        s.remote.running = None;
        s.remote.answers.insert(id, result);
        s.wanted = Some(crate::sync::adapter::Want::Now);
        return notice;
    }
    "Remote job ended after leaving its sync space".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{
        core::Device,
        space::{Identity, Space},
        transport::{Transport, sync as exchange},
    };
    use std::path::PathBuf;
    #[test]
    fn remote_inventory_and_installed_commands_need_no_entry_file() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let local = Space::create("requester", None)?;
        let sync = SyncState::new(local.id);
        let request = snapshot(
            &sync,
            dir.path(),
            "host".into(),
            String::new(),
            &[],
            Operation::Inventory,
            vec![],
        )?;
        let receipt = perform(&request, dir.path(), false, &Default::default())?;
        assert!(receipt.ok && receipt.text.contains(std::env::consts::OS));
        let mut invocation = request;
        invocation.operation = Operation::Tool {
            id: "a".repeat(24),
            command: "check".into(),
        };
        invocation.validate()?;
        assert!(perform(&invocation, dir.path(), false, &Default::default()).is_err());
        Ok(())
    }
    #[tokio::test]
    #[ignore = "separate Windows and Linux processes share a disposable encrypted test pool"]
    async fn cross_platform_remote_build() -> Result<()> {
        let base = PathBuf::from(
            std::env::var_os("SPINDLE_POOL_QA").context("private test root required")?,
        );
        let phase = std::env::var("SPINDLE_POOL_PHASE")?;
        let relay = Transport::Folder(base.join("relay"));
        if phase == "send" {
            fs::create_dir_all(&base)?;
            let mut local = Space::create("sender", None)?;
            let target = Space::open(
                Identity {
                    keys: local.keys_for(false),
                    device: Device::generate("builder")?,
                    secrets: false,
                },
                None,
            )?;
            local.add_member(target.member_entry()?)?;
            let mut state = SyncState::new(local.id.clone());
            let work = tempfile::tempdir()?;
            fs::write(
                work.path().join("main.rs"),
                r#"fn main(){println!("remote sandbox {}",std::env::consts::OS);}"#,
            )?;
            let req = snapshot(
                &state,
                work.path(),
                target.id.device.id.clone(),
                "main.rs".into(),
                &[],
                Operation::Build,
                vec![],
            )?;
            state.remote.sent.insert(signature(&req)?, req);
            export(&state, &mut local)?;
            exchange(&mut local, &relay, &mut Default::default()).await?;
            fs::write(base.join("sender.fixture"), serde_json::to_vec(&state)?)?;
            fs::write(base.join("target.fixture"), serde_json::to_vec(&target.id)?)?;
        } else if phase == "build" {
            let id: Identity = serde_json::from_slice(&fs::read(base.join("target.fixture"))?)?;
            let sender: SyncState =
                serde_json::from_slice(&fs::read(base.join("sender.fixture"))?)?;
            let mut local = Space::open(id.clone(), None)?;
            let mut state = SyncState::new(id);
            exchange(&mut local, &relay, &mut Default::default()).await?;
            import(&mut state, &local)?;
            assert!(state.remote.pending.is_empty());
            state.remote.allowed.insert(sender.identity.device.id);
            import(&mut state, &local)?;
            let (id, r) = state
                .remote
                .pending
                .pop_first()
                .context("Target did not receive job")?;
            let cancel = Arc::new(AtomicBool::new(false));
            assert!(perform(&r, &base.join("data"), false, &cancel).is_err());
            let out = perform(&r, &base.join("data"), true, &cancel)?;
            assert!(
                out.ok && out.text.contains("remote sandbox"),
                "{}",
                out.text
            );
            println!("{}", out.text);
            state.remote.answers.insert(id, out);
            export(&state, &mut local)?;
            exchange(&mut local, &relay, &mut Default::default()).await?;
            import(&mut state, &local)?;
            assert!(state.remote.pending.is_empty());
        } else {
            let mut state: SyncState =
                serde_json::from_slice(&fs::read(base.join("sender.fixture"))?)?;
            let mut local = Space::open(state.identity.clone(), None)?;
            exchange(&mut local, &relay, &mut Default::default()).await?;
            import(&mut state, &local)?;
            let r = state
                .remote
                .received
                .values()
                .next()
                .context("Sender received no authenticated reply")?;
            let data = core::unb64(&r.bytes)?;
            assert!(
                r.ok && ((r.extension == "elf" && data.starts_with(b"\x7fELF"))
                    || (r.extension == "exe" && data.starts_with(b"MZ")))
            );
            println!(
                "Sender received {} bytes of tested .{} output",
                data.len(),
                r.extension
            );
        }
        Ok(())
    }
}
