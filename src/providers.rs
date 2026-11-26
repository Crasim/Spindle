use crate::{generation::Limits, ollama::Message};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, RequestBuilder, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum Protocol {
    #[default]
    ChatCompletions,
    Messages,
    Responses,
    ChatGpt,
    ClaudeConsole,
}

impl Protocol {
    pub fn oauth(self) -> bool {
        matches!(self, Self::ChatGpt | Self::ClaudeConsole)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Connection {
    pub name: String,
    pub base: String,
    pub protocol: Protocol,
    pub key: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Provider {
    #[serde(skip)]
    pub catalog_warning: Option<String>,
    #[cfg(any(windows, target_os = "android"))]
    #[serde(default)]
    pub registration: Option<crate::oauth::Registration>,
    #[cfg(any(windows, target_os = "linux"))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_login: Option<crate::claude_console::Login>,
    pub id: String,
    pub name: String,
    pub base: String,
    pub protocol: Protocol,
    pub models: Vec<String>,
    #[serde(default)]
    model_limits: BTreeMap<String, Limits>,
    credential: Option<String>,
    /// A synced API profile can require a key before one has been installed here.
    #[serde(default)]
    pub(crate) needs_auth: bool,
}

/// Known reasoning model families, for providers whose catalogs don't say.
pub fn reasoning_name(id: &str) -> bool {
    let tag = route(id).1.to_ascii_lowercase();
    let base = tag.rsplit('/').next().unwrap_or(&tag);
    [
        "inkling",
        "gpt-oss",
        "deepseek-r1",
        "deepseek-reasoner",
        "qwen3",
        "qwq",
        "magistral",
        "gpt-5",
        "claude-3-7",
        "claude-sonnet-4",
        "claude-opus-4",
        "claude-sonnet-5",
        "claude-opus-5",
        "claude-haiku-4",
        "claude-haiku-5",
        "grok-3-mini",
        "grok-4",
        "gemini-2.5",
        "gemini-3",
        "exaone-deep",
        "phi4-reasoning",
        "-thinking",
        "kimi-k2",
        "glm-4.5",
        "glm-4.6",
        "cogito",
        "smallthinker",
        "openthinker",
        "r1-distill",
    ]
    .iter()
    .any(|k| base.contains(k))
        || ["o1", "o3", "o4"]
            .iter()
            .any(|k| base == *k || base.starts_with(&format!("{k}-")))
}

fn base_name(id: &str) -> String {
    let tag = route(id).1.to_ascii_lowercase();
    tag.rsplit('/').next().unwrap_or(&tag).to_owned()
}

/// Known image-capable model families, for providers whose catalogs don't say.
pub fn vision_name(id: &str) -> bool {
    let base = base_name(id);
    [
        "llava",
        "vision",
        "-vl",
        "vl:",
        "minicpm-v",
        "moondream",
        "gemma3:",
        "gemma3-",
        "gemma-3",
        "llama4",
        "pixtral",
        "gpt-4o",
        "gpt-4.1",
        "gpt-5",
        "claude-3",
        "claude-sonnet",
        "claude-opus",
        "claude-haiku",
        "gemini",
        "grok-4",
        "mistral-small3",
        "qwen2.5vl",
        "qwen3-vl",
        "granite3.2-vision",
    ]
    .iter()
    .any(|k| base.contains(k))
        && !base.contains("gemma3:1b")
        && !base.contains("gemma3:270m")
}

/// Known audio-capable model families, for providers whose catalogs don't say.
pub fn audio_name(id: &str) -> bool {
    let base = base_name(id);
    ["audio", "whisper", "voxtral", "gemma3n", "omni", "realtime"]
        .iter()
        .any(|k| base.contains(k))
}

pub fn route(model: &str) -> (&str, &str) {
    model
        .strip_prefix('@')
        .and_then(|s| s.split_once('/'))
        .unwrap_or(("ollama", model))
}

fn endpoint(input: &str) -> Result<String> {
    let url = Url::parse(input.trim()).context("Enter a full API base URL")?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    ensure!(
        url.scheme() == "https" || (local && url.scheme() == "http"),
        "Use HTTPS for remote providers or HTTP on loopback"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Enter a base URL without credentials, query or fragment"
    );
    Ok(url.as_str().trim_end_matches('/').into())
}

impl Provider {
    pub(crate) fn new_id() -> Result<String> {
        Ok(format!("p-{}", crate::sync::core::hex(&crate::vault::random::<16>()?)))
    }

    pub(crate) fn same_connection(&self, other: &Self) -> bool {
        self.base == other.base && self.protocol == other.protocol && self.name == other.name
    }

    pub(crate) fn merge_catalog(&mut self, other: Self) {
        if !other.models.is_empty() {
            self.models = other.models;
            self.model_limits = other.model_limits;
        }
    }

    pub(crate) fn has_saved_key(&self) -> bool {
        self.credential.is_some()
    }

    pub fn catalog_window(&self, tag: &str) -> Option<u64> {
        self.model_limits.get(tag).and_then(|m| m.context)
    }

    pub async fn connect(
        form: Connection,
        id: String,
        vault: String,
        client: &Client,
    ) -> Result<Self> {
        ensure!(
            !form.name.trim().is_empty() && form.name.len() <= 64,
            "Choose a provider name (up to 64 characters)"
        );
        let mut provider = Self {
            catalog_warning: None,
            #[cfg(any(windows, target_os = "android"))]
            registration: None,
            #[cfg(any(windows, target_os = "linux"))]
            claude_login: None,
            id,
            name: form.name.trim().into(),
            base: if form.protocol == Protocol::ClaudeConsole {
                "https://api.anthropic.com/v1".into()
            } else {
                endpoint(&form.base)?
            },
            protocol: form.protocol,
            models: vec![],
            model_limits: Default::default(),
            credential: None,
            needs_auth: form.protocol.oauth() || !form.key.is_empty(),
        };
        if form.protocol == Protocol::ChatGpt {
            #[cfg(any(windows, target_os = "android"))]
            {
                ensure!(
                    provider.base == crate::oauth::API && form.key.is_empty(),
                    "ChatGPT sign-in requires the official OpenAI endpoint"
                );
                let (reg, access) = crate::oauth::sign_in(client, vault, None).await?;
                provider.registration = Some(reg);
                provider.refresh_authorized(client, &access).await;
                return Ok(provider);
            }
            #[cfg(not(any(windows, target_os = "android")))]
            bail!("ChatGPT sign-in is available in Spindle on Windows and Android");
        }
        if form.protocol == Protocol::ClaudeConsole {
            #[cfg(any(windows, target_os = "linux"))]
            {
                ensure!(form.key.is_empty(), "Claude Console uses a browser sign-in");
                let login = crate::claude_console::Login::new(&form.base, vault)?;
                login.sign_in().await?;
                let access = login.access().await?;
                provider.claude_login = Some(login);
                provider.refresh_authorized(client, &access).await;
                return Ok(provider);
            }
            #[cfg(not(any(windows, target_os = "linux")))]
            bail!("Claude Console sign-in requires the Anthropic CLI on a computer");
        }
        provider.refresh_catalog(client, &form.key).await?;
        if !form.key.is_empty() {
            #[cfg(windows)]
            {
                keyring::Entry::new("Spindle", &vault)?
                    .set_password(&form.key)
                    .context("Saving the API key in Windows Credential Manager")?;
                provider.credential = Some(vault);
            }
            #[cfg(target_os = "android")]
            {
                crate::oauth::store_android_key(&vault, &form.key)?;
                provider.credential = Some(vault);
            }
            #[cfg(not(any(windows, target_os = "android")))]
            bail!("Saved provider keys are available on Windows and Android");
        }
        Ok(provider)
    }

    pub fn key(&self) -> Result<String> {
        if let Some(_target) = &self.credential {
            #[cfg(windows)]
            return keyring::Entry::new("Spindle", _target)?
                .get_password()
                .context("Read provider key in Windows Credential Manager");
            #[cfg(target_os = "android")]
            return crate::oauth::android_key(_target);
            #[cfg(not(any(windows, target_os = "android")))]
            bail!("Windows credential unavailable");
        }
        Ok(String::new())
    }
    pub async fn access(&self, client: &Client) -> Result<String> {
        if self.protocol == Protocol::ChatGpt {
            #[cfg(any(windows, target_os = "android"))]
            if let Some(r) = &self.registration {
                ensure!(self.base == crate::oauth::API, "Invalid ChatGPT connection endpoint");
                return crate::oauth::access(client, r).await;
            }
            bail!("Sign in to this ChatGPT connection in Settings › Providers");
        }
        if self.protocol == Protocol::ClaudeConsole {
            #[cfg(any(windows, target_os = "linux"))]
            if let Some(login) = &self.claude_login {
                ensure!(self.base == crate::claude_console::API, "Invalid Claude Console connection endpoint");
                return login.access().await;
            }
            bail!("Connect to Claude Console on a computer with the official Anthropic CLI");
        }
        let _ = client;
        self.key()
    }

    /// Store an API key that arrived by sync, the way keys typed here are stored.
    pub fn store_key(&mut self, key: &str) -> Result<()> {
        ensure!(!self.protocol.oauth(), "This provider uses browser sign-in");
        #[cfg(windows)]
        {
            let target = self
                .credential
                .clone()
                .unwrap_or_else(|| format!("Spindle provider {}", self.id));
            keyring::Entry::new("Spindle", &target)?
                .set_password(key)
                .context("Saving the API key in Windows Credential Manager")?;
            self.credential = Some(target);
            self.needs_auth = true;
            Ok(())
        }
        #[cfg(target_os = "android")]
        {
            let target = self.credential.clone().unwrap_or_else(|| format!("Spindle provider {}", self.id));
            crate::oauth::store_android_key(&target, key)?;
            self.credential = Some(target);
            self.needs_auth = true;
            Ok(())
        }
        #[cfg(not(any(windows, target_os = "android")))]
        {
            let _ = key;
            bail!("Storing API keys needs Windows Credential Manager")
        }
    }

    pub fn forget_key(&self) -> Result<()> {
        #[cfg(windows)]
        if let Some(target) = &self.credential {
            match keyring::Entry::new("Spindle", target)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => (),
                Err(e) => return Err(e.into()),
            }
        }
        #[cfg(target_os = "android")]
        if let Some(target) = &self.credential {
            match std::fs::remove_file(crate::oauth::android_key_path(target)?) {
                Ok(()) => (),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => (),
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    fn authorize(&self, req: RequestBuilder, key: &str) -> RequestBuilder {
        match self.protocol {
            Protocol::ChatCompletions | Protocol::Responses | Protocol::ChatGpt
                if !key.is_empty() =>
            {
                req.bearer_auth(key)
            }
            Protocol::Messages => {
                let req = req.header("anthropic-version", "2023-06-01");
                if key.is_empty() {
                    req
                } else {
                    req.header("x-api-key", key)
                }
            }
            Protocol::ClaudeConsole => req.header("anthropic-version", "2026-10-21").bearer_auth(key),
            _ => req,
        }
    }

    /// Dollars per input and output token from the provider's catalog.
    pub fn price(&self, tag: &str) -> Option<(f64, f64)> {
        self.model_limits.get(tag).and_then(|l| l.price)
    }

    /// Billed per use: a hosted API rather than a plan sign-in or a local server.
    pub fn metered(&self) -> bool {
        let local = Url::parse(&self.base)
            .ok()
            .is_some_and(|u| matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")));
        self.protocol != Protocol::ChatGpt && !local
    }

    /// Credits left, for providers that report them (OpenRouter with a management key).
    pub async fn credits(&self, client: &Client) -> Option<f64> {
        if !self.base.contains("openrouter.ai") {
            return None;
        }
        let key = self.access(client).await.ok()?;
        let v: Value = self
            .authorize(client.get(format!("{}/credits", self.base)), &key)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()?;
        Some(v["data"]["total_credits"].as_f64()? - v["data"]["total_usage"].as_f64()?)
    }

    /// The provider's catalog says this model accepts images.
    pub fn sees(&self, tag: &str) -> bool {
        self.model_limits.get(tag).is_some_and(|l| l.vision)
    }

    /// The provider's catalog says this model accepts audio.
    pub fn hears(&self, tag: &str) -> bool {
        self.model_limits.get(tag).is_some_and(|l| l.audio)
    }

    /// The provider's catalog says this model reasons.
    pub fn reasons(&self, tag: &str) -> bool {
        self.model_limits.get(tag).is_some_and(|l| l.reasoning)
    }

    pub async fn refresh_catalog(&mut self, client: &Client, key: &str) -> Result<()> {
        self.model_limits = self.catalog(client, key).await?;
        self.models = self.model_limits.keys().cloned().collect();
        Ok(())
    }

    pub async fn refresh_authorized(&mut self, client: &Client, key: &str) {
        if let Err(err) = self.refresh_catalog(client, key).await {
            self.catalog_warning = Some(format!("Account authorized. Refresh the catalog to retry: {err:#}"));
        }
    }

    pub async fn sign_in_again(mut self, client: &Client, vault: String) -> Result<Self> {
        #[cfg(any(windows, target_os = "android"))]
        if self.protocol == Protocol::ChatGpt {
            ensure!(self.base == crate::oauth::API, "Invalid ChatGPT connection endpoint");
            let old = self.registration.as_ref();
            let vault = old.map(|r| r.vault.clone()).unwrap_or(vault);
            let (registration, key) = crate::oauth::sign_in(client, vault, old).await?;
            self.registration = Some(registration);
            self.refresh_authorized(client, &key).await;
            return Ok(self);
        }
        #[cfg(any(windows, target_os = "linux"))]
        if let Some(login) = &self.claude_login {
            ensure!(self.protocol == Protocol::ClaudeConsole && self.base == crate::claude_console::API, "Invalid Claude Console connection endpoint");
            login.sign_in().await?;
            let key = login.access().await?;
            self.refresh_authorized(client, &key).await;
            return Ok(self);
        }
        let _ = (client, vault);
        bail!("Choose a sign-in method available on this device in Settings › Providers")
    }

    pub async fn sign_out(&self, client: &Client) -> Result<()> {
        #[cfg(any(windows, target_os = "android"))]
        if self.protocol == Protocol::ChatGpt && let Some(registration) = &self.registration {
            crate::oauth::revoke(client, registration).await?;
        }
        #[cfg(any(windows, target_os = "linux"))]
        if self.protocol == Protocol::ClaudeConsole && let Some(login) = &self.claude_login {
            login.sign_out().await?;
        }
        let _ = client;
        Ok(())
    }

    async fn catalog(&self, client: &Client, key: &str) -> Result<BTreeMap<String, Limits>> {
        let req = self.authorize(client.get(format!("{}/models", self.base)), key);
        let data: Value = req
            .timeout(Duration::from_secs(12))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let plan = self.protocol == Protocol::ChatGpt;
        let entries = data[if plan { "models" } else { "data" }]
            .as_array()
            .context("Expected a provider model list")?;
        let names: BTreeMap<_, _> = entries
            .iter()
            .filter(|v| !plan || v["visibility"] == "list")
            .filter_map(|v| {
                let id = v[if plan { "slug" } else { "id" }].as_str()?;
                if id.is_empty() || id.len() >= 256 || id.contains(char::is_whitespace) {
                    return None;
                }
                Some((
                    id.to_owned(),
                    Limits::from_metadata(v, matches!(self.protocol, Protocol::Messages | Protocol::ClaudeConsole)),
                ))
            })
            .collect();
        ensure!(!names.is_empty(), "Provider returned no models");
        Ok(names)
    }

    pub async fn chat(
        &self,
        client: &Client,
        model: &str,
        messages: &[Message],
        preview: Option<&tokio::sync::watch::Sender<String>>,
        threshold: u32,
        obs: &mut crate::automation::Observation,
    ) -> Result<String> {
        ensure!(
            self.models.iter().any(|s| s == model),
            "Model is missing from the provider catalog"
        );
        let key = self.access(client).await?;
        let local = matches!(
            Url::parse(&self.base)?.host_str(),
            Some("127.0.0.1" | "localhost" | "[::1]")
        );
        let limits = if local && self.protocol == Protocol::ChatCompletions {
            *self
                .catalog(client, &key)
                .await.with_context(|| format!("Could not read the model catalog from {} at {}. Check that the local model server is running", self.name, self.base))?
                .get(model)
                .context("Local server changed its loaded model. Refresh its catalog.")?
        } else if matches!(self.protocol, Protocol::Messages | Protocol::ClaudeConsole) && !self.model_limits.contains_key(model) {
            // Profiles saved by older versions have model IDs without capability metadata.
            self.catalog(client, &key)
                .await
                .ok()
                .and_then(|models| models.get(model).copied())
                .unwrap_or_default()
        } else {
            self.model_limits.get(model).copied().unwrap_or_default()
        };
        let allowance = limits.allowance(obs.window, messages);
        let (path, mut body) = match self.protocol {
            Protocol::Responses | Protocol::ChatGpt => (
                "responses",
                json!({"model":model,"instructions":messages.iter().filter(|m|m.role=="system").map(|m|m.content.as_str()).collect::<Vec<_>>().join("\n"),"input":wire_messages(self.protocol,messages.iter().filter(|m|m.role!="system")),"stream":true,"store":false}),
            ),
            Protocol::ChatCompletions => (
                "chat/completions",
                json!({"model":model,"messages":wire_messages(self.protocol,messages.iter()),"stream":true,"stream_options":{"include_usage":true}}),
            ),
            Protocol::Messages | Protocol::ClaudeConsole => (
                "messages",
                json!({"model":model,"system":messages.iter().filter(|m|m.role=="system").map(|m|m.content.as_str()).collect::<Vec<_>>().join("\n"),"messages":wire_messages(self.protocol,messages.iter().filter(|m|m.role!="system")),"max_tokens":allowance,"stream":true}),
            ),
        };
        if matches!(self.protocol, Protocol::Messages | Protocol::ClaudeConsole) {
            obs.output_budget = Some(allowance);
        }
        if self.protocol == Protocol::Responses && limits.output.is_some() {
            obs.output_budget = Some(allowance);
            body["max_output_tokens"] = json!(allowance);
        }
        if self.protocol == Protocol::ChatCompletions && (local || limits.output.is_some()) {
            obs.output_budget = Some(allowance);
            if local {
                body["max_tokens"] = json!(allowance);
            } else {
                body["max_completion_tokens"] = json!(allowance);
            }
        }
        apply_tuning(self.protocol, &obs.tuning, &mut body, allowance);
        let inkling = (local || self.name.eq_ignore_ascii_case("Maya")) && self.protocol == Protocol::ChatCompletions
            && model.to_ascii_lowercase().contains("inkling");
        if inkling && obs.tuning.thinking == crate::state::Thinking::Off {
            body["chat_template_kwargs"] = json!({"reasoning_effort":"none"});
        }
        // Maya's Inkling template expects the browser call in its native tool channel.
        let browser_tool = inkling
            && messages.first().is_some_and(|m| {
                m.role == "system" && m.content.contains(crate::engine::BROWSER_TOOL_EXAMPLE)
            });
        if browser_tool
            && let Some(definition) = crate::tools::definitions().as_array()
                .and_then(|tools| tools.iter().find(|t| t["function"]["name"] == "browser_tab"))
        {
            body["tools"] = json!([definition]);
        }
        let mut tool_name = String::new();
        let mut tool_arguments = String::new();
        let mut reasoning = false;
        let mut finish_reason = String::new();
        let mut request = self
            .authorize(client.post(format!("{}/{path}", self.base)), &key)
            .json(&body);
        if Url::parse(&self.base)?
            .host_str()
            .is_some_and(|h| matches!(h, "localhost" | "127.0.0.1" | "[::1]"))
        {
            request = request.timeout(Duration::from_secs(3600));
        }
        let response = request.send().await?;
        obs.http(&response, threshold);
        let mut response = response.error_for_status()?;
        let mut buffer = Vec::new();
        let mut answer = String::new();
        while let Some(chunk) = response.chunk().await? {
            buffer.extend_from_slice(&chunk);
            ensure!(
                buffer.len() < 1024 * 1024,
                "Provider stream record exceeds limit"
            );
            while let Some(i) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = buffer.drain(..=i).collect();
                let Some(data) = std::str::from_utf8(&line)?.trim().strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" && self.protocol == Protocol::ChatCompletions {
                    if browser_tool && !obs.output_limited && tool_name == "browser_tab"
                        && let Ok(action) = serde_json::from_str::<crate::browser_link::Action>(&tool_arguments)
                        && crate::tools::extract(&answer).1.is_none()
                    {
                        let call = crate::tools::taught(&crate::tools::Call::BrowserTab(action));
                        answer = format!("{}\n\n```spindle-tool\n{call}\n```", answer.trim());
                    }
                    if answer.trim().is_empty() {
                        if inkling && messages.iter().all(|m| m.images.is_empty())
                            && matches!(finish_reason.as_str(), "stop" | "length" | "")
                            && !obs.rejected && obs.limit.is_none() {
                            return self.inkling_final(client, &key, &body, allowance, obs, preview).await;
                        }
                        if reasoning || !obs.output_limited {
                            let detail = if reasoning { "reasoning but no final answer" } else { "no final answer text" };
                            bail!("{model} returned {detail} (finish: {finish_reason}, {} output tokens)", obs.output);
                        }
                    }
                    return complete(answer, obs.output_limited);
                }
                let event: Value =
                    serde_json::from_str(data).context("Invalid provider stream event")?;
                if let Some(rate) = event
                    .pointer("/timings/predicted_per_second")
                    .and_then(Value::as_f64)
                    .filter(|n| *n > 0.)
                {
                    obs.rate = Some(rate);
                }
                if let Some(n) = event
                    .pointer("/message/usage/input_tokens")
                    .or_else(|| event.pointer("/usage/prompt_tokens"))
                    .and_then(Value::as_u64)
                {
                    obs.input = n;
                }
                if let Some(n) = event
                    .pointer("/usage/output_tokens")
                    .or_else(|| event.pointer("/usage/completion_tokens"))
                    .and_then(Value::as_u64)
                {
                    obs.output = n;
                }
                ensure!(
                    event.get("error").is_none(),
                    "Provider reported an inference error"
                );
                let delta = match self.protocol {
                    Protocol::Responses | Protocol::ChatGpt => match event["type"].as_str() {
                        Some("response.completed" | "response.incomplete")
                            if event["type"] == "response.completed"
                                || event
                                    .pointer("/response/incomplete_details/reason")
                                    .and_then(Value::as_str)
                                    == Some("max_output_tokens") =>
                        {
                            obs.output_limited = event["type"] == "response.incomplete";
                            obs.input = event
                                .pointer("/response/usage/input_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            obs.output = event
                                .pointer("/response/usage/output_tokens")
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            return complete(answer, obs.output_limited);
                        }
                        Some("response.failed" | "response.incomplete" | "error") => {
                            let code = event
                                .pointer("/response/error/code")
                                .or_else(|| event.get("code"))
                                .and_then(Value::as_str)
                                .unwrap_or("incomplete");
                            if code == "subscription_sharing_usage_limit_exceeded"
                                || code == "rate_limit_exceeded"
                            {
                                obs.rejected = true;
                                obs.limit.get_or_insert(crate::automation::Limit {
                                    reset: None,
                                    reason: format!("Provider reported {code}"),
                                });
                            }
                            bail!("OpenAI response stopped: {code}");
                        }
                        Some("response.output_text.delta") => event["delta"].as_str(),
                        _ => None,
                    },
                    Protocol::ChatCompletions => {
                        let choice = &event["choices"][0];
                        if let Some(reason) = choice["finish_reason"].as_str() {
                            finish_reason = reason.to_owned();
                        }
                        reasoning |= choice["delta"]["reasoning_content"].as_str().is_some_and(|s| !s.is_empty());
                        obs.output_limited |= choice["finish_reason"] == "length";
                        if browser_tool && let Some(calls) = choice["delta"]["tool_calls"].as_array() {
                            for call in calls.iter().filter(|call| call["index"].as_u64().unwrap_or(0) == 0) {
                                let function = &call["function"];
                                tool_name.push_str(function["name"].as_str().unwrap_or_default());
                                tool_arguments.push_str(function["arguments"].as_str().unwrap_or_default());
                                ensure!(tool_name.len() + tool_arguments.len() <= 16 * 1024, "Browser tool call exceeds limit");
                            }
                        }
                        choice["delta"]["content"].as_str()
                    }
                    Protocol::Messages | Protocol::ClaudeConsole => {
                        if event["type"] == "message_stop" {
                            return complete(answer, obs.output_limited);
                        }
                        obs.output_limited |= matches!(
                            event["delta"]["stop_reason"].as_str(),
                            Some("max_tokens" | "model_context_window_exceeded")
                        );
                        event["delta"]["text"].as_str()
                    }
                };
                if let Some(text) = delta {
                    answer.push_str(text);
                    ensure!(answer.len() <= 128 * 1024, "Response exceeds limit");
                    if let Some(tx) = preview {
                        tx.send_modify(|s| s.push_str(text));
                    }
                }
            }
        }
        Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Provider stream closed before completion").into())
    }

    /// Ask the loaded Inkling template to render the same text conversation, then start
    /// generation inside its public text block. A thinking-only stop gets one retry.
    async fn inkling_final(&self, client: &Client, key: &str, body: &Value, allowance: u64,
        obs: &mut crate::automation::Observation,
        preview: Option<&tokio::sync::watch::Sender<String>>) -> Result<String> {
        let root = self.base.trim_end_matches('/').strip_suffix("/v1")
            .context("Inkling ended without a final answer; no local template endpoint")?;
        let template: Value = self.authorize(client.post(format!("{root}/apply-template")), key)
            .json(&json!({"messages":body["messages"],"chat_template_kwargs":{"reasoning_effort":"none"}}))
            .send().await?.error_for_status()?.json().await?;
        let prompt = template["prompt"].as_str().filter(|s| s.contains("<|message_user|>"))
            .context("Inkling ended without a final answer; the server did not return its chat template")?;
        let mut request = json!({
            "model": body["model"],
            "prompt": format!("{prompt}<|message_model|><|content_text|>"),
            "n_predict": allowance, "stream": true,
            "stop": ["<|end_message|>", "<|message_model|>", "<|content_thinking|>", "<|content_model_end_sampling|>"]
        });
        if let Some(t) = body.get("temperature") { request["temperature"] = t.clone(); }
        let interrupted = || format!("{} disconnected while generating the final answer for {}. Check the model server at {root} before retrying", self.name, body["model"].as_str().unwrap_or("Inkling"));
        let mut response = self.authorize(client.post(format!("{root}/completion")), key)
            .timeout(Duration::from_secs(3600)).json(&request).send().await
            .with_context(interrupted)?.error_for_status()?;
        let mut buffer = Vec::new();
        let mut answer = String::new();
        while let Some(chunk) = response.chunk().await.with_context(interrupted)? {
            buffer.extend_from_slice(&chunk);
            ensure!(buffer.len() < 1024 * 1024, "Provider stream record exceeds limit");
            while let Some(end) = buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = buffer.drain(..=end).collect();
                let Some(data) = std::str::from_utf8(&line)?.trim().strip_prefix("data:") else { continue };
                if data.trim() == "[DONE]" { return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, interrupted()).into()); }
                let event: Value = serde_json::from_str(data).context("Invalid Inkling completion event")?;
                if let Some(error) = event.get("error") { bail!("Inkling generation failed: {error}"); }
                if let Some(text) = event["content"].as_str() {
                    ensure!(answer.len() + text.len() <= 128 * 1024, "Response exceeds limit");
                    answer.push_str(text);
                    if let Some(tx) = preview { tx.send_modify(|s| s.push_str(text)); }
                }
                if event["stop"] == true {
                    obs.input += event["tokens_evaluated"].as_u64().unwrap_or(0);
                    obs.output += event["tokens_predicted"].as_u64().unwrap_or(0);
                    obs.output_limited = event["stop_type"] == "limit" || event["stopped_limit"] == true;
                    obs.rate = event.pointer("/timings/predicted_per_second").and_then(Value::as_f64).or(obs.rate);
                    return complete(answer, obs.output_limited).context("Inkling ended without a final answer after a text-channel retry");
                }
            }
        }
        Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, interrupted()).into())
    }
}
/// Messages in each protocol's format. Text-only messages stay plain strings; images
/// become the protocol's content parts.
fn wire_messages<'a>(
    protocol: Protocol,
    messages: impl Iterator<Item = &'a Message>,
) -> Vec<Value> {
    messages
        .map(|m| {
            if m.images.is_empty() {
                return json!({"role": m.role, "content": m.content});
            }
            let url = |data: &String| format!("data:{};base64,{data}", crate::ollama::image_type(data));
            let parts: Vec<Value> = match protocol {
                Protocol::Messages | Protocol::ClaudeConsole => m
                    .images
                    .iter()
                    .map(|d| json!({"type": "image", "source": {"type": "base64", "media_type": crate::ollama::image_type(d), "data": d}}))
                    .chain(std::iter::once(json!({"type": "text", "text": m.content})))
                    .collect(),
                Protocol::Responses | Protocol::ChatGpt => std::iter::once(json!({"type": "input_text", "text": m.content}))
                    .chain(m.images.iter().map(|d| json!({"type": "input_image", "image_url": url(d)})))
                    .collect(),
                Protocol::ChatCompletions => std::iter::once(json!({"type": "text", "text": m.content}))
                    .chain(m.images.iter().map(|d| json!({"type": "image_url", "image_url": {"url": url(d)}})))
                    .collect(),
            };
            json!({"role": m.role, "content": parts})
        })
        .collect()
}

