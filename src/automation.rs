use crate::{
    providers::route,
    state::{Mode, State, Task},
};
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

pub const FIVE_HOURS: i64 = 18000;
pub const WEEK: i64 = 604800;
pub fn now() -> i64 {
    Utc::now().timestamp()
}
pub fn local_time(t: i64) -> String {
    Local
        .timestamp_opt(t, 0)
        .single()
        .map(|v| v.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "Unknown".into())
}
pub fn parse_local(s: &str) -> Result<i64> {
    Local
        .from_local_datetime(
            &NaiveDateTime::parse_from_str(s.trim(), "%Y-%m-%d %H:%M")
                .context("Use YYYY-MM-DD HH:MM in local time")?,
        )
        .single()
        .context("Choose a local time outside the daylight-saving transition")
        .map(|v| v.timestamp())
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Schedule {
    #[serde(default)]
    pub skill: Option<(String, String)>,
    pub id: u64,
    pub title: String,
    pub prompt: String,
    pub model: String,
    pub next: i64,
    pub interval: i64,
    pub enabled: bool,
    pub chat: u64,
    pub chain: Vec<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Rule {
    pub scope: String,
    pub period: i64,
    pub cap: u32,
    pub percent: u32,
    pub used: u32,
    pub reset: i64,
    #[serde(default)]
    pub pause: bool,
}
impl Rule {
    fn hit(&self, model: &str) -> bool {
        (self.scope == model || self.scope == route(model).0)
            && u64::from(self.used) * 100 >= u64::from(self.cap) * u64::from(self.percent)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limit {
    pub reset: Option<i64>,
    pub reason: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Meter {
    pub attempts: u64,
    pub input: u64,
    pub output: u64,
}
#[derive(Default)]
pub struct Observation {
    pub output_budget: Option<u64>,
    pub output_limited: bool,
    pub rate: Option<f64>,
    pub window: Option<u64>,
    pub headers: Vec<String>,
    pub limit: Option<Limit>,
    pub rejected: bool,
    pub input: u64,
    pub output: u64,
    /// Advanced model settings to apply to this request.
    pub tuning: crate::state::Tuning,
}
impl Observation {
    pub fn http(&mut self, response: &reqwest::Response, threshold: u32) {
        self.headers = crate::usage::header_stats(response.headers());
        self.rejected = response.status().as_u16() == 429;
        self.limit = provider_limit(
            response.headers(),
            response.status().as_u16(),
            threshold,
            now(),
        );
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Wake {
    pub task: u64,
    pub at: i64,
    pub reason: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Automation {
    pub cloud_usage: crate::usage::CloudUsage,
    pub provider_stats: BTreeMap<String, (i64, Vec<String>)>,
    pub meters: BTreeMap<String, Meter>,
    pub schedules: Vec<Schedule>,
    pub chain: Vec<String>,
    pub fallback_on_disconnect: bool,
    pub rules: Vec<Rule>,
    pub limits: BTreeMap<String, Limit>,
    pub wake: Option<Wake>,
    pub background: bool,
    pub events: Vec<String>,
    pub reported_percent: u32,
}
impl Default for Automation {
    fn default() -> Self {
        Self {
            cloud_usage: Default::default(),
            provider_stats: Default::default(),
            meters: BTreeMap::new(),
            schedules: vec![],
            chain: vec![],
            fallback_on_disconnect: false,
            rules: vec![],
            limits: BTreeMap::new(),
            wake: None,
            background: false,
            events: vec![],
            reported_percent: 100,
        }
    }
}

impl Automation {
    /// How much of the tightest configured usage budget for this model is spent, 0.0 to 1.0.
    pub fn pressure(&self, model: &str) -> Option<f64> {
        self.rules
            .iter()
            .filter(|r| r.cap > 0 && (r.scope == model || r.scope == route(model).0))
            .map(|r| f64::from(r.used) / f64::from(r.cap))
            .reduce(f64::max)
    }

    pub fn record(&mut self, s: impl AsRef<str>) {
        self.events
            .push(format!("{}  {}", local_time(now()), s.as_ref()));
        if self.events.len() > 64 {
            self.events.remove(0);
        }
    }
    fn roll(&mut self, t: i64) {
        for rule in &mut self.rules {
            if rule.reset <= t {
                rule.reset += ((t - rule.reset) / rule.period + 1) * rule.period;
                rule.used = 0;
            }
        }
        self.limits.retain(|_, v| v.reset.is_none_or(|r| r > t));
    }
    fn limit(&self, model: &str, time: i64) -> Option<Limit> {
        let provider = route(model).0;
        let mut limits = self
            .rules
            .iter()
            .filter(|r| r.hit(model))
            .map(|r| Limit {
                reset: Some(r.reset),
                reason: format!(
                    "{} reached {}% of its {}-request budget",
                    r.scope, r.percent, r.cap
                ),
            })
            .collect::<Vec<_>>();
        if provider == "ollama-cloud" {
            limits.extend(self.cloud_usage.limit(self.reported_percent, time));
        }
        if let Some(l) = self.limits.get(provider) {
            limits.push(l.clone());
        }
        limits
            .into_iter()
            .max_by_key(|l| l.reset.unwrap_or(i64::MAX))
    }
    pub fn select(
        &mut self,
        primary: &str,
        chain: &[String],
        disconnected: &[String],
        t: i64,
    ) -> std::result::Result<String, Limit> {
        self.roll(t);
        let mut blocks = Vec::new();
        for model in std::iter::once(primary).chain(chain.iter().map(String::as_str)) {
            if let Some(limit) = self.limit(model, t) {
                if self.rules.iter().any(|r| r.pause && r.hit(model)) {
                    return Err(limit);
                }
                blocks.push(limit);
            } else if !disconnected.iter().any(|failed| failed == model) {
                return Ok(model.into());
            }
        }
        Err(blocks
            .into_iter()
            .min_by_key(|l| l.reset.unwrap_or(i64::MAX))
            .unwrap_or_else(|| Limit {reset: None, reason: "All eligible models disconnected during this request".into()}))
    }
    pub fn debit(&mut self, model: &str) {
        self.meters.entry(model.into()).or_default().attempts += 1;
        for r in &mut self.rules {
            if r.scope == model || r.scope == route(model).0 {
                r.used = r.used.saturating_add(1);
            }
        }
    }
    pub fn recover(&mut self, t: i64) {
        self.roll(t);
    }
}

pub fn due(state: &mut State, t: i64) -> bool {
    if let Some(w) = &state.automation.wake {
        if state.queue.front().map(|q| q.id) != Some(w.task) {
            state.automation.wake = None;
        } else if w.at <= t {
            state.automation.wake = None;
            state.held = false;
            return true;
        }
    }
    if !state.queue.is_empty() || state.held {
        return false;
    }
    let Some(s) = state
        .automation
        .schedules
        .iter_mut()
        .filter(|s| s.enabled && s.next <= t)
        .min_by_key(|s| (s.next, s.id))
    else {
        return false;
    };
    if s.interval == 0 {
        s.enabled = false;
    } else {
        s.next += ((t - s.next) / s.interval + 1) * s.interval;
    }
    state.serial += 1;
    state.queue.push_back(Task {
        queued_at: Some(now()),
        steps: 0,
        tool_result: false,
        pastes: vec![],
        images: vec![],
        continuation: Default::default(),
        hold_reason: None,
        id: state.serial,
        text: s.prompt.clone(),
        model: s.model.clone(),
        mode: Mode::Chat,
        context: s
            .skill
            .as_ref()
            .map(|(_, definition)| format!("User-selected skill:\n{definition}"))
            .unwrap_or_default(),
        steering: vec![],
        skill: s.skill.clone(),
        chain: s.chain.clone(),
        disconnected: vec![],
        origin: Some(s.chat),
        schedule: Some(s.id),
        actual: None,
    });
    state.automation.record(format!(
        "Started scheduled task #{} ({})",
        state.serial,
        state.queue.back().unwrap().model
    ));
    true
}

pub fn next_tick(state: &State, t: i64) -> std::time::Duration {
    let next = state
        .automation
        .wake
        .iter()
        .map(|w| w.at)
        .chain(
            state
                .automation
                .schedules
                .iter()
                .filter(|s| s.enabled && state.queue.is_empty() && !state.held)
                .map(|s| s.next),
        )
        .chain((state.queue.is_empty() && !state.held).then(|| crate::bobbins::next(state)).flatten())
        .min();
    std::time::Duration::from_secs(
        next.map(|n| n.saturating_sub(t).clamp(1, 60) as u64)
            .unwrap_or(86400),
    )
}

impl crate::engine::Engine {
    pub fn automation_command(&mut self, cmd: &str, args: &str) -> Result<String> {
        let (action, rest) = args.split_once(' ').unwrap_or((args, ""));
        let time = now();
        match (cmd, action) {
            ("/automation", "" | "list") => Ok(self
                .state
                .automation
                .schedules
                .iter()
                .map(|s| {
                    format!(
                        "#{}  {}  {}  {}",
                        s.id,
                        s.title,
                        local_time(s.next),
                        if s.enabled { "enabled" } else { "paused" }
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")),
            ("/automation", "add") => {
                let (title, prompt, model, next, interval): (String, String, String, i64, i64) =
                    serde_json::from_str(rest).context(
                        "Expected [title, prompt, model, first Unix timestamp, interval seconds]",
                    )?;
                ensure!(
                    !title.trim().is_empty()
                        && title.len() <= 160
                        && !prompt.trim().is_empty()
                        && prompt.len() <= 9000,
                    "Enter a title and prompt within the task size limits"
                );
                ensure!(self.models.contains(&model), "Choose an available model");
                let (prompt, skill) =
                    if let Some(invocation) = prompt.trim_start().strip_prefix('/') {
                        let (name, task) = invocation
                            .split_once(char::is_whitespace)
                            .context("Use /skill followed by an assignment")?;
                        let definition =
                            self.state.skill_definition(name).context(
                                "A scheduled slash command must name an available skill",
                            )?;
                        ensure!(
                            !task.trim().is_empty() && task.len() + definition.len() <= 9000,
                            "Supply an assignment within the skill’s input limit"
                        );
                        (
                            task.trim().to_owned(),
                            Some((name.to_owned(), definition.to_owned())),
                        )
                    } else {
                        (prompt, None)
                    };
                ensure!(
                    next > time
                        && next <= time + 10 * 365 * 86400
                        && (interval == 0 || (60..=365 * 86400).contains(&interval)),
                    "Choose a future time within ten years and a repeat interval of one minute to one year"
                );
                self.state.serial += 1;
                let id = self.state.serial;
                self.state.automation.schedules.push(Schedule {
                    skill,
                    id,
                    title,
                    prompt,
                    model,
                    next,
                    interval,
                    enabled: true,
                    chat: self.state.chat_id,
                    chain: self.state.automation.chain.clone(),
                });
                Ok(format!("Scheduled #{id} for {}", local_time(next)))
            }
            ("/automation", "enable" | "pause" | "remove") => {
                let id = rest.parse::<u64>()?;
                let schedules = &mut self.state.automation.schedules;
                let index = schedules
                    .iter()
                    .position(|s| s.id == id)
                    .context("Unknown schedule")?;
                if action == "remove" {
                    schedules.remove(index);
                } else {
                    ensure!(
                        action == "pause"
                            || schedules[index].interval > 0
                            || schedules[index].next > time,
                        "Choose a new time for a past one-time schedule"
                    );
                    schedules[index].enabled = action == "enable";
                }
                Ok(format!("Updated schedule #{id}"))
            }
            ("/automation", "background") => {
                ensure!(
                    matches!(rest, "on" | "off"),
                    "Use /automation background on or off"
                );
                let enabled = rest == "on";
                background(&self.root, enabled)?;
                self.state.automation.background = enabled;
                Ok("Windows background scheduling updated".into())
            }
            ("/defer", _) => {
                let at = parse_local(args)?;
                ensure!(at > time, "Choose a future time");
                let task = self.state.queue.front().context("No outstanding task")?.id;
                self.state.held = true;
                self.state.automation.wake = Some(Wake {
                    task,
                    at,
                    reason: "Scheduled by you".into(),
                });
                Ok(format!("Held task #{task} until {}", local_time(at)))
            }
            ("/fallback", "") => Ok(format!(
                "Fallback order: {}",
                self.state.automation.chain.join(" → ")
            )),
            ("/fallback", "set" | "clear") => {
                let chain: Vec<String> = if action == "clear" {
                    vec![]
                } else {
                    rest.split_whitespace().map(String::from).collect()
                };
                for (i, m) in chain.iter().enumerate() {
                    ensure!(
                        self.models.contains(m) && !chain[..i].contains(m),
                        "Choose distinct, connected fallback models"
                    );
                }
                self.state.automation.chain = chain;
                Ok("Fallback order saved for new tasks and schedules".into())
            }
            ("/fallback", "disconnects") => {
                self.state.automation.fallback_on_disconnect = match rest {
                    "on" => true,
                    "off" => false,
                    _ => bail!("Use /fallback disconnects on or off"),
                };
                Ok(format!("Fallback on provider disconnects is {rest}"))
            }
            ("/quota", "threshold") => {
                let n = rest.parse()?;
                ensure!((1..=100).contains(&n), "Enter a percentage from 1 to 100");
                self.state.automation.reported_percent = n;
                Ok(format!("Hold at {n}% reported usage"))
            }
            ("/quota", "set") => {
                let r: Rule = serde_json::from_str(rest)?;
                ensure!(
                    self.models
                        .iter()
                        .any(|m| *m == r.scope || route(m).0 == r.scope),
                    "Choose a known provider or model"
                );
                ensure!(
                    matches!(r.period, FIVE_HOURS | WEEK)
                        && r.cap > 0
                        && (1..=100).contains(&r.percent)
                        && r.reset > time
                        && r.reset <= time + r.period + 60,
                    "Check the cap, percentage and next reset within the selected window"
                );
                self.state
                    .automation
                    .rules
                    .retain(|v| v.scope != r.scope || v.period != r.period);
                self.state.automation.rules.push(r);
                Ok("Request budget saved".into())
            }
            ("/quota", "remove") => {
                let (scope, period) = rest.rsplit_once(' ').context("Choose a budget to remove")?;
                let period = period.parse::<i64>()?;
                self.state
                    .automation
                    .rules
                    .retain(|r| r.scope != scope || r.period != period);
                Ok("Request budget removed".into())
            }
            ("/quota", "clear") => {
                ensure!(
                    self.state.automation.limits.remove(rest).is_some(),
                    "Choose an active provider hold"
                );
                Ok("Provider hold cleared. Resume the queue to retry.".into())
            }
            _ => anyhow::bail!(
                "Use the Automations page for schedules, fallback order and usage limits"
            ),
        }
    }
}

fn duration_seconds(value: &str) -> Option<i64> {
    let mut rest = value;
    let mut total = 0f64;
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        let n = rest[..end].parse::<f64>().ok()?;
        rest = &rest[end..];
        let (unit, scale) = if rest.starts_with("ms") {
            ("ms", 0.001)
        } else if rest.starts_with('s') {
            ("s", 1.)
        } else if rest.starts_with('m') {
            ("m", 60.)
        } else if rest.starts_with('h') {
            ("h", 3600.)
        } else if rest.starts_with('d') {
            ("d", 86400.)
        } else if rest.is_empty() {
            ("", 1.)
        } else {
            return None;
        };
        total += n * scale;
        rest = &rest[unit.len()..];
    }
    Some((total.ceil() as i64).clamp(1, 31_536_000))
}
fn reset_time(v: &str, t: i64) -> Option<i64> {
    DateTime::parse_from_rfc3339(v)
        .or_else(|_| DateTime::parse_from_rfc2822(v))
        .map(|d| d.timestamp().max(t + 1))
        .ok()
        .or_else(|| duration_seconds(v).map(|n| t + n))
}
pub fn provider_limit(
    headers: &reqwest::header::HeaderMap,
    status: u16,
    threshold: u32,
    t: i64,
) -> Option<Limit> {
    let val = |k: &str| headers.get(k).and_then(|v| v.to_str().ok());
    let mut resets = vec![];
    let mut hit = false;
    for window in ["primary", "secondary"] {
        if val(&format!("x-codex-{window}-used-percent"))
            .and_then(|v| v.parse::<f64>().ok())
            .is_some_and(|n| n >= f64::from(threshold))
        {
            hit = true;
            resets.push(val(&format!("x-codex-{window}-reset-at")).and_then(|v| {
                v.parse::<i64>().ok().map(|n| n.max(t + 1)).or_else(|| {
                    DateTime::parse_from_rfc3339(v)
                        .ok()
                        .map(|d| d.timestamp().max(t + 1))
                })
            }));
        }
    }
    for prefix in ["x-ratelimit", "anthropic-ratelimit"] {
        for unit in ["requests", "tokens", "input-tokens", "output-tokens"] {
            let name = |field: &str| {
                if prefix == "x-ratelimit" {
                    format!("{prefix}-{field}-{unit}")
                } else {
                    format!("{prefix}-{unit}-{field}")
                }
            };
            let cap = val(&name("limit")).and_then(|n| n.parse::<f64>().ok());
            let left = val(&name("remaining")).and_then(|n| n.parse::<f64>().ok());
            if left.is_some_and(|l| {
                l <= 0. || cap.is_some_and(|c| c > 0. && (c - l) * 100. >= c * f64::from(threshold))
            }) {
                hit = true;
                resets.push(val(&name("reset")).and_then(|s| reset_time(s, t)));
            }
        }
    }
    let retry = val("retry-after").and_then(|v| reset_time(v, t));
    if status != 429 && !hit {
        return None;
    }
    let reset = if retry.is_none() && resets.iter().any(Option::is_none) {
        None
    } else {
        resets.into_iter().flatten().chain(retry).max()
    };
    Some(Limit {
        reset,
        reason: if status == 429 {
            "Provider rate limit (HTTP 429)".into()
        } else {
            format!("Provider-reported usage reached {threshold}%")
        },
    })
}

pub fn background(root: &Path, enable: bool) -> Result<()> {
    #[cfg(windows)]
    {
        use std::{
            hash::{DefaultHasher, Hash, Hasher},
            os::windows::process::CommandExt,
        };
        let mut h = DefaultHasher::new();
        root.hash(&mut h);
        std::env::var_os("LOCALAPPDATA").hash(&mut h);
        let name = format!("Spindle-Project-{:016x}", h.finish());
        let mut c = std::process::Command::new(
            std::path::PathBuf::from(
                std::env::var_os("WINDIR").context("Windows directory unavailable")?,
            )
            .join("System32/schtasks.exe"),
        );
        c.creation_flags(0x08000000);
        if enable {
            let action = format!(
                "\"{}\" --run-due \"{}\"",
                std::env::current_exe()?.display(),
                crate::projects::display_path(root)
            );
            c.args([
                "/Create", "/TN", &name, "/SC", "MINUTE", "/MO", "1", "/IT", "/RL", "LIMITED",
                "/TR", &action, "/F",
            ]);
        } else {
            c.args(["/Delete", "/TN", &name, "/F"]);
        }
        let out = c.output()?;
        ensure!(
            out.status.success(),
            "Windows Task Scheduler: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (root, enable);
        anyhow::bail!("Background scheduling requires Windows");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rule(scope: &str, period: i64, reset: i64, pause: bool) -> Rule {
        Rule {
            scope: scope.into(),
            period,
            cap: 10,
            percent: 80,
            used: 8,
            reset,
            pause,
        }
    }
    #[test]
    fn fallback_and_earliest_eligible_reset() {
        let mut auto = Automation::default();
        auto.rules = vec![
            rule("p1", FIVE_HOURS, 150, false),
            rule("p1", WEEK, 320, false),
        ];
        let chain = vec!["@p2/b".into(), "@ollama-cloud/c".into()];
        assert_eq!(auto.select("@p1/a", &chain, &[], 100).unwrap(), "@p2/b");
        auto.rules.push(rule("p2", FIVE_HOURS, 200, false));
        auto.limits.insert(
            "ollama-cloud".into(),
            Limit {
                reset: None,
                reason: "unknown".into(),
            },
        );
        assert_eq!(
            auto.select("@p1/a", &chain, &[], 100).unwrap_err().reset,
            Some(200)
        );
        auto.rules[1].pause = true;
        assert_eq!(
            auto.select("@p1/a", &chain, &[], 100).unwrap_err().reset,
            Some(320)
        );
        assert_eq!(auto.select("@p1/a", &chain, &[], 320).unwrap(), "@p1/a");
        auto.debit("@p1/a");
        assert_eq!(auto.rules[0].used, 1);
        assert_eq!(auto.rules[1].used, 1);
    }
    #[test]
    fn schedule_coalesces_and_remembers_chat() {
        let mut s = State::default();
        s.automation.schedules.push(Schedule {
            skill: None,
            id: 4,
            title: "Review".into(),
            prompt: "Review progress".into(),
            model: "one".into(),
            next: 100,
            interval: 60,
            enabled: true,
            chat: 0,
            chain: vec!["two".into()],
        });
        s.switch_chat(None).unwrap();
        assert!(s.chats.contains_key(&0));
        assert!(due(&mut s, 265));
        assert_eq!(s.automation.schedules[0].next, 280);
        assert!(!due(&mut s, 269));
        let task = &s.queue[0];
        assert_eq!(task.origin, Some(0));
        assert_eq!(task.chain, ["two"]);
        s.held = true;
        s.automation.wake = Some(Wake {
            task: task.id,
            at: 400,
            reason: "held".into(),
        });
        assert!(!due(&mut s, 399));
        assert!(due(&mut s, 400));
        assert!(!s.held);
    }
    #[test]
    fn retry_after_and_reported_thresholds() {
        use reqwest::header::HeaderMap;
        let mut h = HeaderMap::new();
        assert!(provider_limit(&h, 200, 80, 100).is_none());
        assert_eq!(provider_limit(&h, 429, 80, 100).unwrap().reset, None);
        h.insert("retry-after", "17".parse().unwrap());
        assert_eq!(provider_limit(&h, 429, 80, 100).unwrap().reset, Some(117));
        h.clear();
        h.insert("x-ratelimit-limit-requests", "50".parse().unwrap());
        h.insert("x-ratelimit-remaining-requests", "10".parse().unwrap());
        h.insert("x-ratelimit-reset-requests", "5h4m".parse().unwrap());
        assert_eq!(provider_limit(&h, 200, 80, 100).unwrap().reset, Some(18340));
        assert!(provider_limit(&h, 200, 90, 100).is_none());
    }
}
