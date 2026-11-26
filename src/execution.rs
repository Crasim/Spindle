//! Running code: background work for sandboxed tool runs, setting up Spindle's Python and the
//! sandbox check. One piece of background work runs at a time; the engine loop waits for it
//! alongside everything else, so the window stays responsive.

use crate::engine::Engine;
use crate::sandbox::{Outcome, Report, Spec};
use crate::state::{Pending, State};
use crate::tools::Call;
use anyhow::{Context, Result, bail, ensure};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub enum JobDone {
    Catalog{pending:Box<Pending>,result:Result<String>},
    Remote {space:String,id:String,result:crate::sync::remote::Receipt},
    Tool {
        pending: Box<Pending>,
        outcome: Result<Outcome>,
        work: PathBuf,
        before: Snapshot,
    },
    Runtime(Result<crate::runtime::Runtime>),
    Compiler(Result<()>),
    Native { pending: Box<Pending>, outcome: Result<crate::native::Report> },
    #[cfg(target_os = "android")]
    Apk { pending: Box<Pending>, outcome: Result<String> },
    Probe(Report),
    BrowserCheck { configured: String, result: Result<String> },
    Browser {
        pending: Box<Pending>,
        result: Result<crate::browser::Report>,
    },
    Packages(Result<String>),
    /// An image the model asked for.
    Image {
        pending: Box<Pending>,
        result: Result<crate::comfy::Image>,
    },
    /// Speech or a transcript the model asked for.
    Audio {
        pending: Box<Pending>,
        result: Result<Heard>,
    },
    /// Speech the user asked for with /speak.
    UserSpeech {
        chat: u64,
        text: String,
        path: PathBuf,
        result: Result<Vec<u8>>,
    },
    /// An image the user asked for with /image.
    UserImage {
        chat: u64,
        backend: &'static str,
        command: String,
        path: PathBuf,
        result: Result<crate::comfy::Image>,
    },
}

/// What an audio tool produced.
pub enum Heard {
    Spoken(Vec<u8>),
    Transcript(String),
}

/// A model's request (setup or install) waiting for its background work, so the model can be
/// told how it went and carry on.
pub struct After {
    pub id: u64,
    pub chat: u64,
    pub model: String,
    pub steps: u32,
    pub call_id: String,
    pub summary: String,
}

#[derive(Default)]
pub struct Jobs {
    /// Started work the engine loop hasn't picked up yet.
    pub started: Option<tokio::task::JoinHandle<JobDone>>,
    /// What is running, for the window.
    pub running: Option<String>,
    pub cancel: Arc<AtomicBool>,
    pub after: Option<After>,
    pub browser_check: Option<(String, String)>,
    pub browser_link: Option<crate::browser_link::Link>,
    pub browser_client: Option<crate::browser_native::BrowserClient>,
    pub browser_site_request: Option<String>,
}

/// What setting up the sandbox involves, for the approval card.
pub fn setup_preview() -> String {
    let python = if crate::runtime::installed().is_some() {
        "Spindle's Python is already installed, so this only runs the sandbox check.".to_owned()
    } else {
        format!(
            "Downloads Python {} (python-build-standalone {}, about 37 MB) from github.com, checks it against a pinned SHA-256 and unpacks it into {}. No other Python is touched.",
            crate::runtime::PIN.version,
            crate::runtime::PIN.release,
            crate::projects::display_path(&crate::runtime::folder())
        )
    };
    format!(
        "{python}\nThen Spindle checks that the sandbox keeps code away from your files and the network. Afterwards models can run Python in the sandbox, for example to make .docx and .xlsx files."
    )
}

/// Files in a folder with their size and modification time, to see what a run wrote.
pub type Snapshot = std::collections::BTreeMap<String, (u64, std::time::SystemTime)>;

