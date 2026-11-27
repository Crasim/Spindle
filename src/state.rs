use crate::ollama::Message;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Worker {
    pub role: String,
    pub model: String,
    pub answer: Option<String>,
    #[serde(default)]
    pub corrections: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum Mode {
    /// Historical wire name for a tool-enabled turn.
    Chat,
    Conversation,
    Plan(u64),
    Build(String),
    Swarm(Vec<Worker>),
    Council(Vec<Worker>),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Task {
    /// Tool actions already taken in this run of work, counting toward the limit.
    #[serde(default, skip_serializing_if = "no_steps")]
    pub steps: u32,
    /// This message carries a tool result back to the model.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tool_result: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pastes: Vec<Attachment>,
    /// Base64 images attached to this message for a vision model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
    #[serde(default)]
    pub continuation: crate::generation::Continuation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_reason: Option<String>,
    #[serde(default)]
    pub chain: Vec<String>,
    /// Only skip these once-disconnected models for this queued request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disconnected: Vec<String>,
    #[serde(default)]
    pub origin: Option<u64>,
    #[serde(default)]
    pub schedule: Option<u64>,
    #[serde(default)]
    pub actual: Option<String>,
    pub id: u64,
    pub text: String,
    pub model: String,
    pub mode: Mode,
    pub context: String,
    pub steering: Vec<String>,
    #[serde(default)]
    pub skill: Option<(String, String)>,
}

fn default_zoom() -> u16 {
    100
}

fn no_steps(n: &u32) -> bool {
    *n == 0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolNote {
    pub summary: String,
    pub status: String,
}

/// A tool call waiting for the user's decision (`approved` false) or approved and waiting to
/// run (`approved` true).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pending {
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub remote: Option<crate::sync::remote::Request>,
    /// Keep the result in ordinary Chat, with browser reads as its only tool.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub chat_read_only: bool,
    pub id: u64,
    pub chat: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
    /// The folder tools work in.
    pub root: PathBuf,
    pub call: crate::tools::Envelope,
    pub risk: crate::policy::Risk,
    pub target: PathBuf,
    pub model: String,
    pub trust: crate::policy::Trust,
    pub steps: u32,
    pub created: i64,
    #[serde(default)]
    pub approved: bool,
    /// How it was allowed, for the log.
    #[serde(default)]
    pub reason: String,
    /// What the action would change, when the call itself doesn't show it (applying changes).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preview: String,
}

impl Pending {
    pub(crate) fn confirmed_change(&self) -> bool {
        self.risk == crate::policy::Risk::Destructive && self.approved && self.reason == "Allowed once by you"
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionRecord {
    pub id: u64,
    pub time: i64,
    pub chat: u64,
    pub model: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool: String,
    pub summary: String,
    pub risk: crate::policy::Risk,
    pub decision: String,
    /// Empty until the action finishes; then a short result or the error.
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub ok: bool,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub path: String,
    pub text: String,
}

#[derive(Clone)]
pub struct BrowserRead {
    pub id: u64,
    pub destination: Option<reqwest::Url>,
    pub approved_origin: Option<String>,
    pub locating: Option<std::time::Instant>,
    pub model: String,
    pub steps: u32,
    pub call: crate::tools::Envelope,
    pub read_only_chat: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default, skip_serializing_if = "crate::bobbins::Registry::is_empty")]
    pub bobbins: crate::bobbins::Registry,
    #[serde(default, skip_serializing_if = "crate::sync::placement::Placement::is_empty")]
    pub placement: crate::sync::placement::Placement,
    #[serde(skip)]
    pub browser_session: Option<crate::browser_link::Paired>,
    #[serde(skip)]
    pub browser_reads: BTreeMap<u64, BrowserRead>,
    #[serde(default)]
    pub unread: std::collections::BTreeSet<u64>,
    #[serde(skip)]
    pub viewed_chat: Option<u64>,
    /// A short-lived country result from the OS location provider. It is never saved or synced.
    #[serde(skip)]
    pub located_web_country: Option<(String, std::time::Instant)>,
    #[serde(default)]
    pub received_threads: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub transfer_namespace: Option<[u8; 16]>,
    #[serde(default)]
    pub thread: ThreadInfo,
    #[serde(default)]
    pub sync: Option<crate::sync::Pairing>,
    /// Sync across devices (conversations, projects, settings…): this device's identity,
    /// switches, transports and maps. Records themselves live in the data folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<crate::sync::adapter::SyncState>,
    /// Local project and chat bindings kept when this device leaves a sync space.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub detached_spaces: BTreeMap<String, crate::sync::adapter::DetachedSpace>,
    #[serde(skip)]
    pub sync_status: String,
    #[serde(default)]
    pub automation: crate::automation::Automation,
    #[serde(default)]
    pub council: Vec<String>,
    #[serde(default)]
    pub providers: Vec<crate::providers::Provider>,
    /// The last successful Ollama catalog for this device's configured endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ollama_catalog: Option<(String, Vec<String>)>,
    #[serde(default)]
    pub preferences: Preferences,
    #[serde(default)]
    pub favorites: Vec<String>,
    /// Models hidden from the picker. They stay connected and usable by existing work.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub hidden_models: std::collections::BTreeSet<String>,
    /// Personal and per-project memories (see `memory.rs`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memories: Vec<crate::memory::Memory>,
    /// The latest sandbox check; running code is on only when it passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<crate::sandbox::Report>,
    /// Standing permissions the user gave (see `policy.rs`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grants: Vec<crate::policy::Grant>,
    /// Tool calls waiting for the user's decision, and approved calls waiting to run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<Pending>,
    /// Every tool action: what was asked, what was decided and what happened. Newest last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionRecord>,
    /// Local troubleshooting entries. Sync transports send conversation records separately.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<crate::diagnostics::Failure>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sync_diagnostics: Vec<crate::diagnostics::SyncEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic_capture: Option<crate::diagnostics::Capture>,
    /// Projects whose memory is turned off.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub memory_off: std::collections::BTreeSet<PathBuf>,
    /// Money budget per paid API provider. Missing entries use the $50-a-day default.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub budgets: BTreeMap<String, crate::meter::Budget>,
    /// Estimated spending per provider: (time, dollars), about six weeks kept.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub spend: BTreeMap<String, Vec<(i64, f64)>>,
    /// Credits left with each provider that reports them. Refreshed, not saved.
    #[serde(skip)]
    pub balances: BTreeMap<String, f64>,
    /// Advanced per-model settings, keyed by qualified model ID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tuning: BTreeMap<String, Tuning>,
    #[serde(default)]
    pub chats: BTreeMap<u64, Conversation>,
    #[serde(default)]
    pub chat_id: u64,
    pub model: String,
    pub history: Vec<Message>,
    pub queue: VecDeque<Task>,
    pub held: bool,
    pub serial: u64,
    pub goal: String,
    pub plan: Option<(u64, String)>,
    pub pending: Option<Attachment>,
    pub attachment: Option<Attachment>,
    pub drafts: BTreeMap<String, String>,
    pub skills: BTreeMap<String, String>,
}

/// How much a reasoning model should think before answering.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Thinking {
    #[default]
    Default,
    Off,
    Low,
    Medium,
    High,
}
impl Thinking {
    pub const ALL: [Thinking; 5] = [
        Thinking::Default,
        Thinking::Off,
        Thinking::Low,
        Thinking::Medium,
        Thinking::High,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Thinking::Default => "Model default",
            Thinking::Off => "Off",
            Thinking::Low => "Low",
            Thinking::Medium => "Medium",
            Thinking::High => "High",
        }
    }
    /// The effort name used by OpenAI-style and Ollama APIs.
    pub fn effort(self) -> Option<&'static str> {
        match self {
            Thinking::Low => Some("low"),
            Thinking::Medium => Some("medium"),
            Thinking::High => Some("high"),
            _ => None,
        }
    }
}

