//! Living plans: a checklist kept with each conversation.
//!
//! The plan is stored with the conversation, not the model, and the current checklist is
//! added to every request, so switching models continues the same plan. The model updates it
//! by ending a reply with a fenced `spindle-plan` block holding the whole list; the user can
//! edit it in the side panel. Accepting or editing a plan never grants any permission.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub text: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub done: bool,
}

/// What changed in one update, for the "Plan updated" note.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    /// Steps (by position in the new list) that were added or changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<usize>,
    #[serde(default)]
    pub added: usize,
    #[serde(default)]
    pub completed: usize,
    #[serde(default)]
    pub reopened: usize,
    #[serde(default)]
    pub removed: usize,
    #[serde(default)]
    pub edited: usize,
}

const MAX_STEPS: usize = 40;
const MAX_STEP: usize = 240;

pub const INSTRUCTION: &str = " For work with several steps, keep a plan: end your reply with a fenced block labeled spindle-plan listing every step on its own line as - [ ] step or - [x] step. Send the whole list again whenever you finish, add, reword or remove a step, and leave the block out when nothing changed.";

impl Update {
    pub fn is_empty(&self) -> bool {
        self.added + self.completed + self.reopened + self.removed + self.edited == 0
            && self.changed.is_empty()
    }

    pub fn summary(&self) -> String {
        let mut parts = vec![];
        for (n, word) in [
            (self.completed, "done"),
            (self.added, "added"),
            (self.edited, "edited"),
            (self.reopened, "reopened"),
            (self.removed, "removed"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {word}"));
            }
        }
        if parts.is_empty() {
            "steps edited".into()
        } else {
            parts.join(", ")
        }
    }

    /// Fold a later update into this one, keeping positions from the later list.
    pub fn merge(&mut self, later: Update) {
        self.added += later.added;
        self.completed += later.completed;
        self.reopened += later.reopened;
        self.removed += later.removed;
        self.edited += later.edited;
        self.changed = later.changed;
    }
}

/// Compare two checklists.
pub fn diff(before: &[Step], after: &[Step]) -> Update {
    let mut update = Update::default();
    for (i, step) in after.iter().enumerate() {
        match before.iter().find(|b| b.text == step.text) {
            Some(old) if old.done == step.done => {}
            Some(old) => {
                update.changed.push(i);
                if step.done && !old.done {
                    update.completed += 1;
                } else {
                    update.reopened += 1;
                }
            }
            None => {
                update.changed.push(i);
                update.added += 1;
                if step.done {
                    update.completed += 1;
                }
            }
        }
    }
    update.removed = before
        .iter()
        .filter(|b| !after.iter().any(|a| a.text == b.text))
        .count();
    // A reworded step reads as one edit rather than an addition and a removal.
    let reworded = update.added.min(update.removed);
    update.added -= reworded;
    update.removed -= reworded;
    update.edited = reworded;
    if update.is_empty() && before != after {
        update.changed = before.iter().zip(after).enumerate()
            .filter_map(|(i, (old, new))| (old != new).then_some(i)).collect();
    }
    update
}

pub fn checklist(steps: &[Step]) -> String {
    steps.iter().map(|s| format!("- [{}] {}", if s.done { "x" } else { " " }, s.text))
        .collect::<Vec<_>>().join("\n")
}

pub fn check_amendment(changes: &[String], text: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!text.trim().is_empty() && text.len() <= 1500, "Describe the change in 1 to 1500 bytes");
    anyhow::ensure!(changes.iter().map(String::len).sum::<usize>() + text.len() <= 6000,
        "The plan has a long revision history. Use /goal replace with the current objective.");
    Ok(())
}

/// The plan as it is sent to the model every turn.
pub fn prompt(steps: &[Step], user_edited: bool) -> Option<String> {
    if steps.is_empty() {
        return None;
    }
    let done = steps.iter().filter(|s| s.done).count();
    let mut text = format!(
        "\n\nPlan for this conversation ({done} of {} done). It is kept across model changes; continue from it.",
        steps.len()
    );
    if user_edited {
        text.push_str(" The user edited it since your last reply.");
    }
    for step in steps {
        text.push_str(&format!(
            "\n- [{}] {}",
            if step.done { "x" } else { " " },
            step.text
        ));
    }
    Some(text)
}

