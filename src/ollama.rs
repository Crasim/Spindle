use crate::generation::{Continuation, Limits};
use anyhow::{Context, Result, bail, ensure};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

/// The local Ollama server, or the one SPINDLE_OLLAMA names (for example
/// http://192.168.1.42:11434), read once.
static CHOSEN: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());
static ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Use the Ollama settings: whether it's a provider at all, and its address (empty: this
/// computer, or SPINDLE_OLLAMA).
pub(crate) fn configure(on: bool, url: &str) {
    ON.store(on, std::sync::atomic::Ordering::Relaxed);
    set_endpoint(url);
}

pub(crate) fn enabled() -> bool {
    ON.load(std::sync::atomic::Ordering::Relaxed)
}

fn set_endpoint(url: &str) {
    let url = base_url(url);
    if let Ok(mut c) = CHOSEN.write() && *c != url {
        *c = url;
    }
}

fn base_url(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    if url.is_empty() || url.starts_with("http://") || url.starts_with("https://") {
        url.to_owned()
    } else {
        format!("http://{url}")
    }
}

pub(crate) fn endpoint() -> String {
    if let Ok(c) = CHOSEN.read()
        && !c.is_empty()
    {
        return c.clone();
    }
    default_endpoint()
}

fn default_endpoint() -> String {
    std::env::var("SPINDLE_OLLAMA")
        .ok()
        .filter(|u| u.starts_with("http"))
        .map(|u| u.trim_end_matches('/').to_owned())
        .unwrap_or_else(|| "http://127.0.0.1:11434".into())
}

static KEYS: std::sync::LazyLock<Mutex<HashMap<String, Option<String>>>> =
    std::sync::LazyLock::new(Default::default);

fn credential(base: &str) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, base.as_bytes());
    format!("ollama-{}", crate::sync::core::hex(hash.as_ref()))
}

#[cfg(not(any(windows, target_os = "android")))]
fn key_path(target: &str) -> std::path::PathBuf {
    crate::runtime::data_dir().join("credentials").join(target)
}

fn load_key(target: &str) -> Result<Option<String>> {
    #[cfg(windows)]
    return match keyring::Entry::new("Spindle", target)?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error).context("Read Ollama key in Windows Credential Manager"),
    };
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "android")]
        let path = crate::oauth::android_key_path(target)?;
        #[cfg(not(target_os = "android"))]
        let path = key_path(target);
        if !path.try_exists()? {
            return Ok(None);
        }
        #[cfg(target_os = "android")]
        return crate::oauth::android_key(target).map(Some);
        #[cfg(not(target_os = "android"))]
        Ok(Some(String::from_utf8(crate::vault::local(&std::fs::read(path)?, false)?)?))
    }
}

pub(crate) fn store_key(address: &str, key: &str) -> Result<()> {
    let base = base_url(address);
    let base = if base.is_empty() { default_endpoint() } else { base };
    let url = reqwest::Url::parse(&base).context("Enter an Ollama server address")?;
    ensure!(matches!(url.scheme(), "http" | "https") && url.has_host() && url.username().is_empty()
        && url.password().is_none() && url.query().is_none() && url.fragment().is_none(),
        "Enter a server URL without credentials, a query, or a fragment");
    ensure!(key.len() <= 4096 && key.bytes().all(|b| b.is_ascii_graphic()), "Invalid Ollama API key");
    let target = credential(&base);
    let mut keys = KEYS.lock().unwrap();
    #[cfg(windows)]
    {
        let entry = keyring::Entry::new("Spindle", &target)?;
        if key.is_empty() {
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => (),
                Err(error) => return Err(error.into()),
            }
        } else {
            entry.set_password(key).context("Save Ollama key in Windows Credential Manager")?;
        }
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_os = "android")]
        let path = crate::oauth::android_key_path(&target)?;
        #[cfg(not(target_os = "android"))]
        let path = key_path(&target);
        if key.is_empty() {
            match std::fs::remove_file(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
            }
        } else {
            #[cfg(target_os = "android")]
            crate::oauth::store_android_key(&target, key)?;
            #[cfg(not(target_os = "android"))]
            {
                std::fs::create_dir_all(path.parent().context("No credential directory")?)?;
                let protected = crate::vault::local(key.as_bytes(), true)?;
                crate::tools::write_generated(&path, &protected, true)?;
            }
        }
    }
    keys.insert(base, (!key.is_empty()).then(|| key.to_owned()));
    Ok(())
}

