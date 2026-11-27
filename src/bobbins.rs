//! Local Bobbin profiles, assignments, and run records in the encrypted workspace store.
use crate::{
    automation,
    state::{Conversation, Mode, State, Task, ThreadInfo},
    tools::Call,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const MAX_TEXT: usize = 6_000;
const MIN_INTERVAL: i64 = 60;
const MAX_INTERVAL: i64 = 365 * 86_400;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    pub profiles: BTreeMap<String, Profile>,
    /// The local chat ID indexes the run so that tool dispatch and recovery use the same scope.
    pub runs: BTreeMap<u64, Run>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Picture {
    #[default]
    Initials,
    Spool,
    Shuttle,
    Loop,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Draft {
    pub name: String,
    pub purpose: String,
    pub instructions: String,
    pub model: String,
    pub project: Option<PathBuf>,
    pub project_id: Option<String>,
    pub read_project: bool,
    pub output_files: bool,
    pub web: bool,
    pub color: usize,
    pub picture: Picture,
    pub avatar: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub revision: u64,
    pub draft: Draft,
    pub paused: bool,
    pub archived: bool,
    pub pinned: bool,
    pub assignments: BTreeMap<String, Assignment>,
}

impl Profile {
    fn can_dispatch(&self) -> bool {
        !self.paused && !self.archived
    }

    fn pause_schedules(&mut self) {
        for schedule in self
            .assignments
            .values_mut()
            .filter_map(|a| a.schedule.as_mut())
        {
            schedule.enabled = false;
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct AssignmentDraft {
    pub title: String,
    pub prompt: String,
    pub done: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub draft: AssignmentDraft,
    pub schedule: Option<Schedule>,
}

impl Assignment {
    pub fn enabled_schedule(&self) -> Option<&Schedule> {
        self.schedule.as_ref().filter(|schedule| schedule.enabled)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub next: i64,
    pub interval: i64,
    pub enabled: bool,
    pub skip_missed: bool,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
    Interrupted,
}

impl Status {
    pub fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Done => "Completed",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
            Self::Interrupted => "Interrupted",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub bobbin: String,
    pub profile_revision: u64,
    pub configuration: Draft,
    pub assignment: Option<String>,
    pub title: String,
    pub completion: String,
    pub home: String,
    pub triggered: i64,
    pub missed: u64,
    pub started: Option<i64>,
    pub finished: Option<i64>,
    pub actual_model: Option<String>,
    pub status: Status,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_task: Option<Task>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Command {
    Save {
        id: Option<String>,
        revision: Option<u64>,
        draft: Draft,
    },
    Duplicate {
        id: String,
    },
    Pin {
        id: String,
        pinned: bool,
    },
    Pause {
        id: String,
        stop: bool,
    },
    Resume {
        id: String,
    },
    Archive {
        id: String,
    },
    Restore {
        id: String,
    },
    Delete {
        id: String,
        name: String,
    },
    Assign {
        id: String,
        assignment: Option<String>,
        revision: u64,
        draft: AssignmentDraft,
    },
    RemoveAssignment {
        id: String,
        assignment: String,
    },
    Schedule {
        id: String,
        assignment: String,
        revision: u64,
        next: i64,
        interval: i64,
        skip_missed: bool,
    },
    PauseSchedule {
        id: String,
        assignment: String,
    },
    Run {
        id: String,
        assignment: Option<String>,
        prompt: String,
    },
    Cancel {
        chat: u64,
    },
    Steer {
        chat: u64,
        text: String,
    },
}

impl Registry {
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty() && self.runs.is_empty()
    }

    pub fn live(&self, id: &str) -> impl Iterator<Item = (&u64, &Run)> {
        self.active_runs().filter(move |(_, run)| run.bobbin == id)
    }

    pub(crate) fn active_runs(&self) -> impl Iterator<Item = (&u64, &Run)> {
        self.runs.iter().filter(|(_, run)| run.status.active())
    }

    fn next_schedule(&self) -> Option<(&str, &str, &Schedule)> {
        let mut schedules = self
            .profiles
            .iter()
            .filter(|(_, profile)| profile.can_dispatch())
            .flat_map(|(id, profile)| {
                profile
                    .assignments
                    .iter()
                    .filter_map(move |(key, assignment)| {
                        assignment
                            .enabled_schedule()
                            .map(|schedule| (id.as_str(), key.as_str(), schedule))
                    })
            })
            .peekable();
        schedules.peek()?;
        let active_assignments: BTreeSet<_> = self
            .active_runs()
            .filter_map(|(_, run)| {
                run.assignment
                    .as_deref()
                    .map(|assignment| (run.bobbin.as_str(), assignment))
            })
            .collect();
        schedules
            .filter(|(id, key, _)| !active_assignments.contains(&(*id, *key)))
            .min_by_key(|(id, key, schedule)| (schedule.next, *id, *key))
    }
}

pub fn home() -> &'static str {
    if cfg!(target_os = "android") {
        "Android · This device"
    } else if cfg!(windows) {
        "Windows · This device"
    } else {
        "Linux · This device"
    }
}

fn id() -> Result<String> {
    Ok(crate::sync::core::hex(&crate::vault::random::<16>()?))
}

fn valid_text(s: &str, max: usize, required: bool) -> bool {
    s.len() <= max
        && (!required || !s.trim().is_empty())
        && !s
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
}

fn validate_profile(draft: &mut Draft) -> Result<()> {
    draft.name = draft.name.trim().to_owned();
    draft.purpose = draft.purpose.trim().to_owned();
    ensure!(
        valid_text(&draft.name, 80, true) && !draft.name.chars().any(char::is_control),
        "Enter a name up to 80 bytes on one line."
    );
    ensure!(
        valid_text(&draft.purpose, 240, true) && !draft.purpose.chars().any(char::is_control),
        "Enter a one-line purpose up to 240 bytes."
    );
    ensure!(
        valid_text(&draft.instructions, MAX_TEXT, false),
        "Instructions are too long or contain control characters."
    );
    ensure!(valid_text(&draft.model, 240, true), "Select a model.");
    ensure!(draft.color < 6, "Select an available avatar color.");
    if let Some(project) = &draft.project {
        draft.project = Some(
            crate::projects::existing(project)?
                .context("Select an existing Spindle project.")?
                .path,
        );
    }
    draft.project_id = draft
        .project
        .as_deref()
        .map(|path| crate::projects::ensure_sync_identity(path, None))
        .transpose()?;
    ensure!(
        !draft.read_project || draft.project.is_some(),
        "Select the project this Bobbin can read."
    );
    if let Some(asset) = &draft.avatar {
        ensure!(avatar_path(asset)?.is_file(), "Choose a saved avatar.");
    }
    Ok(())
}

fn validate_assignment(draft: &AssignmentDraft) -> Result<()> {
    ensure!(
        valid_text(&draft.title, 100, true) && !draft.title.chars().any(char::is_control),
        "Enter a short assignment title."
    );
    ensure!(
        valid_text(&draft.prompt, MAX_TEXT, true),
        "Enter assignment instructions up to 6,000 bytes."
    );
    ensure!(
        valid_text(&draft.done, 2_000, true),
        "Describe what a completed assignment should deliver."
    );
    Ok(())
}

pub fn avatar_path(asset: &str) -> Result<PathBuf> {
    ensure!(
        asset.len() == 64 && asset.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid Bobbin avatar ID."
    );
    Ok(crate::runtime::data_dir()
        .join("bobbin-avatars")
        .join(format!("{asset}.png")))
}

pub fn import_avatar(path: &Path) -> Result<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).context("Couldn't open the selected artwork.")?;
    let mut bytes = Vec::new();
    file.take(12 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    store_avatar(&bytes)
}

pub fn store_avatar(bytes: &[u8]) -> Result<String> {
    use std::io::{Cursor, Write};
    ensure!(
        bytes.len() <= 12 * 1024 * 1024,
        "Avatar artwork must be below 12 MB."
    );
    let mut decoder = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    ensure!(
        matches!(
            decoder.format(),
            Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
        ),
        "Choose PNG or JPEG artwork."
    );
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4_096);
    limits.max_image_height = Some(4_096);
    limits.max_alloc = Some(80 * 1024 * 1024);
    decoder.limits(limits);
    let png = decoder
        .decode()?
        .resize_to_fill(256, 256, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    let mut packed = Cursor::new(Vec::new());
    png.write_to(&mut packed, image::ImageFormat::Png)?;
    let hash = crate::sync::core::hex(
        ring::digest::digest(&ring::digest::SHA256, packed.get_ref()).as_ref(),
    );
    let path = avatar_path(&hash)?;
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    if !path.is_file() {
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(packed.get_ref())?;
        file.as_file().sync_all()?;
        if let Err(e) = file.persist_noclobber(&path) {
            ensure!(path.is_file(), "Couldn't save avatar: {}", e.error);
        }
    }
    Ok(hash)
}

fn profile<'a>(state: &'a mut State, id: &str) -> Result<&'a mut Profile> {
    state
        .bobbins
        .profiles
        .get_mut(id)
        .context("This Bobbin no longer exists.")
}

fn resolve_project(draft: &Draft) -> Result<()> {
    if let Some(path) = &draft.project {
        let project = crate::projects::existing(path)?
            .context("Restore the selected project or change the Bobbin’s project.")?;
        ensure!(
            project.path == *path,
            "The selected project moved. Review its folder in Bobbin settings."
        );
        ensure!(
            crate::projects::sync_identity(path)? == draft.project_id,
            "The selected folder now belongs to a different project. Review it in Bobbin settings."
        );
    }
    Ok(())
}

fn queue(
    state: &mut State,
    models: &[String],
    bobbin: &str,
    assignment: Option<&str>,
    prompt: Option<&str>,
    occurrence: i64,
    missed: u64,
) -> Result<u64> {
    let profile = state
        .bobbins
        .profiles
        .get(bobbin)
        .context("This Bobbin no longer exists.")?;
    ensure!(
        profile.can_dispatch(),
        "Resume the Bobbin before starting work."
    );
    let definition = assignment
        .map(|key| {
            profile
                .assignments
                .get(key)
                .context("This assignment no longer exists.")
        })
        .transpose()?;
    ensure!(
        !definition.is_some()
            || !state
                .bobbins
                .live(bobbin)
                .any(|(_, run)| run.assignment.as_deref() == assignment),
        "This assignment already has an active run."
    );
    ensure!(
        models.contains(&profile.draft.model),
        "The assigned model is unavailable. Check its connection or choose another model in Bobbin settings."
    );
    resolve_project(&profile.draft)?;
    let (text, title, completion) = match (definition, prompt.filter(|p| !p.trim().is_empty())) {
        (Some(a), None) => (
            a.draft.prompt.clone(),
            a.draft.title.clone(),
            a.draft.done.clone(),
        ),
        (None, Some(p)) => (
            p.trim().to_owned(),
            p.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(80)
                .collect(),
            String::new(),
        ),
        _ => anyhow::bail!("Enter a message or select a saved assignment."),
    };
    ensure!(
        valid_text(&text, MAX_TEXT, true),
        "Enter a message up to 6,000 bytes."
    );
    let revision = profile.revision;
    let draft = profile.draft.clone();
    let run_id = id()?;
    state.serial += 1;
    let chat = state.serial;
    state.chats.insert(
        chat,
        Conversation {
            thread: ThreadInfo {
                title: format!("{} · {title}", draft.name),
                started: true,
                model: draft.model.clone(),
                project: draft.project.clone(),
                section: "Bobbin runs".into(),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    state.serial += 1;
    state.queue.push_back(Task {
        steps: 0,
        tool_result: false,
        pastes: vec![],
        images: vec![],
        continuation: Default::default(),
        hold_reason: None,
        chain: vec![],
        disconnected: vec![],
        origin: Some(chat),
        schedule: None,
        actual: None,
        id: state.serial,
        text,
        model: draft.model.clone(),
        mode: Mode::Chat,
        context: String::new(),
        steering: vec![],
        skill: None,
    });
    state.bobbins.runs.insert(
        chat,
        Run {
            id: run_id,
            bobbin: bobbin.to_owned(),
            profile_revision: revision,
            configuration: draft,
            assignment: assignment.map(str::to_owned),
            title,
            completion,
            home: home().into(),
            triggered: occurrence,
            missed,
            started: None,
            finished: None,
            actual_model: None,
            status: Status::Queued,
            outcome: String::new(),
            held_task: None,
        },
    );
    Ok(chat)
}

pub fn start(state: &mut State, chat: u64, model: &str) {
    if let Some(run) = state
        .bobbins
        .runs
        .get_mut(&chat)
        .filter(|run| run.status.active())
    {
        run.started.get_or_insert_with(automation::now);
        run.actual_model = Some(model.into());
        run.status = Status::Running;
    }
}

pub fn finish(state: &mut State, chat: u64, status: Status, outcome: &str) {
    if let Some(run) = state
        .bobbins
        .runs
        .get_mut(&chat)
        .filter(|run| run.status.active())
    {
        run.status = status;
        run.finished = Some(automation::now());
        run.outcome = outcome.chars().take(360).collect();
        run.held_task = None;
        state.note_thread_update(chat);
    }
}

pub fn recover(state: &mut State) {
    let pending: BTreeSet<_> = state
        .queue
        .iter()
        .filter_map(|task| task.origin)
        .chain(state.requests.iter().map(|request| request.chat))
        .collect();
    let interrupted: Vec<_> = state
        .bobbins
        .active_runs()
        .filter(|(chat, run)| run.held_task.is_none() && !pending.contains(chat))
        .map(|(chat, _)| *chat)
        .collect();
    for chat in interrupted {
        finish(
            state,
            chat,
            Status::Interrupted,
            "Spindle closed before the run recorded a result. Review the chat before retrying.",
        );
    }
}

pub fn next(state: &State) -> Option<i64> {
    state
        .bobbins
        .next_schedule()
        .map(|(_, _, schedule)| schedule.next)
}

pub fn due(state: &mut State, models: &[String], now: i64) -> Option<String> {
    if cfg!(target_os = "android") || state.held || !state.queue.is_empty() {
        return None;
    }
    let (bobbin, assignment, schedule) = state.bobbins.next_schedule()?;
    let when = schedule.next;
    if when > now {
        return None;
    }
    let (bobbin, assignment) = (bobbin.to_owned(), assignment.to_owned());
    let missed = if schedule.interval > 0 {
        (now - when) / schedule.interval
    } else {
        0
    };
    let skip = schedule.skip_missed && now - (when + missed * schedule.interval) >= MIN_INTERVAL;
    let result = if skip {
        Ok(None)
    } else {
        queue(
            state,
            models,
            &bobbin,
            Some(&assignment),
            None,
            when + missed * schedule.interval,
            missed as u64,
        )
        .map(Some)
    };
    let saved = state
        .bobbins
        .profiles
        .get_mut(&bobbin)
        .unwrap()
        .assignments
        .get_mut(&assignment)
        .unwrap()
        .schedule
        .as_mut()
        .unwrap();
    saved.last_error = result.as_ref().err().map(|err| err.to_string());
    if saved.interval == 0 || result.is_err() {
        saved.enabled = false;
    } else {
        saved.next = when + (missed + 1) * saved.interval;
    }
    match result {
        Ok(Some(chat)) => Some(format!("Bobbin run queued in chat #{chat}")),
        Ok(None) => Some("Skipped a missed Bobbin run".into()),
        Err(e) => Some(format!("Bobbin schedule paused: {e:#}")),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Project,
    Output,
    Web,
}

impl Access {
    fn file_path(self, call: &Call) -> Option<&str> {
        let path = call.raw_path();
        match self {
            Self::Project => Some(path),
            Self::Output if matches!(path, "outputs" | "outputs/") => Some("."),
            Self::Output => Some(path.strip_prefix("outputs/").unwrap_or(path)),
            Self::Web => None,
        }
    }
}

pub(crate) struct RunScope {
    pub read_project: bool,
    pub output_files: bool,
    pub web: bool,
}

impl Registry {
    pub(crate) fn scope(&self, run: &Run, block_web: bool) -> Option<RunScope> {
        let current = &self.profiles.get(&run.bobbin)?.draft;
        let granted = &run.configuration;
        Some(RunScope {
            read_project: granted.read_project
                && current.read_project
                && granted.project == current.project,
            output_files: granted.output_files && current.output_files,
            web: granted.web && current.web && !block_web,
        })
    }
}

pub fn visible_file(part: &str) -> bool {
    let name = part.to_ascii_lowercase();
    !name.starts_with('.')
        && !name.starts_with("_spindle")
        && !name.ends_with(".key")
        && !name.ends_with(".pem")
        && !name.ends_with(".pfx")
        && !name.starts_with("id_rsa")
}

pub fn validate_path(root: &Path, call: &Call, access: Access) -> Result<PathBuf> {
    let path = access
        .file_path(call)
        .context("This Bobbin tool does not open files.")?;
    if access == Access::Output {
        ensure!(
            path == "."
                || (!path.trim().is_empty()
                    && !path.starts_with(['.', '-'])
                    && !path.contains(['/', '\\', ':'])),
            "Choose one filename for this run’s output."
        );
    }
    let (target, inside) = crate::tools::resolve(root, path)?;
    ensure!(
        inside,
        "The path must stay inside this run’s approved folder."
    );
    let root = root.canonicalize()?;
    let rel = target.strip_prefix(&root)?;
    ensure!(
        rel.iter().all(|part| visible_file(&part.to_string_lossy())),
        "Hidden files and credential files are outside Bobbin access."
    );
    Ok(target)
}

pub fn authorize(state: &State, chat: u64, call: &Call) -> Result<Option<Access>> {
    let Some(run) = state.bobbins.runs.get(&chat) else {
        return Ok(None);
    };
    ensure!(run.status.active(), "This Bobbin run has ended.");
    let scope = state
        .bobbins
        .scope(run, state.preferences.block_web)
        .context("This Bobbin was deleted.")?;
    let access = match call {
        Call::List { .. } | Call::Read { .. } | Call::Search { .. }
            if scope.output_files
                && (call.raw_path() == "outputs" || call.raw_path().starts_with("outputs/")) =>
        {
            Access::Output
        }
        Call::List { .. } | Call::Read { .. } | Call::Search { .. } if scope.read_project => {
            resolve_project(&run.configuration)?;
            Access::Project
        }
        Call::List { .. }
        | Call::Read { .. }
        | Call::Search { .. }
        | Call::Create { .. }
        | Call::Patch { .. }
            if scope.output_files =>
        {
            Access::Output
        }
        Call::WebSearch { .. } | Call::WebOpen { .. } if scope.web => Access::Web,
        _ => anyhow::bail!(
            "This action is outside the current Bobbin run’s approved access. Report the limitation in the result."
        ),
    };
    Ok(Some(access))
}

impl crate::engine::Engine {
    fn cancel_bobbin_run(&mut self, chat: u64) -> Result<()> {
        ensure!(
            self.state
                .bobbins
                .runs
                .get(&chat)
                .is_some_and(|run| run.status.active()),
            "This Bobbin run is no longer active."
        );
        let front = self
            .state
            .queue
            .front()
            .filter(|task| task.origin == Some(chat))
            .map(|task| task.id);
        self.state.queue.retain(|task| task.origin != Some(chat));
        self.state.requests.retain(|pending| pending.chat != chat);
        if self.jobs.catalog_chat == Some(chat) {
            self.jobs
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if front.is_some()
            && self
                .state
                .automation
                .wake
                .as_ref()
                .is_some_and(|wake| Some(wake.task) == front)
        {
            self.state.automation.wake = None;
            self.state.held = false;
        }
        finish(
            &mut self.state,
            chat,
            Status::Cancelled,
            "Stopped by you. Prior outputs remain in the run chat.",
        );
        Ok(())
    }

    pub(crate) fn bobbin_command(&mut self, arguments: &str) -> Result<String> {
        if arguments.trim().is_empty() {
            return Ok(self
                .state
                .bobbins
                .profiles
                .values()
                .map(|p| p.draft.name.as_str())
                .collect::<Vec<_>>()
                .join("\n"));
        }
        let command: Command = serde_json::from_str(arguments).context("Invalid Bobbin action")?;
        let s = &mut self.state;
        match command {
            Command::Save {
                id: key,
                revision,
                mut draft,
            } => {
                validate_profile(&mut draft)?;
                if let Some(key) = key {
                    let saved = profile(s, &key)?;
                    ensure!(
                        revision == Some(saved.revision),
                        "This Bobbin changed. Reopen its settings to review the latest version."
                    );
                    let old = &saved.draft;
                    if old.model != draft.model
                        || old.instructions != draft.instructions
                        || old.purpose != draft.purpose
                        || old.project != draft.project
                        || old.read_project != draft.read_project
                        || old.output_files != draft.output_files
                        || old.web != draft.web
                    {
                        saved.pause_schedules();
                    }
                    saved.draft = draft;
                    saved.revision += 1;
                } else {
                    s.bobbins.profiles.insert(
                        id()?,
                        Profile {
                            draft,
                            revision: 1,
                            paused: false,
                            archived: false,
                            pinned: false,
                            assignments: BTreeMap::new(),
                        },
                    );
                }
                Ok("Bobbin saved".into())
            }
            Command::Duplicate { id: key } => {
                let mut duplicate = profile(s, &key)?.clone();
                duplicate.draft.name = format!(
                    "{} copy",
                    duplicate.draft.name.chars().take(60).collect::<String>()
                );
                duplicate.draft.project = None;
                duplicate.draft.project_id = None;
                duplicate.draft.read_project = false;
                duplicate.draft.output_files = false;
                duplicate.draft.web = false;
                duplicate.revision = 1;
                duplicate.pinned = false;
                duplicate.paused = false;
                duplicate.archived = false;
                for a in duplicate.assignments.values_mut() {
                    a.schedule = None;
                }
                s.bobbins.profiles.insert(id()?, duplicate);
                Ok("Bobbin duplicated as a draft".into())
            }
            Command::Pin { id, pinned } => {
                profile(s, &id)?.pinned = pinned;
                Ok("Bobbin pin updated".into())
            }
            Command::Pause { id, stop } => {
                profile(s, &id)?.paused = true;
                let chats: Vec<_> = s
                    .bobbins
                    .live(&id)
                    .filter(|(_, run)| stop || run.started.is_none())
                    .map(|(chat, _)| *chat)
                    .collect();
                if stop {
                    for chat in chats {
                        self.cancel_bobbin_run(chat)?;
                    }
                } else {
                    for chat in chats {
                        if let Some(index) =
                            s.queue.iter().position(|task| task.origin == Some(chat))
                        {
                            let task = s.queue.remove(index).unwrap();
                            if index == 0
                                && s.automation
                                    .wake
                                    .as_ref()
                                    .is_some_and(|wake| wake.task == task.id)
                            {
                                s.automation.wake = None;
                                s.held = false;
                            }
                            s.bobbins.runs.get_mut(&chat).unwrap().held_task = Some(task);
                        }
                    }
                }
                Ok("Bobbin paused".into())
            }
            Command::Resume { id } => {
                let p = profile(s, &id)?;
                ensure!(!p.archived, "Restore the Bobbin before resuming it.");
                ensure!(
                    self.models.contains(&p.draft.model),
                    "The assigned model is unavailable. Check the connection in Settings."
                );
                resolve_project(&p.draft)?;
                p.paused = false;
                for run in s.bobbins.runs.values_mut().filter(|r| r.bobbin == id) {
                    if let Some(task) = run.held_task.take() {
                        s.queue.push_back(task);
                    }
                }
                Ok("Bobbin resumed".into())
            }
            Command::Archive { id } => {
                let p = profile(s, &id)?;
                p.archived = true;
                p.paused = true;
                p.pause_schedules();
                let chats: Vec<_> = s.bobbins.live(&id).map(|(chat, _)| *chat).collect();
                for chat in chats {
                    self.cancel_bobbin_run(chat)?;
                }
                Ok("Bobbin archived".into())
            }
            Command::Restore { id } => {
                profile(s, &id)?.archived = false;
                Ok("Bobbin restored in paused state".into())
            }
            Command::Delete { id, name } => {
                let p = profile(s, &id)?;
                ensure!(
                    p.archived && p.draft.name == name,
                    "Archive the Bobbin and confirm its exact name. Run chats will be kept."
                );
                ensure!(
                    s.bobbins.live(&id).next().is_none(),
                    "Wait for active work to stop."
                );
                s.bobbins.profiles.remove(&id);
                Ok("Bobbin deleted. Run chats were kept.".into())
            }
            Command::Assign {
                id: key,
                assignment,
                revision,
                draft,
            } => {
                validate_assignment(&draft)?;
                let p = profile(s, &key)?;
                ensure!(
                    !p.archived && p.revision == revision,
                    "Reopen the Bobbin to edit the latest assignment."
                );
                let key = match assignment {
                    Some(key) => {
                        ensure!(
                            p.assignments.contains_key(&key),
                            "This assignment no longer exists."
                        );
                        key
                    }
                    None => id()?,
                };
                let schedule = p
                    .assignments
                    .get(&key)
                    .and_then(|a| a.schedule.clone())
                    .map(|mut s| {
                        s.enabled = false;
                        s
                    });
                p.assignments.insert(key, Assignment { draft, schedule });
                p.revision += 1;
                Ok("Assignment saved".into())
            }
            Command::RemoveAssignment { id, assignment } => {
                let p = profile(s, &id)?;
                p.assignments
                    .remove(&assignment)
                    .context("This assignment no longer exists.")?;
                p.revision += 1;
                Ok("Assignment removed".into())
            }
            Command::Schedule {
                id,
                assignment,
                revision,
                next,
                interval,
                skip_missed,
            } => {
                ensure!(
                    !cfg!(target_os = "android"),
                    "Bobbin schedules run on Windows or Linux."
                );
                ensure!(
                    next > automation::now() && next <= automation::now() + MAX_INTERVAL,
                    "Choose a future start within the next year."
                );
                ensure!(
                    interval == 0 || (MIN_INTERVAL..=MAX_INTERVAL).contains(&interval),
                    "Choose a valid repeat interval."
                );
                let p = profile(s, &id)?;
                ensure!(
                    p.can_dispatch() && p.revision == revision,
                    "Review the active Bobbin and its latest assignment."
                );
                ensure!(
                    self.models.contains(&p.draft.model),
                    "Connect the assigned model before enabling a schedule."
                );
                resolve_project(&p.draft)?;
                let a = p
                    .assignments
                    .get_mut(&assignment)
                    .context("This assignment no longer exists.")?;
                a.schedule = Some(Schedule {
                    next,
                    interval,
                    enabled: true,
                    skip_missed,
                    last_error: None,
                });
                p.revision += 1;
                Ok(format!(
                    "Schedule enabled for {}",
                    automation::local_time(next)
                ))
            }
            Command::PauseSchedule { id, assignment } => {
                let p = profile(s, &id)?;
                let schedule = p
                    .assignments
                    .get_mut(&assignment)
                    .and_then(|a| a.schedule.as_mut())
                    .context("This schedule no longer exists.")?;
                schedule.enabled = false;
                p.revision += 1;
                Ok("Assignment schedule paused".into())
            }
            Command::Run {
                id,
                assignment,
                prompt,
            } => {
                let chat = queue(
                    s,
                    &self.models,
                    &id,
                    assignment.as_deref(),
                    Some(&prompt),
                    automation::now(),
                    0,
                )?;
                Ok(format!("Bobbin run queued in chat #{chat}"))
            }
            Command::Cancel { chat } => {
                self.cancel_bobbin_run(chat)?;
                Ok("Bobbin run cancelled".into())
            }
            Command::Steer { chat, text } => {
                ensure!(
                    valid_text(&text, 2_000, true),
                    "Enter a correction up to 2,000 bytes."
                );
                ensure!(
                    s.bobbins.runs.get(&chat).is_some_and(|r| r.status.active()),
                    "This run is no longer active."
                );
                let index = s
                    .queue
                    .iter()
                    .position(|q| q.origin == Some(chat))
                    .context("Steer once this run returns to the queue.")?;
                let q = &s.queue[index];
                ensure!(
                    q.steering.len() < 8,
                    "This run has reached its correction limit."
                );
                s.serial += 1;
                let q = &mut s.queue[index];
                q.id = s.serial;
                q.steering.push(text);
                q.continuation = Default::default();
                Ok("Bobbin correction queued".into())
            }
        }
    }
}