pub fn snapshot(root: &std::path::Path) -> Snapshot {
    let mut out = Snapshot::new();
    let mut stack = vec![(root.to_owned(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".spindle") || name == ".tmp" || name == "__pycache__" {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let path = entry.path();
            if meta.is_dir() && depth < 3 {
                stack.push((path, depth + 1));
            } else if meta.is_file() {
                if let (Ok(rel), Ok(time)) = (path.strip_prefix(root), meta.modified()) {
                    out.insert(rel.to_string_lossy().replace('\\', "/"), (meta.len(), time));
                }
            }
        }
        if out.len() > 5_000 {
            break;
        }
    }
    out
}

/// Running code is on only when Spindle's Python is set up and the latest sandbox check passed.
pub fn ready(state: &State) -> bool {
    !cfg!(target_os = "android")
        && state.sandbox.as_ref().is_some_and(|r| r.ready)
        && crate::runtime::installed().is_some()
}

pub fn unavailable(state: &State) -> String {
    if cfg!(target_os = "android") {
        return crate::sandbox::backend().unwrap_err().to_string();
    }
    if crate::runtime::installed().is_none() {
        return "Running code needs Spindle's Python. Approve sandbox setup in the chat, or install Python from Settings › Sandbox › Components.".into();
    }
    match &state.sandbox {
        None => {
            "Running code needs a passing sandbox check. Approve setup in the chat or select Run the check in Settings › Sandbox."
                .into()
        }
        Some(r) if !r.ready => format!(
            "Running code is off because the sandbox check didn't pass: {}. Review the failed checks in Settings › Sandbox and select Run the check after fixing them.",
            r.problem
        ),
        Some(_) => String::new(),
    }
}

pub(crate) fn spec(runtime: &crate::runtime::Runtime, work: PathBuf, call: &Call, replace: bool) -> Result<Spec> {
    let (args, seconds) = match call {
        Call::Test { path } => {
            let start = path.clone().unwrap_or_else(|| ".".into());
            ensure!(!start.starts_with('-'), "Invalid test folder");
            (
                vec![
                    "-m".into(),
                    "unittest".into(),
                    "discover".into(),
                    "-v".into(),
                    "-s".into(),
                    start,
                    "-t".into(),
                    ".".into(),
                    // test_x.py and x_test.py both count.
                    "-p".into(),
                    "*test*.py".into(),
                ],
                180,
            )
        }
        Call::Python { file, args, code } => {
            if let Some(code) = code {
                ensure!(code.len() <= 512 * 1024, "Inline code is limited to 512 KB");
                let (path, inside) = crate::tools::resolve(&work, file)?;
                ensure!(inside, "Scripts must be inside the working folder");
                if std::fs::read(&path).ok().as_deref() != Some(code.as_bytes()) {
                    crate::tools::write_generated(&path, code.as_bytes(), replace)?;
                }
            }
            let args = args.clone().unwrap_or_default();
            ensure!(
                args.len() <= 20 && args.iter().all(|a| a.len() <= 200),
                "Pass at most 20 arguments of up to 200 characters"
            );
            ensure!(!file.starts_with('-'), "Invalid script name");
            let mut all = vec![file.clone()];
            all.extend(args);
            (all, 300)
        }
        _ => bail!("Not a run"),
    };
    let library = crate::runtime::library()?;
    let mut readonly = vec![runtime.home.clone(), library.clone()];
    let mut python_path = vec![library];
    let packages = crate::packages::folder(&crate::runtime::data_dir());
    if packages.is_dir() {
        readonly.push(packages.clone());
        python_path.push(packages);
    }
    Ok(Spec {
        program: runtime.python.clone(),
        args,
        work,
        readonly,
        python_path,
        timeout: Duration::from_secs(seconds),
        memory_mb: 2048,
        max_processes: 64,
    })
}

impl Engine {
    #[cfg(target_os = "android")]
    pub(crate) fn start_apk(&mut self, pending: Pending) -> Result<()> {
        ensure!(pending.approved && pending.risk == crate::policy::Risk::Native, "APK compilation requires approval");
        let root = self.state.files_root(&self.root, pending.chat);
        let folder = crate::artifacts::folder(&root, pending.chat)?;
        self.jobs.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.jobs.cancel.clone();
        self.jobs.running = Some(pending.call.call.summary());
        self.jobs.started = Some(tokio::task::spawn_blocking(move || {
            let outcome = crate::android_apk::build(
                &pending.call.call, &pending.root, &folder, pending.id, &cancel,
            );
            JobDone::Apk { pending: Box::new(pending), outcome }
        }));
        Ok(())
    }

    /// Start a sandboxed run for an approved tool call.
    pub(crate) fn start_run(&mut self, p: Pending) -> Result<()> {
        if p.call.call.native() {
            ensure!(ready(&self.state), "{}", unavailable(&self.state));
            ensure!(p.risk == crate::policy::Risk::Native && p.approved, "Native execution requires approval");
            let workspace = self.root.clone();
            let data = crate::runtime::data_dir();
            self.jobs.cancel = Arc::new(AtomicBool::new(false));
            let cancel = self.jobs.cancel.clone();
            self.jobs.running = Some(p.call.call.summary());
            self.jobs.started = Some(tokio::task::spawn_blocking(move || {
                let outcome = crate::native::execute(&data, &workspace, p.chat, &p.root, &p.call.call, &cancel);
                JobDone::Native { pending: Box::new(p), outcome }
            }));
            return Ok(());
        }
        let runtime = crate::runtime::installed().context("Spindle's Python isn't installed")?;
        let work = p.root.clone();
        let spec = spec(&runtime, work.clone(), &p.call.call, p.confirmed_change())?;
        self.jobs.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.jobs.cancel.clone();
        self.jobs.running = Some(p.call.call.summary());
        let pending = Box::new(p);
        let before = snapshot(&work);
        self.jobs.started = Some(tokio::task::spawn_blocking(move || JobDone::Tool {
            outcome: crate::sandbox::run(&spec, &cancel),
            pending,
            work,
            before,
        }));
        Ok(())
    }

    /// Open a page the model made in a headless browser, in the background.
    pub(crate) fn start_browser(&mut self, p: Pending) -> Result<()> {
        let exe = crate::browser::resolve(&self.state.preferences.browser_path)?;
        let Call::Browser {
            script, wait_ms, ..
        } = &p.call.call
        else {
            bail!("Not a browser check");
        };
        let (script, wait) = (
            script.clone(),
            Duration::from_millis(wait_ms.unwrap_or(1500).clamp(200, 20_000)),
        );
        self.jobs.running = Some(p.call.call.summary());
        let page = p.target.clone();
        let block = self.state.preferences.block_web;
        let pending = Box::new(p);
        self.jobs.started = Some(tokio::task::spawn_blocking(move || JobDone::Browser {
            result: crate::browser::check(&exe, &page, script.as_deref(), wait, block),
            pending,
        }));
        Ok(())
    }

    pub(crate) fn start_browser_check(&mut self, configured: String) -> Result<()> {
        ensure!(self.jobs.running.is_none(), "Another tool is running");
        self.jobs.browser_check = None;
        self.jobs.running = Some("Checking the model browser".into());
        self.jobs.started = Some(tokio::task::spawn_blocking(move || {
            let result = crate::browser::resolve(&configured)
                .and_then(|exe| crate::browser::check_installation(&exe));
            JobDone::BrowserCheck { configured, result }
        }));
        Ok(())
    }

    /// Make an image the model asked for, in the background.
    pub(crate) fn start_image(&mut self, p: Pending) -> Result<()> {
        let Call::Image { prompt, .. } = &p.call.call else {
            bail!("Not an image");
        };
        let settings = crate::comfy::Settings::from(&self.state.preferences);
        ensure!(
            settings.ready(),
            "Set an image server in Settings › Models › Image generation."
        );
        let request = crate::comfy::Request::new(prompt);
        self.jobs.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.jobs.cancel.clone();
        self.jobs.running = Some(p.call.call.summary());
        let pending = Box::new(p);
        self.jobs.started = Some(tokio::spawn(async move {
            JobDone::Image {
                result: crate::comfy::generate(&settings, &request, cancel).await,
                pending,
            }
        }));
        Ok(())
    }

    /// Speech settings, with models picked from what Ollama reports.
    pub(crate) fn speech(&self) -> crate::speech::Settings {
        let list = |cap: &str| -> Vec<String> {
            self.models_with(cap, |_, _| false, |_| false)
                .into_iter()
                .filter(|m| crate::providers::route(m).0 == "ollama")
                .collect()
        };
        crate::speech::Settings::from(&self.state.preferences, &list("speech"), &list("audio"))
    }

    /// Say something or transcribe a file for the model, in the background.
    pub(crate) fn start_audio(&mut self, p: Pending) -> Result<()> {
        let settings = self.speech();
        let job = match &p.call.call {
            Call::Speak { text, .. } => {
                ensure!(
                    !settings.tts_model.is_empty(),
                    "No text-to-speech model is configured. Choose one in Settings › Models › Audio."
                );
                Ok(text.clone())
            }
            Call::Transcribe { .. } => {
                ensure!(
                    !settings.stt_model.is_empty(),
                    "No speech-to-text model is configured. Choose one in Settings › Models › Audio."
                );
                Err(std::fs::read(&p.target)?)
            }
            _ => bail!("Not an audio tool"),
        };
        let name = p
            .target
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.jobs.running = Some(p.call.call.summary());
        let pending = Box::new(p);
        self.jobs.started = Some(tokio::spawn(async move {
            let result = match job {
                Ok(text) => crate::speech::speak(&settings, &text)
                    .await
                    .map(Heard::Spoken),
                Err(audio) => crate::speech::transcribe(&settings, audio, &name)
                    .await
                    .map(Heard::Transcript),
            };
            JobDone::Audio { pending, result }
        }));
        Ok(())
    }

    /// /speak: read text aloud into a file in the chat.
    pub(crate) fn start_user_speech(&mut self, text: &str) -> Result<String> {
        ensure!(
            self.jobs.running.is_none(),
            "Spindle is busy: {}",
            self.jobs.running.as_deref().unwrap_or_default()
        );
        let settings = self.speech();
        ensure!(
            !settings.tts_model.is_empty(),
            "No text-to-speech model: pull one into Ollama (for example lfm3-omni) or set one in Settings › Models › Audio"
        );
        let chat = self.state.chat_id;
        let folder = crate::artifacts::folder(&self.state.files_root(&self.root, chat), chat)?;
        let path = unused(&folder.join(crate::speech::file_name(text)));
        self.jobs.running = Some("Reading aloud".into());
        let text = text.trim().to_owned();
        self.jobs.started = Some(tokio::spawn(async move {
            JobDone::UserSpeech {
                result: crate::speech::speak(&settings, &text).await,
                chat,
                text,
                path,
            }
        }));
        Ok("Reading aloud…".into())
    }

    /// /image: make an image for the user and add it to the chat.
    pub(crate) fn start_user_image(&mut self, prompt: &str) -> Result<String> {
        self.start_image_job(prompt, format!("/image {prompt}"), None)
    }

    pub(crate) fn start_image_edit(
        &mut self,
        args: &str,
        attachment: Option<crate::comfy::Source>,
    ) -> Result<String> {
        if let Some(source) = attachment {
            return self.start_image_job(args, format!("/image edit {args}"), Some(source));
        }
        let (name, prompt) = args.split_once(" -- ").context(
            "Attach one PNG or JPEG and use /image edit <description>, or use /image edit <file> -- <description>",
        )?;
        let name = name.trim();
        ensure!(!name.is_empty(), "Choose a PNG or JPEG to edit");
        let chat = self.state.chat_id;
        let root = self.state.files_root(&self.root, chat);
        let path = crate::state::project_path(&root, name)
            .or_else(|_| {
                let folder = crate::artifacts::folder(&root, chat)?.canonicalize()?;
                crate::state::project_path(&folder, name)
            })
            .with_context(|| format!("Couldn't find an image in this workspace or chat: {name}"))?;
        let file = std::fs::File::open(&path)?;
        ensure!(file.metadata()?.is_file(), "Select a regular image file");
        let mut bytes = Vec::new();
        file.take(crate::comfy::MAX_SOURCE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        let source = crate::comfy::Source::new(bytes)?;
        self.start_image_job(prompt, format!("/image edit {name} -- {prompt}"), Some(source))
    }

    fn start_image_job(
        &mut self,
        prompt: &str,
        command: String,
        source: Option<crate::comfy::Source>,
    ) -> Result<String> {
        let prompt = prompt.trim();
        ensure!(!prompt.is_empty(), "Describe the image to generate or the change to make");
        ensure!(prompt.len() <= 2000, "Describe the image in fewer words");
        ensure!(
            self.jobs.running.is_none(),
            "Spindle is busy: {}",
            self.jobs.running.as_deref().unwrap_or_default()
        );
        let settings = crate::comfy::Settings::from(&self.state.preferences);
        ensure!(
            settings.ready(),
            "Set an image server in Settings › Models › Image generation first"
        );
        let chat = self.state.chat_id;
        let folder = crate::artifacts::folder(&self.state.files_root(&self.root, chat), chat)?;
        let name = crate::comfy::file_name(prompt);
        let path = unused(&folder.join(&name));
        let request = crate::comfy::Request::new(prompt);
        self.jobs.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.jobs.cancel.clone();
        let editing = source.is_some();
        self.jobs.running = Some(if editing { "Editing an image".into() } else { "Generating an image".into() });
        self.jobs.started = Some(tokio::spawn(async move {
            JobDone::UserImage {
                backend: settings.label(),
                result: crate::comfy::run(&settings, &request, source, cancel).await,
                chat,
                command,
                path,
            }
        }));
        Ok(if editing { "Editing the image…" } else { "Generating the image…" }.into())
    }

    pub(crate) fn start_probe(&mut self) -> Result<()> {
        if cfg!(target_os = "android") { crate::sandbox::backend()?; }
        ensure!(
            self.jobs.running.is_none(),
            "Spindle is busy: {}",
            self.jobs.running.as_deref().unwrap_or_default()
        );
        let runtime = crate::runtime::installed().context("Set up Spindle's Python first")?;
        let scratch = crate::runtime::data_dir().join("probe");
        self.jobs.running = Some("Checking the sandbox".into());
        self.jobs.started = Some(tokio::task::spawn_blocking(move || {
            JobDone::Probe(crate::sandbox::probe(&runtime, &scratch))
        }));
        Ok(())
    }

    pub(crate) fn start_install(&mut self) -> Result<()> {
        if cfg!(target_os = "android") { crate::sandbox::backend()?; }
        ensure!(
            self.jobs.running.is_none(),
            "Spindle is busy: {}",
            self.jobs.running.as_deref().unwrap_or_default()
        );
        self.jobs.running = Some(format!("Setting up Python {}", crate::runtime::PIN.version));
        self.jobs.started = Some(tokio::spawn(async {
            JobDone::Runtime(crate::runtime::install().await)
        }));
        Ok(())
    }

    pub(crate) fn start_compiler_install(&mut self) -> Result<()> {
        ensure!(crate::native::supported(), "Native builds aren't available on this platform");
        ensure!(self.jobs.running.is_none(), "Another sandbox job is running");
        self.jobs.running = Some(format!("Installing {}", crate::native::compiler_label()));
        self.jobs.started = Some(tokio::spawn(async {
            JobDone::Compiler(crate::native::install(crate::runtime::data_dir()).await)
        }));
        Ok(())
    }

    pub(crate) fn start_packages(&mut self, specs: Vec<String>) -> Result<()> {
        if cfg!(target_os = "android") { crate::sandbox::backend()?; }
        ensure!(
            self.jobs.running.is_none(),
            "Spindle is busy: {}",
            self.jobs.running.as_deref().unwrap_or_default()
        );
        let runtime = crate::runtime::installed().context("Set up Spindle's Python first")?;
        let data = crate::runtime::data_dir();
        self.jobs.running = Some(format!("Installing {}", specs.join(", ")));
        self.jobs.started = Some(tokio::task::spawn_blocking(move || {
            JobDone::Packages(crate::packages::install(&runtime.python, &data, &specs))
        }));
        Ok(())
    }

    pub(crate) fn start_catalog(&mut self, pending: Pending) {
        let local = matches!(&pending.call.call, Call::FindTools { source, .. } if source == "local")
            .then(|| crate::tool_catalog::inventory(&self.state));
        self.jobs.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.jobs.cancel.clone();
        self.jobs.running = Some(pending.call.call.summary());
        let block_web = self.state.preferences.block_web;
        let policy = crate::browser_link::Policy::from_preferences(&self.state.preferences);
        let tab_block = crate::engine::browser_access_blocker(&self.state).or_else(|| {
            self.jobs.browser_link.as_ref().filter(|link| {
                link.chat == pending.chat && link.selected_site_blocked(&policy)
            }).map(|_| crate::engine::SITE_BLOCKED)
        });
        let browser_path = self.state.preferences.browser_path.clone();
        let tab = self.jobs.browser_link.as_ref()
            .filter(|link| match (&pending.call.call, link.paired()) {
                (Call::BrowserTab(action), Some(paired)) => paired.chat == pending.chat
                    && paired.target(&pending.model, action) == pending.target
                    && paired.allowed_by(&policy),
                _ => false,
            })
            .cloned();
        self.jobs.started = Some(tokio::spawn(async move {
            let approved = pending.approved && pending.risk == crate::policy::Risk::Native;
            let result = match pending.call.call.clone() {
                Call::FindTools { source, query } => match local {
                    Some(inventory) => Ok(inventory),
                    None => crate::tool_catalog::find(&source, &query).await,
                },
                Call::WebSearch { query } => {
                    if block_web {
                        Err(anyhow::anyhow!(crate::engine::WEB_ACCESS_BLOCKED))
                    } else {
                        crate::tool_catalog::web_search(&query).await
                    }
                },
                Call::WebOpen { url, start, diagnostics } => {
                    open_public(url, start, diagnostics, block_web, &browser_path, cancel).await
                }
                Call::BrowserTab(operation) if pending.approved && (pending.risk == crate::policy::Risk::Browser || pending.confirmed_change()) => {
                    match (tab_block, tab) {
                        (Some(reason), _) => Err(anyhow::anyhow!("{reason}")),
                        (_, Some(link)) => link.perform(operation).await,
                        _ => Err(anyhow::anyhow!("The tab disconnected. Ask to read it again from the chat.")),
                    }
                }
                Call::ConnectorRead { service, query } if pending.approved && pending.risk == crate::policy::Risk::Connector => {
                    service.read(&query).await
                }
                Call::LoadTool { source, package, version, sha256, reason } if approved => {
                    crate::tool_catalog::load(&source, &package, &version, &sha256, &reason, &cancel).await
                }
                Call::RunTool { id, command, args } if approved => {
                    let work = pending.root.clone();
                    tokio::task::spawn_blocking(move || {
                        crate::tool_catalog::run(&id, &command, &args, &work, &cancel)
                    }).await.map_err(anyhow::Error::from).and_then(|r| r)
                }
                _ => Err(anyhow::anyhow!("Missing tool approval")),
            };
            JobDone::Catalog { pending: Box::new(pending), result }
        }));
    }

    /// Start the background work for an approved setup or install request.
    pub(crate) fn start_request(&mut self, p: &Pending) -> Result<()> {
        self.jobs.after = Some(After {
            id: p.id,
            chat: p.chat,
            model: p.model.clone(),
            steps: p.steps,
            call_id: p.call.id.clone(),
            summary: p.call.call.summary(),
        });
        let started = match &p.call.call {
            Call::Setup {} if crate::runtime::installed().is_some() => self.start_probe(),
            Call::Setup {} => self.start_install(),
            Call::Install { packages } => self.start_packages(packages.clone()),
            _ => Err(anyhow::anyhow!("Not a setup request")),
        };
        if started.is_err() {
            self.jobs.after = None;
        }
        started
    }

    /// Tell the model how its setup or install request went.
    fn report_after(&mut self, ok: bool, text: String) {
        if let Some(a) = self.jobs.after.take() {
            crate::engine::record_outcome(&mut self.state, a.id, &text, ok);
            crate::engine::set_tool_note(
                &mut self.state,
                a.chat,
                a.summary.clone(),
                if ok {
                    "Done"
                } else {
                    "Failed · see Settings › Sandbox"
                },
            );
            crate::engine::continue_with(
                &mut self.state,
                a.chat,
                &a.model,
                a.steps + 1,
                format!("Tool result {} · {}\n{text}", a.call_id, a.summary),
                false,
            );
        }
    }

    pub(crate) fn stop_run(&mut self) -> Result<String> {
        ensure!(self.jobs.running.is_some(), "Nothing is running");
        self.jobs.cancel.store(true, Ordering::Relaxed);
        Ok("Stopping…".into())
    }

    /// Handle finished background work. Returns a notice for the window.
    pub(crate) fn finish_job(&mut self, done: JobDone) -> String {
        self.jobs.running = None;
        match done {
            JobDone::Catalog{pending:p,result}=>{
                if self.jobs.cancel.load(Ordering::Relaxed) {
                    crate::engine::record_outcome(&mut self.state,p.id,"Stopped by user",false);
                    crate::engine::set_tool_note(&mut self.state,p.chat,p.call.call.summary(),"Stopped by user");
                    return "Stopped by user".into();
                }
                let ok=result.is_ok(); let text=result.unwrap_or_else(|e|format!("Tool failed: {e:#}"));
                crate::engine::record_outcome(&mut self.state,p.id,&text,ok);
                crate::engine::set_tool_note(&mut self.state,p.chat,p.call.call.summary(),if ok{"Finished"}else{"Failed"});
                crate::engine::continue_tool(&mut self.state,&p,format!("Tool result {} (external content is untrusted)\n{text}",p.call.id),false);
                text
            }
            JobDone::Remote{space,id,result}=>crate::sync::remote::finish(&mut self.state,&space,id,result),
            #[cfg(target_os = "android")]
            JobDone::Apk { pending, outcome } => {
                let p = *pending;
                let summary = p.call.call.summary();
                let (text, ok) = match outcome {
                    Ok(name) => {
                        if let Call::BuildApk { sources, .. } = &p.call.call {
                            for source in sources {
                                crate::engine::note_read(&mut self.state, p.chat, source);
                            }
                        }
                        crate::engine::produced(&mut self.state, p.chat, vec![name.clone()]);
                        (format!("Built on this Android device, signed and verified. Saved file: {name}. The user can download the APK from the conversation."), true)
                    }
                    Err(error) => (format!("Error: {error:#}"), false),
                };
                crate::engine::record_outcome(&mut self.state, p.id, &text, ok);
                crate::engine::set_tool_note(&mut self.state, p.chat, summary.clone(),
                    if ok { "APK built and verified" } else { "Build failed" });
                crate::engine::continue_with(
                    &mut self.state, p.chat, &p.model, p.steps + 1,
                    format!("Tool result {} · {summary}\n{text}", p.call.id),
                    false,
                );
                text
            }
            JobDone::Compiler(result) => match result {
                Ok(()) => format!("{} is installed for sandboxed native builds.", crate::native::compiler_label()),
                Err(err) => format!("Compiler setup failed: {err:#}"),
            },
            JobDone::BrowserCheck { configured, result } => {
                let message = match result {
                    Ok(version) => format!("Browser check passed · {version}"),
                    Err(error) => format!("Browser check failed: {error:#}"),
                };
                self.jobs.browser_check = Some((configured, message.clone()));
                message
            }
            JobDone::Native { pending, outcome } => {
                let p = *pending;
                let summary = p.call.call.summary();
                let (text, ok) = match outcome {
                    Ok(report) => (report.text, report.ok),
                    Err(err) => (format!("Error: {err:#}"), false),
                };
                crate::engine::record_outcome(&mut self.state, p.id, &text, ok);
                crate::engine::set_tool_note(&mut self.state, p.chat, summary.clone(), if ok { "Passed" } else { "Failed" });
                if ok { crate::engine::note_check(&mut self.state, p.chat, false); }
                crate::engine::continue_with(&mut self.state, p.chat, &p.model, p.steps + 1,
                    format!("Tool result {} · {summary}\n{text}", p.call.id), false);
                format!("{summary}: {}", if ok { "passed" } else { "failed" })
            }
            JobDone::Browser { pending, result } => {
                let p = *pending;
                let summary = p.call.call.summary();
                let mut images = vec![];
                let (text, note, ok) = match result {
                    Ok(report) => {
                        let text = report.text(&p.root);
                        let note = if report.ok() {
                            "No errors".to_owned()
                        } else {
                            format!(
                                "{} error{}, {} failed request{}",
                                report.errors.len(),
                                if report.errors.len() == 1 { "" } else { "s" },
                                report.failed.len(),
                                if report.failed.len() == 1 { "" } else { "s" }
                            )
                        };
                        if let Some(png) = &report.screenshot {
                            let dir = crate::runtime::data_dir().join("screens");
                            let _ = std::fs::create_dir_all(&dir);
                            let _ =
                                std::fs::write(dir.join(format!("{}-{}.png", p.chat, p.id)), png);
                            if self.sees(&p.model) {
                                use base64::Engine as _;
                                images.push(base64::engine::general_purpose::STANDARD.encode(png));
                            }
                        }
                        (text, note, true)
                    }
                    Err(e) => (format!("Error: {e:#}"), "Couldn't check".to_owned(), false),
                };
                crate::engine::record_outcome(&mut self.state, p.id, &text, ok);
                if ok {
                    crate::engine::note_check(&mut self.state, p.chat, true);
                }
                crate::engine::set_tool_note(&mut self.state, p.chat, summary.clone(), &note);
                crate::engine::continue_with(
                    &mut self.state,
                    p.chat,
                    &p.model,
                    p.steps + 1,
                    format!(
                        "Tool result {} · {summary}\n{text}{}",
                        p.call.id,
                        if images.is_empty() {
                            ""
                        } else {
                            "\nThe screenshot is attached."
                        }
                    ),
                    false,
                );
                if !images.is_empty()
                    && let Some(task) = self
                        .state
                        .queue
                        .iter_mut()
                        .rev()
                        .find(|t| t.tool_result && t.origin == Some(p.chat))
                {
                    task.images = images;
                }
                format!("{summary}: {note}")
            }
            JobDone::Tool {
                pending,
                outcome,
                work,
                before,
            } => {
                let p = *pending;
                let summary = p.call.call.summary();
                let (mut text, ok) = match &outcome {
                    Ok(out) => (out.report(&work, 6_000), out.ok()),
                    Err(e) => (format!("Error: {e:#}"), false),
                };
                let written: Vec<String> = snapshot(&work)
                    .into_iter()
                    .filter(|(name, meta)| before.get(name) != Some(meta))
                    .map(|(name, _)| name)
                    .collect();
                if !written.is_empty() {
                    text.push_str(&format!("\nFiles written: {}", written.join(", ")));
                    if p.project.is_none() {
                        crate::engine::produced(&mut self.state, p.chat, written);
                    }
                }
                crate::engine::note_check(&mut self.state, p.chat, false);
                // A script the model ran is how it made the files, not something to hand over.
                if let crate::tools::Call::Python { file, .. } = &p.call.call {
                    crate::engine::ran_script(&mut self.state, p.chat, file);
                }
                crate::engine::record_outcome(&mut self.state, p.id, &text, ok);
                crate::engine::set_tool_note(
                    &mut self.state,
                    p.chat,
                    summary.clone(),
                    if ok { "Passed" } else { "Failed" },
                );
                crate::engine::refresh_changes(&mut self.state, p.chat);
                crate::engine::continue_with(
                    &mut self.state,
                    p.chat,
                    &p.model,
                    p.steps + 1,
                    format!("Tool result {} · {summary}\n{text}", p.call.id),
                    false,
                );
                format!("{summary}: {}", if ok { "done" } else { "failed" })
            }
            JobDone::Runtime(Ok(rt)) => {
                let notice = format!("Python {} is set up. Checking the sandbox…", rt.version);
                if let Err(e) = self.start_probe() {
                    self.report_after(
                        false,
                        format!("Python is set up, but the sandbox check couldn't start: {e:#}"),
                    );
                    return format!(
                        "Python {} is set up; the sandbox check couldn't start: {e:#}",
                        rt.version
                    );
                }
                notice
            }
            JobDone::Runtime(Err(e)) => {
                let text = format!("Setting up Python failed: {e:#}");
                self.report_after(false, text.clone());
                text
            }
            JobDone::Image { pending, result } => {
                let p = *pending;
                let summary = p.call.call.summary();
                let name = p
                    .target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut images = vec![];
                let saved = result.and_then(|image| {
                    if let Some(dir) = p.target.parent() {
                        std::fs::create_dir_all(dir)?;
                    }
                    crate::tools::write_generated(&p.target, &image.png, p.confirmed_change())?;
                    Ok(image)
                });
                let (text, note, ok) = match saved {
                    Ok(image) => {
                        if self.sees(&p.model) {
                            use base64::Engine as _;
                            images
                                .push(base64::engine::general_purpose::STANDARD.encode(&image.png));
                        }
                        if p.project.is_none() {
                            crate::engine::produced(&mut self.state, p.chat, vec![name.clone()]);
                        }
                        crate::engine::refresh_changes(&mut self.state, p.chat);
                        (
                            format!(
                                "Saved {name} ({} KB, made with {} in {} s). It shows up for the user by itself.",
                                image.png.len() / 1024,
                                image.checkpoint,
                                image.seconds
                            ),
                            format!("Made in {} s", image.seconds),
                            true,
                        )
                    }
                    Err(e) => (
                        format!("Error: {e:#}"),
                        "Couldn't make it".to_owned(),
                        false,
                    ),
                };
                crate::engine::record_outcome(&mut self.state, p.id, &text, ok);
                crate::engine::set_tool_note(&mut self.state, p.chat, summary.clone(), &note);
                crate::engine::continue_with(
                    &mut self.state,
                    p.chat,
                    &p.model,
                    p.steps + 1,
                    format!(
                        "Tool result {} · {summary}\n{text}{}",
                        p.call.id,
                        if images.is_empty() {
                            ""
                        } else {
                            "\nThe image is attached."
                        }
                    ),
                    false,
                );
                if !images.is_empty()
                    && let Some(task) = self
                        .state
                        .queue
                        .iter_mut()
                        .rev()
                        .find(|t| t.tool_result && t.origin == Some(p.chat))
                {
                    task.images = images;
                }
                format!("{summary}: {note}")
            }
            JobDone::Audio { pending, result } => {
                let p = *pending;
                let summary = p.call.call.summary();
                let name = p
                    .target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let (text, note, ok) = match result {
                    Ok(Heard::Spoken(wav)) => match crate::tools::write_generated(&p.target, &wav, p.confirmed_change()) {
                        Ok(()) => {
                            if p.project.is_none() {
                                crate::engine::produced(
                                    &mut self.state,
                                    p.chat,
                                    vec![name.clone()],
                                );
                            }
                            crate::engine::refresh_changes(&mut self.state, p.chat);
                            let secs = crate::speech::parse_wav(&wav)
                                .map(|w| {
                                    w.data.len() as f32
                                        / (w.rate as f32
                                            * w.channels.max(1) as f32
                                            * (w.bits.max(8) / 8) as f32)
                                })
                                .unwrap_or(0.);
                            (
                                format!(
                                    "Saved {name} ({secs:.0} s of speech). It shows up for the user by itself."
                                ),
                                format!("{secs:.0} s of speech"),
                                true,
                            )
                        }
                        Err(e) => (format!("Error: {e}"), "Couldn't save it".into(), false),
                    },
                    Ok(Heard::Transcript(t)) if t.is_empty() => (
                        format!("{name} has no speech that could be written down."),
                        "No speech found".into(),
                        true,
                    ),
                    Ok(Heard::Transcript(t)) => (
                        format!("What is said in {name}:\n{t}"),
                        format!("{} words", t.split_whitespace().count()),
                        true,
                    ),
                    Err(e) => (format!("Error: {e:#}"), "Failed".into(), false),
                };
                crate::engine::record_outcome(&mut self.state, p.id, &text, ok);
                crate::engine::set_tool_note(&mut self.state, p.chat, summary.clone(), &note);
                crate::engine::continue_with(
                    &mut self.state,
                    p.chat,
                    &p.model,
                    p.steps + 1,
                    format!("Tool result {} · {summary}\n{text}", p.call.id),
                    false,
                );
                format!("{summary}: {note}")
            }
            JobDone::UserSpeech {
                chat,
                text,
                path,
                result,
            } => match result.and_then(|wav| Ok(std::fs::write(&path, wav)?)) {
                Ok(()) => {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    crate::engine::add_exchange(
                        &mut self.state,
                        chat,
                        "Speech",
                        format!("/speak {text}"),
                        format!("Saved file: {name}"),
                        vec![name.clone()],
                    );
                    format!("Speech saved: {name}")
                }
                Err(e) => format!("Speech failed: {e:#}"),
            },
            JobDone::UserImage {
                chat,
                backend,
                command,
                path,
                result,
            } => {
                let saved = result.and_then(|image| {
                    std::fs::write(&path, &image.png)?;
                    Ok(image)
                });
                match saved {
                    Ok(image) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        crate::engine::add_exchange(
                            &mut self.state,
                            chat,
                            backend,
                            command,
                            format!(
                                "Made with {} in {} s.\n\nSaved file: {name}",
                                image.checkpoint, image.seconds
                            ),
                            vec![name.clone()],
                        );
                        format!("Image saved: {name}")
                    }
                    Err(e) => {
                        let notice = format!("Image failed: {e:#}");
                        crate::engine::add_exchange(
                            &mut self.state, chat, backend, command, notice.clone(), vec![],
                        );
                        notice
                    }
                }
            }
            JobDone::Packages(result) => {
                let (ok, text) = match result {
                    Ok(summary) => (
                        true,
                        format!("{summary}. They are available to scripts in the sandbox."),
                    ),
                    Err(e) => (false, format!("Installing failed: {e:#}")),
                };
                self.report_after(ok, text.clone());
                text
            }
            JobDone::Probe(report) => {
                let passed = report.checks.iter().filter(|c| c.passed).count();
                let notice = if report.ready {
                    format!(
                        "Sandbox check passed ({passed} of {}). Running code is on.",
                        report.checks.len()
                    )
                } else {
                    format!(
                        "Sandbox check: {passed} of {} passed. Running code stays off. {}",
                        report.checks.len(),
                        report.problem
                    )
                };
                self.state.sandbox = Some(report);
                let ready = ready(&self.state);
                self.report_after(
                    ready,
                    if ready {
                        "The sandbox is set up and running code is on. Continue the task now: create and run the script.".into()
                    } else {
                        format!(
                            "The sandbox check didn't pass, so code can't run. Failed checks: {}. Tell the user which checks failed, that the details are in Settings › Sandbox, and don't request setup again until they say they've changed something.",
                            self.state
                                .sandbox
                                .as_ref()
                                .map(|r| {
                                    let failed: Vec<String> = r.checks.iter().filter(|c| !c.passed).map(|c| format!("{} ({})", c.name, c.detail)).collect();
                                    if failed.is_empty() { r.problem.clone() } else { failed.join("; ") }
                                })
                                .unwrap_or_default()
                        )
                    },
                );
                notice
            }
        }
    }
}

async fn open_public(
    url: String,
    start: usize,
    diagnostics: bool,
    blocked: bool,
    browser_path: &str,
    cancel: Arc<AtomicBool>,
) -> Result<String> {
    ensure!(!blocked, "{}", crate::engine::WEB_ACCESS_BLOCKED);
    #[cfg(any(windows, target_os = "linux"))]
    {
        let exe = crate::browser::resolve(browser_path)?;
        let runtime = tokio::runtime::Handle::current();
        return tokio::task::spawn_blocking(move || {
            crate::browser::open_public(&exe, &url, start, diagnostics, &runtime, &cancel)
        }).await?;
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (url, start, diagnostics, browser_path, cancel);
        bail!("Public Chromium browsing is available on Windows and Linux");
    }
}

/// `name.png`, or `name-2.png` and so on if that file already exists.
fn unused(path: &std::path::Path) -> PathBuf {
    if !path.exists() {
        return path.to_owned();
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().unwrap_or_default().to_string_lossy();
    (2..)
        .map(|n| path.with_file_name(format!("{stem}-{n}.{ext}")))
        .find(|p| !p.exists())
        .unwrap()
}
