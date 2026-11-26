use crate::ollama::Message;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub fn message_tokens(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|m| m.content.len().div_ceil(3) as u64 + 12)
        .sum()
}

pub const DEFAULT_OUTPUT: u64 = 16_384;
const MAX_REPLY_BYTES: usize = 512 * 1024;
const CONTEXT_TAIL: usize = 6_144;
const AUTO_SEGMENTS: u8 = 4;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Limits {
    pub output: Option<u64>,
    pub context: Option<u64>,
    /// The provider reports that this model can reason before answering.
    #[serde(default)]
    pub reasoning: bool,
    /// Accepts image input.
    #[serde(default)]
    pub vision: bool,
    /// Accepts audio input.
    #[serde(default)]
    pub audio: bool,
    /// Dollars per input and output token, when the catalog publishes prices.
    #[serde(default)]
    pub price: Option<(f64, f64)>,
}

impl Limits {
    pub fn from_metadata(v: &Value, anthropic: bool) -> Self {
        let info = &v["model_info"];
        let arch = info["general.architecture"].as_str().unwrap_or("");
        let positive = |v: &Value| v.as_u64().filter(|n| *n > 0);
        let output = [
            &v["max_output_tokens"],
            &v["max_completion_tokens"],
            &v["top_provider"]["max_completion_tokens"],
            &info["max_output_tokens"],
            &info[format!("{arch}.max_output_tokens")],
        ]
        .into_iter()
        .find_map(positive)
        .or_else(|| anthropic.then(|| positive(&v["max_tokens"])).flatten());
        let context = positive(&info[format!("{arch}.context_length")])
            .or_else(|| positive(&v["context_length"]))
            .or_else(|| positive(&v["context_window"]));
        let listed = |list: &Value, word: &str| {
            list.as_array()
                .is_some_and(|a| a.iter().any(|s| s.as_str() == Some(word)))
        };
        // Ollama lists "thinking" in capabilities; OpenRouter lists "reasoning" among
        // supported parameters.
        let reasoning = listed(&v["capabilities"], "thinking")
            || listed(&v["supported_parameters"], "reasoning")
            || listed(&v["supported_parameters"], "include_reasoning")
            || listed(&v["supported_parameters"], "reasoning_effort");
        let modality = |word: &str| {
            listed(&v["architecture"]["input_modalities"], word)
                || listed(&v["input_modalities"], word)
                || listed(&v["modalities"]["input"], word)
        };
        Self {
            output,
            context,
            reasoning,
            vision: listed(&v["capabilities"], "vision") || modality("image"),
            audio: listed(&v["capabilities"], "audio") || modality("audio"),
            price: {
                let rate = |v: &Value| {
                    v.as_f64()
                        .or_else(|| v.as_str()?.parse().ok())
                        .filter(|n: &f64| n.is_finite() && *n >= 0.)
                };
                rate(&v["pricing"]["prompt"]).zip(rate(&v["pricing"]["completion"]))
            },
        }
    }