pub(crate) fn request(client: &Client, method: reqwest::Method, path: &str) -> Result<reqwest::RequestBuilder> {
    request_for(client, method, &endpoint(), path)
}

fn request_for(client: &Client, method: reqwest::Method, base: &str, path: &str) -> Result<reqwest::RequestBuilder> {
    let request = client.request(method, format!("{base}/api/{path}"));
    let mut keys = KEYS.lock().unwrap();
    let key = match keys.entry(base.to_owned()) {
        std::collections::hash_map::Entry::Occupied(key) => key.into_mut(),
        std::collections::hash_map::Entry::Vacant(key) => {
            let found = load_key(&credential(key.key()))?;
            key.insert(found)
        }
    };
    match key {
        Some(secret) => Ok(request.bearer_auth(secret.as_str())),
        None => Ok(request),
    }
}

/// SPINDLE_SAMPLING=default leaves small models at their own sampling settings, for comparing.
fn plain_sampling() -> bool {
    std::env::var("SPINDLE_SAMPLING").is_ok_and(|v| v == "default")
}

/// Model sizes in billions of parameters, as Ollama reported them.
static SIZES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, f64>>> =
    std::sync::LazyLock::new(Default::default);

/// A model's size in billions of parameters, once Spindle has asked Ollama about it.
pub fn known_size(model: &str) -> Option<f64> {
    let sizes = SIZES.lock().ok()?;
    sizes
        .get(model)
        .or_else(|| sizes.get(crate::providers::route(model).1))
        .copied()
}

/// "2.6B", "360M" from /api/show details, or the parameter count from model_info.
fn parameter_size(v: &Value) -> Option<f64> {
    if let Some(s) = v["details"]["parameter_size"].as_str() {
        let s = s.trim().to_ascii_uppercase();
        let (num, scale) = match s.chars().last()? {
            'B' => (&s[..s.len() - 1], 1.0),
            'M' => (&s[..s.len() - 1], 0.001),
            'T' => (&s[..s.len() - 1], 1000.0),
            _ => (s.as_str(), 1e-9),
        };
        if let Ok(n) = num.parse::<f64>() {
            return Some(n * scale);
        }
    }
    v["model_info"]["general.parameter_count"]
        .as_f64()
        .map(|n| n / 1e9)
}

/// For troubleshooting: when SPINDLE_TRACE names a folder, every request to Ollama and the
/// whole reply (with any native tool calls) is appended there, one JSON line per request, in
/// trace-YYYY-MM-DD.jsonl. Off unless the variable is set.
fn trace(body: &Value, answer: &str, native: &[Value], outcome: &dyn std::fmt::Debug) {
    let Some(dir) = std::env::var_os("SPINDLE_TRACE").map(std::path::PathBuf::from) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let now = chrono::Local::now();
    let line = json!({
        "time": now.to_rfc3339(),
        "request": body,
        "answer": answer,
        "native_tool_calls": native,
        "outcome": format!("{outcome:?}"),
    });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("trace-{}.jsonl", now.format("%Y-%m-%d"))))
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Message {
    pub role: String,
    pub content: String,
    /// Base64 images for vision models. Ollama reads this field as is; other providers
    /// get it converted by `providers::wire_messages`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl Message {
    pub fn new(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            images: vec![],
        }
    }
}

/// The MIME type of base64 image data, from its first bytes.
pub fn image_type(data: &str) -> &'static str {
    if data.starts_with("/9j/") {
        "image/jpeg"
    } else if data.starts_with("R0lGOD") {
        "image/gif"
    } else if data.starts_with("UklGR") {
        "image/webp"
    } else {
        "image/png"
    }
}

#[derive(Deserialize)]
struct Tag {
    name: String,
    remote_model: Option<String>,
    remote_host: Option<String>,
    capabilities: Option<Vec<String>>,
}