/// Add advanced model settings to a request body in the provider's own terms.
fn apply_tuning(
    protocol: Protocol,
    tuning: &crate::state::Tuning,
    body: &mut serde_json::Value,
    allowance: u64,
) {
    use crate::state::Thinking;
    let thinking = tuning.thinking;
    match protocol {
        Protocol::Messages | Protocol::ClaudeConsole => {
            if let Some(effort) = thinking.effort() {
                let wanted: u64 = match effort {
                    "low" => 2_048,
                    "medium" => 8_192,
                    _ => 24_576,
                };
                // The thinking budget must leave room for the answer inside max_tokens.
                let budget = wanted.min(allowance.saturating_sub(1_024)).max(1_024);
                body["max_tokens"] = json!(allowance.max(budget + 1_024));
                body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
            } else if let Some(t) = tuning.temperature {
                // Anthropic requires the default temperature while thinking.
                body["temperature"] = json!(t.min(1.0));
            }
        }
        Protocol::Responses | Protocol::ChatGpt => {
            match thinking {
                Thinking::Off => body["reasoning"] = json!({"effort": "minimal"}),
                t => {
                    if let Some(effort) = t.effort() {
                        body["reasoning"] = json!({ "effort": effort });
                    }
                }
            }
            if let Some(t) = tuning.temperature {
                body["temperature"] = json!(t);
            }
        }
        Protocol::ChatCompletions => {
            if let Some(effort) = thinking.effort() {
                body["reasoning_effort"] = json!(effort);
            }
            if let Some(t) = tuning.temperature {
                body["temperature"] = json!(t);
            }
        }
    }
}