    pub fn allowance(&self, context: Option<u64>, messages: &[Message]) -> u64 {
        let mut budget = self.output.unwrap_or(DEFAULT_OUTPUT).min(DEFAULT_OUTPUT);
        if let Some(window) = context.or(self.context) {
            // Reserve prompt space in a shared context. The runtime owns the exact token count.
            let estimated_input = message_tokens(messages) + 64;
            budget = budget.min(window.saturating_sub(estimated_input).max(1));
        }
        budget.max(1)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Continuation {
    pub text: String,
    pub segments: u8,
    pub output: u64,
}

impl Continuation {
    pub fn pending(&self) -> bool {
        self.segments > 0 || !self.text.is_empty()
    }

    pub fn messages(&self, space: usize) -> Vec<Message> {
        if !self.pending() {
            return vec![];
        }
        if self.text.is_empty() {
            return vec![Message::new(
                "user",
                "The preceding generation reached its token limit before producing a visible answer. Give the requested answer directly and concisely.",
            )];
        }
        // Keep the continuation inside the same history budget as an ordinary prompt.
        let tail_bytes = space.saturating_sub(320).clamp(128, CONTEXT_TAIL);
        let mut start = self.text.len().saturating_sub(tail_bytes);
        while !self.text.is_char_boundary(start) {
            start += 1;
        }
        vec![
            Message::new("assistant", &self.text[start..]),
            Message::new(
                "user",
                "The previous reply was cut off. Continue from the exact last character of the excerpt, including any unfinished code or Markdown fence. Return only the missing remainder. Do not repeat the excerpt or open a new code block.",
            ),
        ]
    }

    pub fn accept(&mut self, chunk: String, limited: bool, tokens: u64) -> Result<Option<String>> {
        if !self.pending() && !limited {
            return Ok(Some(chunk));
        }
        // Some models echo the last line(s). Strip a substantive exact overlap,
        // keeping short repeated syntax (for example adjacent HTML closing tags).
        let mut skip = 0;
        if self.text.len() >= 32 && chunk.starts_with(&self.text) {
            skip = self.text.len();
        } else {
            let bound = self.text.len().min(chunk.len()).min(CONTEXT_TAIL);
            for n in (32..=bound).rev() {
                if self.text.as_bytes().ends_with(&chunk.as_bytes()[..n])
                    && chunk.is_char_boundary(n)
                {
                    skip = n;
                    break;
                }
            }
        }
        let new = &chunk[skip..];
        ensure!(
            self.text.len() + new.len() <= MAX_REPLY_BYTES,
            "Reply exceeded the 512 KiB conversation limit. The last saved portion is available in this thread"
        );
        self.text.push_str(new);
        if !limited {
            return Ok(Some(std::mem::take(&mut self.text)));
        }
        self.output = self.output.saturating_add(tokens);
        self.segments = self.segments.saturating_add(1);
        ensure!(
            self.segments < AUTO_SEGMENTS && (!new.is_empty() || self.segments == 1),
            "The model repeatedly reached its output limit. Progress is saved. Resume to continue from the checkpoint"
        );
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shared_context_is_not_an_output_limit() {
        let ollama = Limits::from_metadata(
            &json!({"model_info":{"general.architecture":"inkling2","inkling2.context_length":1048576}}),
            false,
        );
        assert_eq!(ollama.output, None);
        assert_eq!(ollama.context, Some(1048576));
        assert_eq!(ollama.allowance(None, &[]), 16_384);
        let claude =
            Limits::from_metadata(&json!({"max_tokens":4096,"max_input_tokens":200000}), true);
        assert_eq!(claude.allowance(None, &[]), 4096);
        let local = ollama.allowance(Some(4096), &[Message::new("user", "x".repeat(600))]);
        assert!(local > 3000 && local < 4096);
    }

    #[test]
    fn repeated_empty_outputs_stop_and_overlapping_code_is_joined() {
        let mut empty = Continuation::default();
        assert!(empty.accept(String::new(), true, 1536).unwrap().is_none());
        assert!(!empty.messages(2048).is_empty());
        assert!(empty.accept(String::new(), true, 1536).is_err());

        let mut draft = Continuation::default();
        let line = "function restartGame() { score = 0; resetBoard(); }";
        let first = format!("<script>\n{line}");
        draft.accept(first.clone(), true, 100).unwrap();
        let last = draft
            .accept(format!("{line}\n</script>"), false, 100)
            .unwrap()
            .unwrap();
        assert_eq!(last, format!("{first}\n</script>"));
    }
}

/// What a reply Spindle cut off for repeating itself starts with, followed by the line.
pub const LOOPED: &str = "<!-- spindle-looped -->";

/// The line a reply keeps repeating, if its last lines are all the same line (at least 12
/// times in a row, ignoring blank lines and indentation).
pub fn looping(text: &str) -> Option<String> {
    let lines: Vec<&str> = text
        .lines()
        .rev()
        .skip(1) // the last line may still be arriving
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(12)
        .collect();
    (lines.len() == 12 && lines[0].len() >= 3 && lines.iter().all(|l| *l == lines[0]))
        .then(|| lines[0].chars().take(160).collect())
}

#[cfg(test)]
mod looping_tests {
    #[test]
    fn a_line_repeated_over_and_over_is_a_loop() {
        let stuck = format!("<html>\n{}", "  el.classList.remove('x');\n".repeat(13));
        assert_eq!(
            super::looping(&stuck).as_deref(),
            Some("el.classList.remove('x');")
        );
        let normal = "a\nb\n}\n}\n}\n}\nc\n";
        assert!(super::looping(normal).is_none());
        assert!(
            super::looping(&"}\n".repeat(20)).is_none(),
            "short closing lines aren't loops"
        );
    }
}