/// Advanced settings for one model. Every field defaults to "let the model decide".
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Tuning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<crate::compaction::Policy>,
    #[serde(default, skip_serializing_if = "is_default_thinking")]
    pub thinking: Thinking,
    /// An exact thinking value the model reported (Ollama /api/show), used instead of
    /// `thinking` when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Price override in dollars per million input and output tokens, for providers whose
    /// catalogs don't publish prices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_in: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_out: Option<f64>,
    /// Context window to request from a local Ollama model, in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    /// How far this model is trusted to act on the computer; `None` uses its size and age.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<crate::policy::Trust>,
}
fn is_default_thinking(t: &Thinking) -> bool {
    *t == Thinking::Default
}
impl Tuning {
    pub fn validate(&self) -> Result<()> {
        if let Some(p) = &self.compaction {
            p.validate()?;
        }
        if let Some(t) = self.temperature {
            ensure!(
                (0.0..=2.0).contains(&t),
                "Temperature must be between 0 and 2"
            );
        }
        if let Some(c) = self.context {
            ensure!(
                (2_048..=1_048_576).contains(&c),
                "Context window must be between 2,048 and 1,048,576 tokens"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Preferences {
    #[serde(default)]
    pub compaction: crate::compaction::Policy,
    #[serde(default)]
    pub compact_sidebar: bool,
    /// Width the user dragged the sidebar to; `None` uses the default for the window size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<u16>,
    pub enter_sends: bool,
    #[serde(default = "crate::memory::enabled")]
    pub enter_steers: bool,
    #[serde(default)]
    pub nerd_stats: bool,
    #[serde(default)]
    pub over_18: bool,
    pub text_size: u8,
    /// Interface zoom in percent, changed with Ctrl+Scroll or Ctrl+Plus/Minus.
    #[serde(default = "default_zoom")]
    pub zoom: u16,
    #[serde(default)]
    pub accent: crate::widgets::Accent,
    #[serde(default)]
    pub theme: crate::widgets::Theme,
    /// Use and save memories in chats.
    #[serde(default = "crate::memory::enabled")]
    pub memory: bool,
    /// Personal memories, used in every chat.
    #[serde(default = "crate::memory::enabled")]
    pub personal_memory: bool,
    /// Project memories, for every project unless turned off for one.
    #[serde(default = "crate::memory::enabled")]
    pub project_memory: bool,
    /// Apply the model's sandbox edits to the project when a reply finishes, with Undo,
    /// instead of asking each time.
    #[serde(default = "crate::memory::enabled")]
    pub auto_apply: bool,
    /// Block network access for the model browser while allowing local page tests.
    #[serde(default)]
    pub block_web: bool,
    /// Country explicitly shared with models for regional web tasks on this device.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub web_country: String,
    /// Whether the user allows Spindle to use this device's OS location service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_os_location: Option<bool>,
    /// This device's preferred Chromium executable. Empty uses browser discovery.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub browser_path: String,
    #[serde(default)]
    pub browser_allowlist: Vec<String>,
    #[serde(default)]
    pub browser_blocklist: Vec<String>,
    #[serde(default)]
    pub browser_mode: crate::browser_link::Mode,
    /// Browser access preferences belong to this workspace on this device.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_browsers: Vec<crate::browser::LinkBrowser>,
    /// Local app control permissions for this workspace.
    #[serde(default)]
    pub app_control: crate::computer_use::Access,
    /// ComfyUI address for /image and the image tool, such as http://localhost:8188.
    #[serde(default)]
    pub comfyui: String,
    /// Where Ollama runs, such as http://model-host.local:11434; empty means this
    /// computer (or SPINDLE_OLLAMA).
    #[serde(default)]
    pub ollama: String,
    /// Whether Ollama is one of the providers. On by default on a computer (where Ollama
    /// usually runs), off on a phone; Settings › Model providers adds or removes it.
    #[serde(default = "ollama_default")]
    pub ollama_on: bool,
    /// Where projects from other devices are kept on this one (asked the first time sync
    /// brings one). Empty: not chosen yet, so they wait.
    #[serde(default)]
    pub projects_dir: String,
    /// Checkpoint for the built-in image workflow; empty picks an SDXL one.
    #[serde(default)]
    pub comfy_checkpoint: String,
    /// A workflow in ComfyUI's API format to use instead of the built-in one.
    #[serde(default)]
    pub comfy_workflow: String,
    /// ComfyUI API workflow with an input image, used for /image edit.
    #[serde(default)]
    pub comfy_edit_workflow: String,
    #[serde(default)]
    pub sd_webui: String,
    #[serde(default)]
    pub image_backend: crate::comfy::Backend,
    #[serde(default)]
    pub image_aspect_ratio: String,
    #[serde(default)]
    pub image_transparent: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comfy_profiles: Vec<crate::comfy::Workflow>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comfy_profile: String,
    /// Speech server (OpenAI-compatible audio API); empty uses the Ollama server.
    #[serde(default)]
    pub speech_url: String,
    /// API key for the speech server, if it needs one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub speech_key: String,
    /// Text-to-speech model; empty picks one Ollama reports as able to speak.
    #[serde(default)]
    pub tts_model: String,
    #[serde(default)]
    pub tts_voice: String,
    /// Speech-to-text model; empty picks one Ollama reports as hearing audio.
    #[serde(default)]
    pub stt_model: String,
    /// Read each finished reply aloud.
    #[serde(default)]
    pub read_aloud: bool,
    /// Run in the background (starts at sign-in; closing the window hides it to the tray).
    #[serde(default)]
    pub background: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_on_sign_in: Option<bool>,
    /// Who speaks: "" for the OpenAI-style server above (Ollama by default), "fish" for
    /// Fish Audio.
    #[serde(default)]
    pub tts_provider: String,
    /// Who transcribes: "" or "fish", as above.
    #[serde(default)]
    pub stt_provider: String,
    /// Fish Audio API key, server (empty for api.fish.audio), models and voice.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fish_key: String,
    #[serde(default)]
    pub fish_url: String,
    #[serde(default)]
    pub fish_tts_model: String,
    /// A Fish Audio voice (its model id); empty uses Fish Audio's default voice.
    #[serde(default)]
    pub fish_voice: String,
    #[serde(default)]
    pub fish_voice_name: String,
    #[serde(default)]
    pub fish_stt_model: String,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            compaction: Default::default(),
            compact_sidebar: false,
            sidebar_width: None,
            enter_sends: true,
            enter_steers: true,
            nerd_stats: false,
            over_18: false,
            text_size: 16,
            zoom: 100,
            accent: Default::default(),
            theme: Default::default(),
            memory: true,
            personal_memory: true,
            project_memory: true,
            auto_apply: true,
            block_web: false,
            web_country: String::new(),
            use_os_location: None,
            browser_path: String::new(),
            browser_allowlist: vec![],
            browser_blocklist: vec![],
            browser_mode: Default::default(),
            disabled_browsers: vec![],
            app_control: Default::default(),
            comfyui: String::new(),
            ollama: String::new(),
            ollama_on: ollama_default(),
            projects_dir: String::new(),
            comfy_checkpoint: String::new(),
            image_aspect_ratio: String::new(),
            image_transparent: false,
            comfy_profiles: vec![],
            comfy_profile: String::new(),
            comfy_workflow: String::new(),
            comfy_edit_workflow: String::new(),
            sd_webui: String::new(),
            image_backend: crate::comfy::Backend::default(),
            speech_url: String::new(),
            speech_key: String::new(),
            tts_model: String::new(),
            tts_voice: String::new(),
            stt_model: String::new(),
            read_aloud: false,
            background: false,
            start_on_sign_in: None,
            tts_provider: String::new(),
            stt_provider: String::new(),
            fish_key: String::new(),
            fish_url: String::new(),
            fish_tts_model: String::new(),
            fish_voice: String::new(),
            fish_voice_name: String::new(),
            fish_stt_model: String::new(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Conversation {
    #[serde(default)]
    pub thread: ThreadInfo,
    pub history: Vec<Message>,
    pub goal: String,
    pub plan: Option<(u64, String)>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PromptVersion {
    pub group: String,
    pub number: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ThreadInfo {
    /// Stable identity of a conversation copied from another local application.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    /// Prompt position → shared identifier for independently saved versions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub prompt_versions: BTreeMap<usize, PromptVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_family: Option<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub edited_messages: std::collections::BTreeSet<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<crate::compaction::Policy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<crate::compaction::Summary>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub started: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub section: String,
    /// Files tools made in this chat's own folder since the last reply, shown as file cards on
    /// the next finished reply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub produced: Vec<String>,
    /// Changes in this conversation's sandbox copy not yet applied to the project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sandbox_changes: Vec<String>,
    /// User-role messages that carry tool results back to the model.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub tool_results: std::collections::BTreeSet<usize>,
    /// Files the model changed since it last checked its work (ran, tested or opened them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unchecked: Vec<String>,
    /// Replies Spindle has continued automatically toward the conversation's goal.
    #[serde(default, skip_serializing_if = "is_zero32")]
    pub goal_runs: u32,
    /// The model said the goal is met.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub goal_done: bool,
    /// Where earlier messages were compacted for the model: message position at the time →
    /// exchanges the summary covered. Shown as a divider in the conversation.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub compactions: std::collections::BTreeMap<usize, usize>,
    /// The living plan for this conversation (see `plan.rs`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<crate::plan::Step>,
    /// User amendments to the active plan, in order of precedence (latest last).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plan_changes: Vec<String>,
    /// Corrections entered while a tool is working. The next model step takes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_steering: Vec<String>,
    /// Plan edits made by the user, keyed by the number of messages at the time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plan_notes: BTreeMap<usize, crate::plan::Update>,
    /// Project folder this conversation belongs to; `None` is a standalone chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
    pub title: String,
    pub model: String,
    /// A model selection shown at the end of the chat until its next matching reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<ModelChange>,
    pub turns: BTreeMap<usize, Turn>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelChange {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    /// A status from Spindle after a run failed, rather than text from the model.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
    /// The tool call this reply made, in the taught format, so the model sees it again in
    /// the history (the reply text shown keeps it out).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// Edits Spindle applied to the project when this reply finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied: Option<Applied>,
    /// The tool this reply called, with what happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolNote>,
    /// How this reply changed the conversation's plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<crate::plan::Update>,
    /// Memories this reply saved, so the desktop can offer Undo.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remembered: Vec<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<Attachment>,
    /// Workspace HTML previews published by a tool, with a fallback when the files are absent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub html_previews: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pastes: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_budget: Option<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub earlier_output: u64,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_selection: Option<ModelChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_from: Option<String>,
    pub input: u64,
    pub output: u64,
    pub tokens_per_second: Option<f64>,
    pub window: Option<u64>,
    pub files: Vec<String>,
}

/// An automatic apply of the sandbox copy to the project, which the user can undo.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    pub summary: String,
    pub diff: String,
    /// Folder holding the project's earlier versions of the files.
    pub keep: std::path::PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undone: Option<String>,
}

fn is_zero32(n: &u32) -> bool {
    *n == 0
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl ThreadInfo {
    pub fn select_model(&mut self, previous: &str, selected: &str) {
        if (self.started || !self.turns.is_empty()) && !previous.is_empty() && previous != selected {
            let from = self.model_selection.take().map_or_else(|| previous.to_owned(), |change| change.from);
            self.model_selection = (from != selected).then(|| ModelChange { from, to: selected.to_owned() });
        }
        self.model = selected.to_owned();
    }

    pub fn take_model_selection(&mut self, requested: &str) -> Option<ModelChange> {
        if self.model_selection.as_ref().is_some_and(|change| change.to == requested) {
            self.model_selection.take()
        } else {
            None
        }
    }

    pub fn label<'a>(&'a self, history: &'a [Message]) -> &'a str {
        if self.title.is_empty() {
            history
                .first()
                .and_then(|m| m.content.lines().next())
                .unwrap_or("New chat")
        } else {
            &self.title
        }
    }
}

impl Task {
    pub fn fallback_from(&self, actual: &str) -> Option<&str> {
        (actual != self.model && self.chain.iter().any(|model| model == actual))
            .then_some(self.model.as_str())
    }
}

pub const APK_SKILL_NAME: &str = "android-apk";
pub const APK_SKILL: &str = include_str!("../skills/android-apk/SKILL.md");
pub const BROWSER_SKILL_NAME: &str = "browser-use";
pub const BROWSER_SKILL: &str = include_str!("../skills/browser-use/SKILL.md");
pub const BUILTIN_SKILLS: [(&str, &str); 3] = [
    (APK_SKILL_NAME, APK_SKILL),
    (BROWSER_SKILL_NAME, BROWSER_SKILL),
    ("spindle", include_str!("../skills/spindle/SKILL.md")),
];

impl State {
    pub(crate) fn available_provider_name(&self, label: &str) -> String {
        let occupied = |candidate: &str| {
            let folded = candidate.to_lowercase();
            (self.preferences.ollama_on && matches!(folded.as_str(), "ollama" | "ollama cloud"))
                || self.providers.iter().any(|profile| profile.name.to_lowercase() == folded)
        };
        if !occupied(label) { return label.to_owned(); }
        (2..).map(|number| {
            let suffix = format!("-{number}");
            let stem: String = label.chars().take(64usize.saturating_sub(suffix.len())).collect();
            format!("{stem}{suffix}")
        }).find(|name| !occupied(name)).unwrap()
    }

    pub(crate) fn current_plan(&self, chat: u64) -> Option<(u64, String)> {
        let (thread, saved) = if chat == self.chat_id {
            (&self.thread, &self.plan)
        } else {
            let c = self.chats.get(&chat)?;
            (&c.thread, &c.plan)
        };
        if thread.steps.is_empty() {
            return saved.clone();
        }
        // Older sessions could persist checklist edits without updating the saved draft.
        let revision = saved.as_ref().map_or(self.serial, |(rev, _)| *rev);
        Some((revision, crate::plan::checklist(&thread.steps)))
    }

    pub(crate) fn can_continue_stopped_goal(&self) -> bool {
        !self.goal.is_empty() && !self.thread.goal_done && (self.plan.is_some() || !self.thread.steps.is_empty())
            && self.history.len().checked_sub(1).and_then(|i| self.thread.turns.get(&i)).is_some_and(|t| t.failed)
            && !self.queue.iter().any(|q| q.origin.unwrap_or(self.chat_id) == self.chat_id)
            && !self.requests.iter().any(|r| r.chat == self.chat_id)
    }

    pub fn skill_definition(&self, name: &str) -> Option<&str> {
        self.skills
            .get(name)
            .map(String::as_str)
            .or_else(|| BUILTIN_SKILLS.iter().find(|(key, _)| *key == name).map(|(_, text)| *text))
    }

    pub fn skill_catalog(&self, limit: usize) -> String {
        BUILTIN_SKILLS.iter().map(|(name, _)| *name)
            .chain(self.skills.keys().map(String::as_str).filter(|name| !BUILTIN_SKILLS.iter().any(|(key, _)| key == name)))
            .take(limit)
            .filter_map(|name| {
                let definition = self.skill_definition(name)?;
                let description: String = definition
                    .lines()
                    .skip(1)
                    .take_while(|line| *line != "---")
                    .find_map(|line| line.strip_prefix("description:").map(str::trim))
                    .unwrap_or("")
                    .chars()
                    .take(140)
                    .collect();
                Some(if description.is_empty() {
                    format!("/{name}")
                } else {
                    format!("/{name} · {description}")
                })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn switch_chat(&mut self, id: Option<u64>) -> Result<()> {
        for task in &mut self.queue {
            task.origin.get_or_insert(self.chat_id);
        }
        let next = if let Some(id) = id {
            self.chats.remove(&id).context("Unknown conversation")?
        } else {
            Conversation::default()
        };
        crate::computer_use::stop(Some(self.chat_id));
        self.thread.model.clone_from(&self.model);
        let previous = Conversation {
            thread: std::mem::replace(&mut self.thread, next.thread),
            history: std::mem::replace(&mut self.history, next.history),
            goal: std::mem::replace(&mut self.goal, next.goal),
            plan: std::mem::replace(&mut self.plan, next.plan),
        };
        if !self.thread.model.is_empty() {
            self.model.clone_from(&self.thread.model);
        }
        if self.queue.iter().any(|t| t.origin == Some(self.chat_id))
            || self.sync.as_ref().is_some_and(|p| p.tracks(self.chat_id))
            || !previous.history.is_empty()
            || !previous.goal.is_empty()
            || previous.thread.started
            || !previous.thread.steps.is_empty()
            || !previous.thread.title.is_empty()
            || previous.thread.pinned
            || previous.thread.archived
            || !previous.thread.section.is_empty()
            || self
                .automation
                .schedules
                .iter()
                .any(|s| s.chat == self.chat_id)
        {
            self.chats.insert(self.chat_id, previous);
        }
        self.chat_id = id.unwrap_or_else(|| {
            self.serial += 1;
            self.serial
        });
        self.attachment = None;
        self.pending = None;
        self.unread.remove(&self.chat_id);
        Ok(())
    }
}

pub struct Store {
    path: PathBuf,
    _lock: File,
    digest: std::cell::Cell<Option<[u8; 32]>>,
}

const SESSION_MAGIC: &[u8] = b"SPINDLE-SESSION-1\0";

pub fn data_dir() -> Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .context("No user state directory available")
}

pub(crate) fn workspace_storage() -> Result<PathBuf> {
    static STORAGE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(path) = STORAGE.get() {
        return Ok(path.clone());
    }
    let base = data_dir()?;
    Ok(STORAGE.get_or_init(|| resolve_workspace_storage(&base)).clone())
}

fn resolve_workspace_storage(base: &Path) -> PathBuf {
    let current = base.join("Spindle").join("state");
    if current.exists() {
        return current;
    }
    // Recognize previous installs by their encrypted session format and hashed
    // workspace directory. Reuse their location and locks in place.
    let candidates = fs::read_dir(base).into_iter().flatten().flatten();
    let mut legacy = candidates.filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .filter(|folder| {
            fs::read_dir(folder).into_iter().flatten().flatten().any(|workspace| {
                let name = workspace.file_name();
                let Some(hash) = name.to_str() else { return false };
                if hash.len() != 16 || !hash.bytes().all(|b| b.is_ascii_hexdigit())
                    || !workspace.file_type().is_ok_and(|kind| kind.is_dir()) {
                    return false;
                }
                let path = workspace.path().join("session.json");
                if !fs::symlink_metadata(&path).is_ok_and(|file| file.is_file()) {
                    return false;
                }
                let mut header = [0; SESSION_MAGIC.len()];
                File::open(path).is_ok_and(|mut file| file.read_exact(&mut header).is_ok())
                    && header == SESSION_MAGIC
            })
        });
    match (legacy.next(), legacy.next()) {
        (Some(folder), None) => folder,
        _ => current,
    }
}

impl Store {
    pub(crate) fn troubleshooting(root: &Path) -> Result<crate::diagnostics::SavedDiagnostics> {
        // The owner publishes session snapshots by atomic replacement. Reading one here
        // doesn't need the workspace lock and never starts a second owner.
        Self::troubleshooting_at(&Self::directory(root)?)
    }

    fn troubleshooting_at(dir: &Path) -> Result<crate::diagnostics::SavedDiagnostics> {
        let bytes = fs::read(dir.join("session.json"))
            .context("No saved Spindle session for this workspace")?;
        let sealed = bytes.strip_prefix(SESSION_MAGIC).context("Open this workspace in Spindle to upgrade its session first")?;
        let plain = crate::vault::local(sealed, false)?;
        serde_json::from_slice(&plain).context("Read diagnostics from the saved session")
    }

    pub(crate) fn session_dir(&self) -> &Path {
        self.path.parent().unwrap()
    }
    pub(crate) fn directory(root: &Path) -> Result<PathBuf> {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hash = DefaultHasher::new();
        root.hash(&mut hash);
        Ok(workspace_storage()?.join(format!("{:016x}", hash.finish())))
    }

    pub fn open(root: &Path) -> Result<(Self, State)> {
        Self::open_directory(&Self::directory(root)?)
    }

    pub(crate) fn open_directory(dir: &Path) -> Result<(Self, State)> {
        fs::create_dir_all(dir)?;
        let lock = File::options()
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("session.lock"))?;
        try_lock(&lock)
            .context("This workspace is already open or its session lock is unavailable")?;
        let path = dir.join("session.json");
        let mut migrate = false;
        let mut state: State = match fs::read(&path) {
            Ok(bytes) => {
                let plain = if let Some(sealed) = bytes.strip_prefix(SESSION_MAGIC) {
                    crate::vault::local(sealed, false)?
                } else {
                    migrate = true;
                    bytes
                };
                serde_json::from_slice(&plain)
                    .context("Invalid session file; original preserved")?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(e) => return Err(e.into()),
        };
        crate::artifacts::upgrade_history(&mut state.thread, &mut state.history);
        for c in state.chats.values_mut() {
            crate::artifacts::upgrade_history(&mut c.thread, &mut c.history);
        }
        state.held |= !state.queue.is_empty();
        let store = Self {
            path,
            _lock: lock,
            digest: Default::default(),
        };
        if migrate {
            store.save(&state)?;
        }
        Ok((store, state))
    }

    pub fn save(&self, state: &State) -> Result<()> {
        let plain = serde_json::to_vec(state)?;
        let hash: [u8; 32] = ring::digest::digest(&ring::digest::SHA256, &plain)
            .as_ref()
            .try_into()?;
        if self.digest.get() == Some(hash) {
            return Ok(());
        }
        let sealed = crate::vault::local(&plain, true)?;
        let mut tmp = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())?;
        tmp.write_all(SESSION_MAGIC)?;
        tmp.write_all(&sealed)?;
        tmp.flush()?;
        tmp.as_file().sync_all()?;
        tmp.persist(&self.path)?;
        self.digest.set(Some(hash));
        Ok(())
    }
}

pub fn project_path(root: &Path, input: &str) -> Result<PathBuf> {
    let path = root.join(input.trim_matches('"')).canonicalize()?;
    let relative = path
        .strip_prefix(root)
        .context("Select a file inside this workspace")?;
    ensure!(
        !relative
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case(".git")),
        "Git internals are outside file review"
    );
    Ok(path)
}

pub fn read_project(root: &Path, input: &str, limit: usize) -> Result<Attachment> {
    let path = project_path(root, input)?;
    let relative = path.strip_prefix(root)?;
    let file = File::open(&path)?;
    ensure!(file.metadata()?.is_file(), "Expected a regular file");
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "File exceeds {limit} bytes; select a smaller excerpt"
    );
    Ok(Attachment {
        path: relative.display().to_string(),
        text: String::from_utf8(bytes).context("Expected UTF-8 text")?,
    })
}

pub const COMMANDS: &[&str] = &[
    "compact",
    "send-pastes",
    "thread",
    "threads",
    "usage",
    "sync-unlock",
    "sync-toggle",
    "preview",
    "providers",
    "automation",
    "fallback",
    "quota",
    "defer",
    "subagent",
    "reauth",
    "council",
    "connect",
    "disconnect",
    "refresh",
    "preferences",
    "favorite",
    "project-import",
    "read",
    "excerpt",
    "help",
    "new",
    "open",
    "goal",
    "plan",
    "model",
    "queue",
    "steer",
    "status",
    "cancel",
    "resume",
    "swarm",
    "skills",
    "permissions",
    "approve",
    "deny",
    "add",
    "exit",
];

pub fn skill_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 48
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            && !COMMANDS.contains(&name),
        "Use a unique lowercase skill name with letters, digits or hyphens"
    );
    Ok(())
}

pub fn parse_skill(text: &str) -> Result<String> {
    let normalized = text.replace("\r\n", "\n");
    let (meta, body) = normalized
        .strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n"))
        .context("SKILL.md needs front matter with name and description")?;
    let field = |key: &str| {
        meta.lines()
            .find_map(|s| s.strip_prefix(key))
            .map(str::trim)
            .unwrap_or("")
    };
    let name = field("name:");
    skill_name(name)?;
    ensure!(
        !field("description:").is_empty() && !body.trim().is_empty(),
        "Missing skill description or instructions"
    );
    Ok(name.into())
}

#[cfg(all(test, windows))]
mod encryption_tests {
    use super::*;

    #[test]
    fn diagnostics_can_be_read_without_taking_the_owner_lock() {
        let dir = tempfile::tempdir().unwrap();
        let (store, mut state) = Store::open_directory(dir.path()).unwrap();
        state.goal = "Synthetic private conversation".into();
        state.diagnostic_capture = Some(crate::diagnostics::Capture::start(crate::automation::now(), 2).unwrap());
        crate::diagnostics::sync(&mut state, 10, crate::diagnostics::SyncDecision::BranchesChanged { before: 0, after: 1, added: 1 }, vec![]);
        store.save(&state).unwrap();
        assert!(Store::open_directory(dir.path()).is_err());
        let saved = Store::troubleshooting_at(dir.path()).unwrap();
        assert_eq!(saved.diagnostic_capture.unwrap().count(), 1);
        assert_eq!(saved.sync_diagnostics[0].chat, 10);
        assert_eq!(state.goal, "Synthetic private conversation");
        drop(store);
    }

    #[test]
    fn renamed_storage_reuses_existing_encrypted_workspaces() {
        let base = tempfile::tempdir().unwrap();
        let previous = base.path().join("previous-install");
        let workspace = previous.join("a3ab1b846af0d1e5");
        let (store, mut state) = Store::open_directory(&workspace).unwrap();
        state.goal = "Keep working on this".into();
        store.save(&state).unwrap();
        drop(store);
        assert_eq!(resolve_workspace_storage(base.path()), previous);
        assert_eq!(Store::open_directory(&workspace).unwrap().1.goal, state.goal);
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(resolve_workspace_storage(empty.path()), empty.path().join("Spindle").join("state"));
    }

    #[test]
    fn plaintext_migrates_once_and_corruption_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let old = State {
            goal: "password-example-secret".into(),
            ..Default::default()
        };
        fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let (store, loaded) = Store::open_directory(dir.path()).unwrap();
        assert_eq!(loaded.goal, old.goal);
        drop(store);
        let mut bytes = fs::read(&path).unwrap();
        assert!(bytes.starts_with(SESSION_MAGIC));
        assert!(!String::from_utf8_lossy(&bytes).contains(&old.goal));
        assert_eq!(Store::open_directory(dir.path()).unwrap().1.goal, old.goal);
        bytes.truncate(bytes.len() / 2);
        fs::write(&path, &bytes).unwrap();
        assert!(Store::open_directory(dir.path()).is_err());
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

/// Wait for an exclusive lock on a file (released when the file closes). Android has no
/// standard-library file locks, so there it calls flock directly.
pub(crate) fn lock_file(file: &File) -> std::io::Result<()> {
    #[cfg(target_os = "android")]
    {
        use std::os::fd::AsRawFd;
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(());
            }
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    file.lock()
}

/// Lock the session file for this process. The standard library's file locks aren't
/// available on Android, so there it calls flock directly (same behaviour: exclusive,
/// released when the file closes).
pub(crate) fn try_lock(file: &File) -> std::result::Result<(), std::fs::TryLockError> {
    #[cfg(target_os = "android")]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        return Err(if e.kind() == std::io::ErrorKind::WouldBlock {
            std::fs::TryLockError::WouldBlock
        } else {
            std::fs::TryLockError::Error(e)
        });
    }
    #[cfg(not(target_os = "android"))]
    file.try_lock()
}

fn ollama_default() -> bool {
    !cfg!(target_os = "android")
}