impl Tag {
    fn catalog_id(&self) -> Option<String> {
        if self
            .capabilities
            .as_ref()
            .is_some_and(|c| !c.iter().any(|s| s == "completion"))
        {
            return None;
        }
        let remote = self.remote_model.as_deref().is_some_and(|s| !s.is_empty())
            || self.remote_host.as_deref().is_some_and(|s| !s.is_empty())
            || self.name.ends_with(":cloud")
            || self.name.ends_with("-cloud");
        Some(if remote {
            format!("@ollama-cloud/{}", self.name)
        } else {
            self.name.clone()
        })
    }
}

#[derive(Clone)]
pub struct Ollama(
    pub(crate) Client,
    pub Option<tokio::sync::watch::Sender<String>>,
    Arc<Mutex<HashMap<String, (Instant, Limits)>>>,
    /// Catalog IDs whose Ollama capabilities include "thinking".
    Arc<Mutex<std::collections::BTreeSet<String>>>,
    /// Thinking levels reported by /api/show, by catalog ID.
    Arc<Mutex<HashMap<String, ThinkInfo>>>,
    /// Capabilities reported by /api/tags or /api/show ("vision", "audio", …), by catalog ID.
    Arc<Mutex<HashMap<String, Vec<String>>>>,
);

/// A model's thinking levels from /api/show: exact values `think` accepts, in order.
/// `true` and `false` appear as "true" and "false".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThinkInfo {
    pub values: Vec<String>,
    pub default: Option<String>,
}

impl ThinkInfo {
    pub fn supported(&self) -> bool {
        self.values.iter().any(|v| v != "false")
    }
    pub fn label(value: &str) -> String {
        match value {
            "false" | "none" => "Off".into(),
            "xhigh" => "Extra high".into(),
            "true" => "On".into(),
            v => {
                let mut c = v.chars();
                c.next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default()
            }
        }
    }
}