pub async fn context_window(client: &Client, id: &str, provider: Option<&Provider>) -> Option<u64> {
    let (scope, tag) = route(id);
    if scope == "ollama" {
        let data: Value = client
            .get(format!("{}/api/ps", crate::ollama::endpoint()))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()?;
        return data["models"]
            .as_array()?
            .iter()
            .find(|m| m["name"] == tag || m["model"] == tag)?["context_length"]
            .as_u64()
            .filter(|n| *n > 0);
    }
    let provider = provider?;
    let url = Url::parse(&provider.base).ok()?;
    // Ask servers on this computer or the local network what they are running with now
    // (their context can change after Spindle first read their catalog). Hosted APIs are
    // known from their catalogs.
    if !local_host(url.host_str().unwrap_or("")) {
        return None;
    }
    let get = |u: Url| async move {
        client
            .get(u)
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json::<Value>()
            .await
            .ok()
    };
    // llama.cpp and servers like it (Project Maya, for one).
    if let Some(n) = get(url.join("/props").ok()?)
        .await
        .and_then(|p| p.pointer("/default_generation_settings/n_ctx")?.as_u64())
        .filter(|n| *n > 0)
    {
        return Some(n);
    }
    // The model list: context_length (Maya, LM Studio, OpenRouter-style), max_model_len
    // (vLLM), meta.n_ctx (llama.cpp).
    let base = provider.base.trim_end_matches('/');
    let list = get(Url::parse(&format!("{base}/models")).ok()?).await?;
    model_context(&list, tag)
}

