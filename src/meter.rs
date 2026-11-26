//! Usage rings: how much of a subscription window or a money budget is spent.
//!
//! Subscription routes (Ollama Cloud windows, request budgets from Automations) report the
//! tightest window. Paid API providers have a money budget per day, week or month. The
//! budget defaults to $50 a day so the ring appears before anyone configures it.

use crate::state::State;
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Period {
    #[default]
    Day,
    Week,
    Month,
}

impl Period {
    pub const ALL: [Period; 3] = [Period::Day, Period::Week, Period::Month];
    pub fn label(self) -> &'static str {
        match self {
            Period::Day => "per day",
            Period::Week => "per week",
            Period::Month => "per month",
        }
    }
    pub fn this(self) -> &'static str {
        match self {
            Period::Day => "Today",
            Period::Week => "This week",
            Period::Month => "This month",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Budget {
    pub dollars: f64,
    pub period: Period,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            dollars: 50.,
            period: Period::Day,
        }
    }
}

fn midnight(date: NaiveDate) -> i64 {
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .map_or(0, |d| d.timestamp())
}

/// Start of the current period and the next reset, in local time.
pub fn bounds(period: Period, now: i64) -> (i64, i64) {
    let today = Local
        .timestamp_opt(now, 0)
        .single()
        .unwrap_or_else(Local::now)
        .date_naive();
    let (start, next) = match period {
        Period::Day => (today, today + Duration::days(1)),
        Period::Week => {
            let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
            (monday, monday + Duration::days(7))
        }
        Period::Month => {
            let first = today.with_day(1).unwrap();
            let next = if first.month() == 12 {
                NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
            }
            .unwrap();
            (first, next)
        }
    };
    (midnight(start), midnight(next))
}

/// A money budget and what is left of it this period.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spend {
    pub budget: Budget,
    pub spent: f64,
    /// Credits left with the provider, when it reports them.
    pub balance: Option<f64>,
    pub reset: i64,
}

impl Spend {
    /// Money that can still be spent this period: the rest of the budget, or less if the
    /// account's credits run out first.
    pub fn available(&self) -> f64 {
        let rest = (self.budget.dollars - self.spent).max(0.);
        self.balance.map_or(rest, |b| rest.min(b.max(0.)))
    }
    /// Credits cannot cover the rest of this period's budget.
    pub fn credit_limited(&self) -> bool {
        self.balance
            .is_some_and(|b| b.max(0.) < (self.budget.dollars - self.spent).max(0.))
    }
    /// How full the ring is, 0.0 to 1.0.
    pub fn fill(&self) -> f32 {
        if self.budget.dollars <= 0. {
            return 1.;
        }
        (1. - self.available() / self.budget.dollars).clamp(0., 1.) as f32
    }
}

pub fn spend(state: &State, provider: &str, now: i64) -> Spend {
    let budget = state.budgets.get(provider).copied().unwrap_or_default();
    let (start, reset) = bounds(budget.period, now);
    let spent = state
        .spend
        .get(provider)
        .map(|entries| {
            entries
                .iter()
                .filter(|(t, _)| *t >= start)
                .map(|(_, d)| d)
                .sum()
        })
        .unwrap_or(0.);
    Spend {
        budget,
        spent,
        balance: state.balances.get(provider).copied(),
        reset,
    }
}

/// Record money spent with a provider. Keeps about six weeks of entries.
pub fn record(state: &mut State, provider: &str, now: i64, dollars: f64) {
    if !(dollars > 0. && dollars.is_finite()) {
        return;
    }
    let entries = state.spend.entry(provider.to_owned()).or_default();
    entries.retain(|(t, _)| *t >= now - 42 * 86_400);
    entries.push((now, dollars));
}

/// A subscription or quota window: its name, fraction used and next reset.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub name: String,
    pub used: f32,
    pub reset: Option<i64>,
}

/// Quota windows that apply to a model, tightest first.
pub fn windows(state: &State, model: &str) -> Vec<Window> {
    let scope = crate::providers::route(model).0;
    let mut out = vec![];
    if scope == "ollama-cloud" {
        out.extend(state.automation.cloud_usage.windows.iter().map(|w| Window {
            name: w.name.clone(),
            used: (w.used / 100.).clamp(0., 1.) as f32,
            reset: w.reset,
        }));
    }
    out.extend(
        state
            .automation
            .rules
            .iter()
            .filter(|r| r.cap > 0 && (r.scope == model || r.scope == scope))
            .map(|r| Window {
                name: format!(
                    "{} budget",
                    if r.period <= 5 * 3600 {
                        "5-hour"
                    } else {
                        "Weekly"
                    }
                ),
                used: (f64::from(r.used) / f64::from(r.cap)).clamp(0., 1.) as f32,
                reset: Some(r.reset).filter(|t| *t > 0),
            }),
    );
    out.sort_by(|a, b| b.used.total_cmp(&a.used));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_limited_by_credits_and_resets_each_period() {
        let mut state = State::default();
        let now = Local::now().timestamp();
        // Default $50 a day, nothing spent: an empty ring.
        let s = spend(&state, "openrouter", now);
        assert_eq!(s.budget, Budget::default());
        assert_eq!(s.fill(), 0.);
        // Only $25 of credit: half the day's budget is unavailable even with no use.
        state.balances.insert("openrouter".into(), 25.);
        let s = spend(&state, "openrouter", now);
        assert!(s.credit_limited());
        assert_eq!(s.available(), 25.);
        assert!((s.fill() - 0.5).abs() < 1e-6);
        // Spending $10 today leaves $15 of credit, which still binds.
        record(&mut state, "openrouter", now, 10.);
        assert!((spend(&state, "openrouter", now).available() - 25.).abs() < 1e-9);
        state.balances.insert("openrouter".into(), 15.);
        let s = spend(&state, "openrouter", now);
        assert!((s.available() - 15.).abs() < 1e-9 && s.credit_limited());
        // Yesterday's spending does not count against today.
        state.balances.clear();
        record(&mut state, "openrouter", now - 2 * 86_400, 40.);
        assert!((spend(&state, "openrouter", now).spent - 10.).abs() < 1e-9);
        let (start, reset) = bounds(Period::Day, now);
        assert!(start <= now && now < reset && reset - start <= 25 * 3600);
        let (start, reset) = bounds(Period::Month, now);
        assert!(start <= now && now < reset);
    }

    #[test]
    fn tightest_window_comes_first() {
        let mut state = State::default();
        state.automation.cloud_usage.windows = vec![
            crate::usage::Window {
                name: "Weekly".into(),
                used: 30.,
                reset: None,
            },
            crate::usage::Window {
                name: "5-hour".into(),
                used: 80.,
                reset: Some(1),
            },
        ];
        let w = windows(&state, "@ollama-cloud/big");
        assert_eq!(w[0].name, "5-hour");
        assert!((w[0].used - 0.8).abs() < 1e-6);
        assert!(windows(&state, "llama3:8b").is_empty());
    }
}