fn step(line: &str) -> Option<Step> {
    let line = line.trim();
    let rest = line
        .strip_prefix(['-', '*', '+', '•'])
        .map(str::trim_start)
        .or_else(|| {
            let digits = line.find(|c: char| !c.is_ascii_digit())?;
            (digits > 0)
                .then(|| line[digits..].strip_prefix(['.', ')']))
                .flatten()
                .map(str::trim_start)
        })?;
    let (done, text) = if let Some(t) = rest.strip_prefix("[x]").or(rest.strip_prefix("[X]")) {
        (true, t)
    } else if let Some(t) = rest.strip_prefix("[ ]") {
        (false, t)
    } else {
        (false, rest)
    };
    let text = text.trim().trim_matches('*').trim();
    (!text.is_empty()).then(|| Step {
        text: text.chars().take(MAX_STEP).collect(),
        done,
    })
}

/// Checklist lines in free text, such as a drafted plan.
pub fn parse(text: &str) -> Vec<Step> {
    let mut steps: Vec<Step> = vec![];
    for line in text.lines() {
        // Only top-level items; indented lines are details of the step above.
        if line.starts_with([' ', '\t']) {
            continue;
        }
        if let Some(s) = step(line) {
            if !steps.iter().any(|x| x.text == s.text) && steps.len() < MAX_STEPS {
                steps.push(s);
            }
        }
    }
    steps
}

/// Remove the last `spindle-plan` block from a reply, returning the reply and its checklist.
pub fn extract(answer: &str) -> (String, Option<Vec<Step>>) {
    use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};
    let mut cut = vec![];
    let mut found = None;
    let mut events = Parser::new(answer).into_offset_iter();
    while let Some((event, range)) = events.next() {
        let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) = event else {
            continue;
        };
        if info.trim() != "spindle-plan" {
            continue;
        }
        let mut end = range.end;
        let mut body = String::new();
        for (inner, r) in events.by_ref() {
            match inner {
                Event::End(TagEnd::CodeBlock) => {
                    end = r.end;
                    break;
                }
                Event::Text(t) => body.push_str(&t),
                _ => {}
            }
        }
        cut.push(range.start..end);
        let steps = parse(&body);
        if !steps.is_empty() {
            found = Some(steps);
        }
    }
    let mut cleaned = answer.to_owned();
    for range in cut.into_iter().rev() {
        cleaned.replace_range(range, "");
    }
    (cleaned.trim_end().to_owned(), found)
}

/// Check a list edited by the user.
pub fn validate(steps: Vec<Step>) -> anyhow::Result<Vec<Step>> {
    anyhow::ensure!(
        steps.len() <= MAX_STEPS,
        "A plan can have up to {MAX_STEPS} steps"
    );
    Ok(steps
        .into_iter()
        .map(|s| Step {
            text: s.text.trim().chars().take(MAX_STEP).collect(),
            done: s.done,
        })
        .filter(|s| !s.text.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_round_trip_and_report_changes() {
        let reply = "Started.\n\n```spindle-plan\n- [x] Read the brief\n- [ ] Draft the outline\n- [ ] Review\n```";
        let (clean, steps) = extract(reply);
        assert_eq!(clean, "Started.");
        let steps = steps.unwrap();
        assert_eq!(steps.len(), 3);
        assert!(steps[0].done && !steps[1].done);
        let mut next = steps.clone();
        next[1].done = true;
        next.push(Step {
            text: "Publish".into(),
            done: false,
        });
        next.remove(2);
        let update = diff(&steps, &next);
        assert_eq!((update.completed, update.added, update.removed), (1, 0, 0));
        assert_eq!(update.edited, 1);
        assert_eq!(update.changed, vec![1, 2]);
        assert_eq!(update.summary(), "1 done, 1 edited");
        let text = prompt(&next, true).unwrap();
        assert!(text.contains("2 of 3 done") && text.contains("- [ ] Publish"));
        assert!(text.contains("edited"));
        let drafted =
            parse("Here is the plan:\n1. Gather data\n   - detail\n2) Clean it\n3. Report");
        assert_eq!(drafted.len(), 3);
        assert_eq!(drafted[1].text, "Clean it");
        assert!(prompt(&[], false).is_none());
    }
}