/// The context a model list reports for one model.
fn model_context(list: &Value, tag: &str) -> Option<u64> {
    let items = list["data"].as_array().or(list["models"].as_array())?;
    let item = items
        .iter()
        .find(|m| m["id"] == tag || m["name"] == tag || m["model"] == tag)
        .or((items.len() == 1).then(|| &items[0]))?;
    [
        "/context_length",
        "/max_model_len",
        "/context_window",
        "/meta/n_ctx",
        "/meta/n_ctx_train",
        "/max_context_length",
    ]
    .iter()
    .find_map(|p| item.pointer(p)?.as_u64().filter(|n| *n > 0))
}

/// This computer or the local network: loopback, private addresses, .local and .lan names.
fn local_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if matches!(host, "localhost" | "::1") || host.ends_with(".local") || host.ends_with(".lan") {
        return true;
    }
    host.parse::<std::net::Ipv4Addr>()
        .is_ok_and(|ip| ip.is_loopback() || ip.is_private() || ip.is_link_local())
}

fn complete(answer: String, limited: bool) -> Result<String> {
    ensure!(
        limited || !answer.trim().is_empty(),
        "Provider returned an empty answer"
    );
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_servers_report_their_context_in_the_model_list() {
        // Project Maya's /v1/models.
        let maya = json!({"object":"list","data":[{"id":"inkling-2-mini","meta":{"n_ctx":262144},"context_length":262144}]});
        assert_eq!(model_context(&maya, "inkling-2-mini"), Some(262144));
        // One model served under another name still counts.
        assert_eq!(model_context(&maya, "inkling"), Some(262144));
        let vllm =
            json!({"data":[{"id":"a","max_model_len":32768},{"id":"b","max_model_len":8192}]});
        assert_eq!(model_context(&vllm, "b"), Some(8192));
        assert_eq!(model_context(&vllm, "c"), None);
        let loading = json!({"data":[{"id":"x","context_length":0}]});
        assert_eq!(model_context(&loading, "x"), None);
        assert!(
            local_host("127.0.0.1")
                && local_host("192.168.1.42")
                && local_host("model-host.local")
        );
        assert!(!local_host("api.openai.com") && !local_host("8.8.8.8"));
    }

    #[test]
    fn reasoning_models_are_recognized_from_catalogs_and_names() {
        assert!(reasoning_name("@ollama-cloud/inkling-2-small:cloud"));
        assert!(reasoning_name("gpt-oss:20b") && reasoning_name("@openai/o3-mini"));
        assert!(reasoning_name("@openrouter/deepseek/deepseek-r1"));
        assert!(!reasoning_name("gemma3:12b") && !reasoning_name("@openai/gpt-4o"));
        assert!(!reasoning_name("@openrouter/meta/llama-3.1-70b"));
        let listed = crate::generation::Limits::from_metadata(
            &json!({"id":"x","supported_parameters":["temperature","reasoning"]}),
            false,
        );
        assert!(listed.reasoning);
        let ollama = crate::generation::Limits::from_metadata(
            &json!({"capabilities":["completion","thinking"]}),
            false,
        );
        assert!(ollama.reasoning);
    }

    #[test]
    fn vision_and_audio_come_from_catalogs_and_names() {
        assert!(
            vision_name("llava:13b") && vision_name("gemma3:12b") && vision_name("@openai/gpt-4o")
        );
        assert!(!vision_name("gemma3:1b") && !vision_name("deepseek-r1:8b"));
        assert!(audio_name("gemma3n:e4b") && audio_name("@openrouter/openai/gpt-4o-audio-preview"));
        assert!(!audio_name("qwen3:8b"));
        let routed = crate::generation::Limits::from_metadata(
            &json!({"architecture":{"input_modalities":["text","image","audio"]}}),
            false,
        );
        assert!(routed.vision && routed.audio);
        let ollama = crate::generation::Limits::from_metadata(
            &json!({"capabilities":["completion","vision"]}),
            false,
        );
        assert!(ollama.vision && !ollama.audio);
    }

    #[test]
    fn catalog_prices_are_read_per_token() {
        let l = crate::generation::Limits::from_metadata(
            &json!({"pricing":{"prompt":"0.000003","completion":0.000015}}),
            false,
        );
        assert_eq!(l.price, Some((0.000003, 0.000015)));
        assert!(
            crate::generation::Limits::from_metadata(&json!({}), false)
                .price
                .is_none()
        );
    }

    #[test]
    fn images_use_each_protocols_content_parts() {
        let m = Message {
            images: vec!["iVBORw0KGgo".into()],
            ..Message::new("user", "What is this?")
        };
        let plain = Message::new("assistant", "A cat.");
        let chat = wire_messages(Protocol::ChatCompletions, [&m, &plain].into_iter());
        assert_eq!(
            chat[0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,iVBORw0KGgo"
        );
        assert_eq!(chat[1]["content"], "A cat.");
        assert!(chat[1].get("images").is_none());
        let claude = wire_messages(Protocol::Messages, [&m].into_iter());
        assert_eq!(claude[0]["content"][0]["source"]["media_type"], "image/png");
        assert_eq!(claude[0]["content"][1]["text"], "What is this?");
        let responses = wire_messages(Protocol::Responses, [&m].into_iter());
        assert_eq!(responses[0]["content"][1]["type"], "input_image");
        // Ollama reads the `images` field directly.
        assert_eq!(
            serde_json::to_value(&m).unwrap()["images"][0],
            "iVBORw0KGgo"
        );
    }

    #[test]
    fn tuning_maps_to_each_protocol() {
        use crate::state::{Thinking, Tuning};
        let t = Tuning {
            thinking: Thinking::High,
            temperature: Some(0.3),
            ..Default::default()
        };
        let mut body = json!({"max_tokens": 4096});
        apply_tuning(Protocol::Messages, &t, &mut body, 4096);
        assert_eq!(body["thinking"]["budget_tokens"], 3072);
        assert_eq!(body["max_tokens"], 4096);
        assert!(body.get("temperature").is_none());
        let mut body = json!({});
        apply_tuning(Protocol::ChatCompletions, &t, &mut body, 4096);
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["temperature"], json!(0.3_f32));
        let off = Tuning {
            thinking: Thinking::Off,
            ..Default::default()
        };
        let mut body = json!({});
        apply_tuning(Protocol::Responses, &off, &mut body, 4096);
        assert_eq!(body["reasoning"]["effort"], "minimal");
        let mut body = json!({});
        apply_tuning(
            Protocol::ChatCompletions,
            &Tuning::default(),
            &mut body,
            4096,
        );
        assert_eq!(body, json!({}));
    }

    #[tokio::test]
    async fn stream_length_signals_preserve_text_and_usage_across_protocols() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        for (protocol, events) in [
            (Protocol::ChatCompletions, vec![
                json!({"choices":[{"delta":{"content":"partial code"},"finish_reason":"length"}]}).to_string(),
                json!({"choices":[],"usage":{"completion_tokens":99}}).to_string(),
                "[DONE]".into(),
            ]),
            (Protocol::Messages, vec![
                json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"partial code"}}).to_string(),
                json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":99}}).to_string(),
                json!({"type":"message_stop"}).to_string(),
            ]),
            (Protocol::Responses, vec![
                json!({"type":"response.output_text.delta","delta":"partial code"}).to_string(),
                json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"},"usage":{"output_tokens":99}}}).to_string(),
            ]),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let p = Provider {
                base: format!("http://{}/v1",listener.local_addr().unwrap()),
                protocol, id:"test".into(), name:"Test".into(), models:vec!["tiny".into()],
                model_limits:[("tiny".into(), Limits {output:Some(2048), context:Some(32768), ..Default::default()})].into(),
                credential:None, catalog_warning:None, needs_auth:false,
                #[cfg(any(windows, target_os = "android"))]
                registration:None,
                #[cfg(any(windows, target_os = "linux"))]
                claude_login:None,
            };
            let server = std::thread::spawn(move || {
                loop {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut input = BufReader::new(&stream);
                    let mut headers = String::new();
                    while !headers.ends_with("\r\n\r\n") { assert!(input.read_line(&mut headers).unwrap() > 0); }
                    let catalog = headers.starts_with("GET ");
                    let (mime, body) = if catalog {
                        ("application/json", json!({"data":[{"id":"tiny","max_output_tokens":2048}]}).to_string())
                    } else {
                        let length = headers.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").and_then(|n| n.parse::<usize>().ok())).unwrap();
                        let mut request = vec![0; length];
                        input.read_exact(&mut request).unwrap();
                        let v: Value = serde_json::from_slice(&request).unwrap();
                        assert_eq!(v.get("max_tokens").or_else(||v.get("max_output_tokens")).unwrap(),2048);
                        ("text/event-stream", events.iter().map(|e|format!("data: {e}\n\n")).collect())
                    };
                    write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
                    if !catalog { break; }
                }
            });
            let mut obs = crate::automation::Observation::default();
            let http = Client::builder().no_proxy().timeout(Duration::from_secs(5)).build().unwrap();
            let answer = p.chat(&http,"tiny",&[Message::new("user","code")],None,100,&mut obs).await.unwrap();
            assert!(obs.output_limited);
            assert_eq!(obs.output,99);
            assert_eq!(answer,"partial code");
            server.join().unwrap();
        }
    }

    #[tokio::test]
    async fn inkling_stream_requires_a_terminal_record_to_accept_generated_code() {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        for finished in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/v1", listener.local_addr().unwrap());
            let provider: Provider = serde_json::from_value(json!({
                "id":"local", "name":"Maya", "base":base, "protocol":"ChatCompletions", "models":["inkling"]
            })).unwrap();
            let worker = std::thread::spawn(move || {
                for path in ["/apply-template", "/completion"] {
                    let (mut socket, _) = listener.accept().unwrap();
                    let mut input = BufReader::new(&socket);
                    let mut headers = String::new();
                    while !headers.ends_with("\r\n\r\n") { assert!(input.read_line(&mut headers).unwrap() > 0); }
                    assert!(headers.starts_with(&format!("POST {path} ")));
                    let length = headers.lines().filter_map(|s| s.split_once(':')).find(|(name,_)| name.eq_ignore_ascii_case("content-length")).unwrap().1.trim().parse().unwrap();
                    let mut payload = vec![0; length];
                    input.read_exact(&mut payload).unwrap();
                    let request: Value = serde_json::from_slice(&payload).unwrap();
                    let mut body = json!({"prompt":"<|message_user|>write code<|end_message|>"}).to_string();
                    let mut mime = "application/json";
                    if path == "/completion" {
                        assert_eq!(request["stream"], true);
                        mime = "text/event-stream";
                        body = format!("data: {}\r\n\r\n", json!({"content":"<html>", "stop":false}));
                        if finished {
                            body.push_str(&format!("data: {}\n\n", json!({"content":"", "stop":true, "stop_type":"limit", "tokens_predicted":4, "tokens_evaluated":12})));
                        }
                    }
                    write!(socket, "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: {mime}\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
                }
            });
            let client = Client::builder().no_proxy().timeout(Duration::from_secs(5)).build().unwrap();
            let mut obs = crate::automation::Observation {output:3,..Default::default()};
            let (preview, _) = tokio::sync::watch::channel(String::new());
            let result = provider.inkling_final(&client, "", &json!({"model":"inkling","messages":[]}), 64, &mut obs, Some(&preview)).await;
            assert_eq!(&*preview.borrow(), "<html>");
            if finished {
                assert_eq!(result.unwrap(), "<html>");
                assert_eq!((obs.input, obs.output, obs.output_limited), (12, 7, true));
            } else {
                assert!(result.unwrap_err().to_string().contains("disconnected"));
            }
            worker.join().unwrap();
        }
    }

    #[test]
    fn connection_destinations_and_qualified_ids() {
        assert!(endpoint("http://localhost:1234/v1/").is_ok());
        assert!(endpoint("http://host.example/v1").is_err());
        assert!(endpoint("https://user:secret@server.example/v1").is_err());
        assert_eq!(route("@p7/vendor/model"), ("p7", "vendor/model"));
        assert_eq!(route("vendor/model"), ("ollama", "vendor/model"));
    }
}
