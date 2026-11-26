use crate::automation::{Limit, now};
use anyhow::{Context, Result, ensure};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CloudUsage {
    pub checked: i64,
    pub requests: Option<u64>,
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub purchased: Option<f64>,
    pub included: Option<f64>,
    pub windows: Vec<Window>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Window {
    pub name: String,
    pub used: f64,
    pub reset: Option<i64>,
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}
fn reset(v: &Value) -> Option<i64> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|d| d.timestamp())
}

impl CloudUsage {
    pub fn summary(&self) -> String {
        let mut rows = vec!["Ollama Cloud usage".to_owned()];
        if let Some(n) = self.requests {
            rows.push(format!("7 days: {n} requests"));
        }
        for w in &self.windows {
            rows.push(format!(
                "{}: {:.2}% used{}",
                w.name,
                w.used,
                w.reset
                    .map(|r| format!(" · resets {}", crate::automation::local_time(r)))
                    .unwrap_or_default()
            ));
        }
        rows.extend(self.warnings.iter().cloned());
        rows.join("\n")
    }
    fn balance(&mut self, v: &Value) -> Result<()> {
        ensure!(
            v.get("included").is_some() || v.get("purchased").is_some(),
            "No account balance reported"
        );
        let included = &v["included"];
        for (key, name) in [
            ("session", "Session"),
            ("weekly", "Weekly"),
            ("monthly", "Monthly"),
        ] {
            let item = &included[key];
            if let Some(left) =
                number(&item["remaining_percent"]).filter(|n| (0.0..=100.0).contains(n))
            {
                self.windows.push(Window {
                    name: name.into(),
                    used: 100.0 - left,
                    reset: reset(&item["resets_at"]),
                });
            }
        }
        self.purchased = number(&v["purchased"]["balance_usd"]);
        self.included = number(&included["balance_usd"]);
        if let (Some(left), Some(allowance)) = (
            self.included,
            number(&included["allowance_usd"]).filter(|v| *v > 0.0),
        ) {
            self.windows.push(Window {
                name: "Included credit".into(),
                used: (100.0 * (1. - left / allowance)).clamp(0., 100.),
                reset: reset(&included["period"]["until"])
                    .or_else(|| reset(&included["resets_at"])),
            });
        }
        Ok(())
    }
    pub fn limit(&self, threshold: u32, time: i64) -> Option<Limit> {
        self.windows
            .iter()
            .filter(|w| w.used >= f64::from(threshold) && w.reset.is_none_or(|t| t > time))
            .map(|w| Limit {
                reason: format!(
                    "Ollama Cloud {} usage {:.2}% (hold at {threshold}%)",
                    w.name, w.used
                ),
                reset: w.reset,
            })
            .max_by_key(|l| l.reset.unwrap_or(i64::MAX))
    }
}

async fn get(client: &reqwest::Client, path: &str) -> Result<Value> {
    Ok(client
        .get(format!("http://127.0.0.1:11434/api/{path}"))
        .timeout(std::time::Duration::from_secs(7))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

pub async fn pull(client: &reqwest::Client, mut previous: CloudUsage) -> CloudUsage {
    let (usage, balance) = tokio::join!(
        get(client, "usage?range=7d&scope=self"),
        get(client, "balance")
    );
    previous.warnings.clear();
    let mut changed = false;
    match usage.and_then(|v| v.get("totals").cloned().context("Missing usage totals")) {
        Ok(v) => {
            previous.requests = v["request_count"].as_u64();
            previous.input = v["input_tokens"].as_u64();
            previous.output = v["output_tokens"].as_u64();
            changed = true;
        }
        Err(_) => previous
            .warnings
            .push("Seven-day totals unavailable from local Ollama".into()),
    }
    let mut fresh = CloudUsage::default();
    match balance.and_then(|v| fresh.balance(&v)) {
        Ok(()) => {
            previous.windows = fresh.windows;
            previous.included = fresh.included;
            previous.purchased = fresh.purchased;
            changed = true;
        }
        Err(_) => previous.warnings.push(
            "Balance unavailable from local Ollama; any displayed balance is the last received"
                .into(),
        ),
    }
    if changed {
        previous.checked = now();
    }
    previous
}

pub fn header_stats(headers: &reqwest::header::HeaderMap) -> Vec<String> {
    let mut result = Vec::new();
    for unit in ["requests", "tokens", "input-tokens", "output-tokens"] {
        for prefix in ["x-ratelimit", "anthropic-ratelimit"] {
            let name = |field: &str| {
                if prefix == "x-ratelimit" {
                    format!("{prefix}-{field}-{unit}")
                } else {
                    format!("{prefix}-{unit}-{field}")
                }
            };
            let value = |field: &str| headers.get(name(field))?.to_str().ok()?.parse::<u64>().ok();
            if let (Some(remaining), Some(cap)) = (value("remaining"), value("limit")) {
                result.push(format!("{remaining} / {cap} {unit} remaining"));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reported_windows_hold_cloud_until_the_latest_reset() {
        let mut stats = CloudUsage::default();
        stats.balance(&serde_json::json!({"included":{"session":{"remaining_percent":9.0,"resets_at":"2030-01-01T00:00:00Z"},"weekly":{"remaining_percent":2.0,"resets_at":"2030-01-04T00:00:00Z"}},"purchased":{"balance_usd":"1.25"}})).unwrap();
        assert_eq!(stats.purchased, Some(1.25));
        assert!(stats.limit(99, 0).is_none());
        assert!(stats.limit(90, 0).unwrap().reason.contains("Weekly"));
        assert!(stats.limit(90, 2_000_000_000).is_none());
    }
}