/// Read the `thinking` object from /api/show, or infer on/off from capabilities.
pub fn think_info(show: &Value) -> Option<ThinkInfo> {
    let text = |v: &Value| match v {
        Value::Bool(b) => Some(b.to_string()),
        Value::String(s) if !s.is_empty() && s.len() <= 40 => Some(s.clone()),
        _ => None,
    };
    if let Some(t) = show.get("thinking").filter(|t| t.is_object()) {
        let values: Vec<String> = t["values"]
            .as_array()
            .map(|a| a.iter().filter_map(text).take(12).collect())
            .unwrap_or_default();
        return Some(ThinkInfo {
            values,
            default: text(&t["default"]),
        });
    }
    show["capabilities"]
        .as_array()
        .is_some_and(|c| c.iter().any(|s| s == "thinking"))
        .then(|| ThinkInfo {
            values: vec!["false".into(), "true".into()],
            default: None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ollama_key_applies_only_to_its_server() {
        let first = "http://127.0.0.1:61001";
        let second = "http://127.0.0.1:61002";
        KEYS.lock().unwrap().extend([(first.to_owned(), Some("test-key".into())), (second.to_owned(), None)]);
        let client = Client::new();
        for (base, expected) in [(first, Some("Bearer test-key")), (second, None)] {
            let req = request_for(&client, reqwest::Method::GET, base, "tags").unwrap().build().unwrap();
            assert_eq!(req.headers().get(reqwest::header::AUTHORIZATION).and_then(|value| value.to_str().ok()), expected);
        }
        let mut keys = KEYS.lock().unwrap();
        keys.remove(first);
        keys.remove(second);
    }
    #[test]
    fn thinking_levels_come_from_show() {
        let gpt =
            think_info(&json!({"thinking":{"values":["low","medium","high"],"default":"medium"}}))
                .unwrap();
        assert_eq!(gpt.values, ["low", "medium", "high"]);
        assert_eq!(gpt.default.as_deref(), Some("medium"));
        assert!(gpt.supported());
        let none = think_info(&json!({"thinking":{"values":[false]}})).unwrap();
        assert!(!none.supported());
        let toggle = think_info(&json!({"capabilities":["completion","thinking"]})).unwrap();
        assert_eq!(toggle.values, ["false", "true"]);
        assert!(think_info(&json!({"capabilities":["completion"]})).is_none());
        assert_eq!(ThinkInfo::label("false"), "Off");
        assert_eq!(ThinkInfo::label("medium"), "Medium");
    }
    #[test]
    fn remote_alias_with_local_looking_name_has_cloud_scope() {
        let remote: Tag =
            serde_json::from_value(json!({"name":"alias:latest","remote_model":"private-cloud"}))
                .unwrap();
        assert_eq!(
            remote.catalog_id().as_deref(),
            Some("@ollama-cloud/alias:latest")
        );
        let local: Tag =
            serde_json::from_value(json!({"name":"model:latest","capabilities":["completion"]}))
                .unwrap();
        assert_eq!(local.catalog_id().as_deref(), Some("model:latest"));
    }
}

impl Ollama {
    pub fn new() -> Result<Self> {
        Ok(Self(
            Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .read_timeout(Duration::from_secs(180))
                .timeout(Duration::from_secs(300))
                .build()?,
            None,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        ))
    }

    pub async fn models(&self) -> Result<Vec<String>> {
        if !enabled() {
            return Ok(vec![]);
        }
        #[derive(Deserialize)]
        struct Tags {
            models: Vec<Tag>,
        }
        let tags: Tags = request(&self.0, reqwest::Method::GET, "tags")?
            .send()
            .await
            .with_context(|| format!("Cannot reach Ollama at {}", endpoint()))?
            .error_for_status()?
            .json()
            .await?;
        let mut thinking = std::collections::BTreeSet::new();
        let mut reported = HashMap::new();
        let mut models: Vec<_> = tags
            .models
            .into_iter()
            .filter_map(|t| {
                let id = t.catalog_id()?;
                if let Some(c) = &t.capabilities {
                    reported.insert(id.clone(), c.clone());
                }
                if t.capabilities
                    .as_ref()
                    .is_some_and(|c| c.iter().any(|s| s == "thinking"))
                {
                    thinking.insert(id.clone());
                }
                Some(id)
            })
            .collect();
        *self.3.lock().unwrap() = thinking;
        self.5.lock().unwrap().extend(reported);
        models.sort();
        Ok(models)
    }

    /// Capabilities Ollama has reported per model.
    pub fn capabilities(&self) -> HashMap<String, Vec<String>> {
        self.5.lock().unwrap().clone()
    }

    /// Thinking levels learned from /api/show so far.
    pub fn think_levels(&self) -> HashMap<String, ThinkInfo> {
        self.4.lock().unwrap().clone()
    }

    /// Models Ollama reported as thinking-capable in the last catalog refresh.
    pub fn thinking(&self) -> std::collections::BTreeSet<String> {
        self.3.lock().unwrap().clone()
    }

    pub(crate) async fn limits(&self, id: &str) -> Limits {
        if let Some((_, limits)) = self
            .2
            .lock()
            .unwrap()
            .get(id)
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(300))
        {
            return *limits;
        }
        let tag = crate::providers::route(id).1;
        let result = async {
            request(&self.0, reqwest::Method::POST, "show").ok()?
                .timeout(Duration::from_secs(2))
                .json(&json!({"model": tag}))
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .json::<Value>()
                .await
                .ok()
        }
        .await;
        let limits = result
            .as_ref()
            .map(|v| Limits::from_metadata(v, false))
            .unwrap_or_default();
        if let Some(size) = result.as_ref().and_then(parameter_size) {
            SIZES.lock().unwrap().insert(id.to_owned(), size);
            SIZES
                .lock()
                .unwrap()
                .insert(crate::providers::route(id).1.to_owned(), size);
        }
        if let Some(info) = result.as_ref().and_then(think_info) {
            self.4.lock().unwrap().insert(id.to_owned(), info);
        }
        if let Some(caps) = result.as_ref().and_then(|v| v["capabilities"].as_array()) {
            self.5.lock().unwrap().insert(
                id.to_owned(),
                caps.iter()
                    .filter_map(|c| c.as_str().map(str::to_owned))
                    .collect(),
            );
        }
        self.2
            .lock()
            .unwrap()
            .insert(id.to_owned(), (Instant::now(), limits));
        limits
    }

    pub async fn chat(&self, model: &str, messages: &[Message]) -> Result<String> {
        let catalog = self.models().await?;
        let id = catalog
            .iter()
            .find(|m| m.as_str() == model || crate::providers::route(m).1 == model)
            .context("Model unavailable in Ollama")?;
        let mut draft = Continuation::default();
        loop {
            let mut request = messages.to_vec();
            request.extend(draft.messages(usize::MAX));
            let mut obs = crate::automation::Observation::default();
            let answer = self.chat_metered(id, &request, 100, &mut obs).await?;
            if let Some(complete) = draft.accept(answer, obs.output_limited, obs.output)? {
                return Ok(complete);
            }
        }
    }
    pub async fn chat_metered(
        &self,
        id: &str,
        messages: &[Message],
        threshold: u32,
        obs: &mut crate::automation::Observation,
    ) -> Result<String> {
        ensure!(
            self.models().await?.iter().any(|m| m == id),
            "Model changed or is unavailable in this Ollama catalog: {id}"
        );
        let (scope, model) = crate::providers::route(id);
        let limits = self.limits(id).await;
        let local_context = (scope == "ollama").then(|| match obs.tuning.context {
            Some(n) => limits.context.map_or(n, |max| n.min(max)),
            None => limits.context.unwrap_or(16_384).min(16_384),
        });
        let allowance = limits.allowance(local_context, messages);
        obs.output_budget = Some(allowance);
        let mut options = json!({"num_predict": allowance});
        if let Some(n) = local_context {
            options["num_ctx"] = json!(n);
        } else if obs.window.is_none() {
            obs.window = limits.context;
        }
        if let Some(t) = obs.tuning.temperature {
            options["temperature"] = json!(t);
        } else if crate::policy::small(id) && !plain_sampling() {
            // Small models call tools more reliably a little cooler than their defaults.
            options["temperature"] = json!(0.3);
        }
        if crate::policy::small(id) && !plain_sampling() {
            // And repeat themselves less with a light penalty (Liquid suggests about 1.05).
            options["repeat_penalty"] = json!(1.05);
        }
        let mut body = json!({
            "model": model, "messages": messages, "stream": true,
            "options": options
        });
        // Models with built-in tool calling get real tool definitions, so they call tools by
        // name instead of imitating the written format.
        let native_tools = self
            .capabilities()
            .get(id)
            .is_some_and(|c| c.iter().any(|s| s == "tools"));
        if native_tools && let Some(system) = messages.first().filter(|message| message.role == "system") {
            // Only the tools the instructions offer, so a small model's list stays short.
            let offered: Vec<Value> = crate::tools::definitions()
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|d| {
                    d["function"]["name"]
                        .as_str()
                        .is_some_and(|n| system.content.contains(&format!("\"name\":\"{n}\"")))
                })
                .collect();
            if !offered.is_empty() {
                body["tools"] = json!(offered);
            }
        }
        match obs.tuning.thinking {
            // A level the model itself reported, sent exactly as given.
            _ if obs.tuning.think_value.is_some() => {
                body["think"] = match obs.tuning.think_value.as_deref() {
                    Some("true") => json!(true),
                    Some("false") => json!(false),
                    other => json!(other),
                }
            }
            crate::state::Thinking::Default => {}
            crate::state::Thinking::Off => body["think"] = json!(false),
            // gpt-oss and Inkling take an effort level; other thinking models take true or false.
            level if ["gpt-oss", "inkling"].iter().any(|m| model.contains(m)) => {
                body["think"] = json!(level.effort())
            }
            _ => body["think"] = json!(true),
        }
        // Models whose tool calls Ollama failed to parse this session: their calls are read
        // from the text instead.
        static UNPARSED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        if UNPARSED.lock().is_ok_and(|m| m.iter().any(|m| m == id)) {
            if let Some(b) = body.as_object_mut() {
                b.remove("tools");
            }
        }
        #[derive(Deserialize)]
        struct Reply {
            #[serde(default)]
            message: Option<Message>,
            #[serde(default)]
            error: Option<String>,
            #[serde(default)]
            done: bool,
            done_reason: Option<String>,
            prompt_eval_count: Option<u64>,
            eval_count: Option<u64>,
            eval_duration: Option<u64>,
        }
        let preview_start = self.1.as_ref().map_or(0, |p| p.borrow().len());
        'attempt: loop {
            let mut response = request(&self.0, reqwest::Method::POST, "chat")?
                .timeout(Duration::from_secs(3600))
                .json(&body)
                .send()
                .await?;
            let status = response.status();
            obs.http(&response, threshold);
            if !status.is_success() {
                bail!(
                    "Ollama returned {status}: {}",
                    response.text().await?.chars().take(500).collect::<String>()
                );
            }
            let mut buffer = Vec::new();
            let mut answer = String::new();
            // Calls a model made in its own tool format: Ollama moves them out of the text.
            let mut native_calls: Vec<serde_json::Value> = vec![];
            loop {
                let chunk = response.chunk().await?;
                let eof = chunk.is_none();
                if let Some(chunk) = chunk {
                    buffer.extend_from_slice(&chunk);
                } else if !buffer.is_empty() {
                    buffer.push(b'\n');
                }
                ensure!(buffer.len() <= 512 * 1024, "Oversized Ollama stream record");
                while let Some(end) = buffer.iter().position(|&b| b == b'\n') {
                    let record: Vec<_> = buffer.drain(..=end).collect();
                    if record.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    let reply: Reply = serde_json::from_slice(&record).with_context(|| {
                        format!(
                            "Invalid Ollama stream record: {}",
                            String::from_utf8_lossy(&record)
                                .chars()
                                .take(200)
                                .collect::<String>()
                        )
                    })?;
                    if let Some(error) = reply.error {
                        // Ollama couldn't parse the model's tool call (Qwen 3.5's XML form, say):
                        // ask again without Ollama's tool parsing and read the call from the text.
                        if body.get("tools").is_some() {
                            if let Ok(mut m) = UNPARSED.lock() {
                                m.push(id.to_owned());
                            }
                            if let Some(b) = body.as_object_mut() {
                                b.remove("tools");
                            }
                            if let Some(preview) = &self.1 {
                                preview.send_modify(|s| s.truncate(preview_start.min(s.len())));
                            }
                            continue 'attempt;
                        }
                        trace(&body, &answer, &native_calls, &format!("error: {error}"));
                        bail!("Ollama stopped with an error: {error}");
                    }
                    let reply_message = reply
                        .message
                        .unwrap_or_else(|| Message::new("assistant", ""));
                    if let Ok(raw) = serde_json::from_slice::<serde_json::Value>(&record)
                        && let Some(calls) = raw
                            .pointer("/message/tool_calls")
                            .and_then(|c| c.as_array())
                    {
                        native_calls.extend(calls.iter().cloned());
                    }
                    if !reply_message.content.is_empty() {
                        answer.push_str(&reply_message.content);
                        // A reply stuck repeating one line won't stop on its own: end it here
                        // and tell the model (see engine::LOOPED).
                        if reply_message.content.contains('\n')
                            && let Some(line) = crate::generation::looping(&answer)
                        {
                            trace(&body, &answer, &native_calls, &"stopped: repeating");
                            return Ok(format!("{}{line}", crate::generation::LOOPED));
                        }
                        ensure!(answer.len() <= 128 * 1024, "Model answer exceeds capacity");
                        if let Some(preview) = &self.1 {
                            preview.send_modify(|s| s.push_str(&reply_message.content));
                        }
                    }
                    if reply.done {
                        obs.input = reply.prompt_eval_count.unwrap_or(0);
                        obs.output = reply.eval_count.unwrap_or(0);
                        obs.rate = reply
                            .eval_duration
                            .filter(|n| *n > 0 && obs.output > 0)
                            .map(|n| obs.output as f64 * 1e9 / n as f64);
                        if crate::providers::route(id).0 == "ollama" {
                            obs.window = crate::providers::context_window(&self.0, id, None).await;
                        }
                        obs.output_limited = reply.done_reason.as_deref() == Some("length");
                        // Only when the reply text doesn't already carry the call.
                        if crate::tools::extract(&answer).1.is_none()
                            && let Some(block) = crate::tools::from_native(&native_calls)
                        {
                            answer = format!("{}\n\n{block}", answer.trim_end());
                        }
                        ensure!(
                            obs.output_limited || !answer.trim().is_empty(),
                            "Model returned an empty answer"
                        );
                        trace(&body, &answer, &native_calls, &obs.output);
                        return Ok(answer);
                    }
                }
                if eof {
                    return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Ollama stream ended before completion").into());
                }
            }
        }
    }
}
