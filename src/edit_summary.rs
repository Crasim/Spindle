//! Recorded changes, kept with the conversation so later file edits cannot change the diff.

use serde::{Deserialize, Serialize};

const MAX_TEXT: usize = 512 * 1024;
const MAX_PREVIEW: usize = 8_000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Edit {
    pub path: String,
    /// Added and removed lines, when both sides could be compared as text.
    pub lines: Option<(usize, usize)>,
    pub patch: Option<String>,
}

impl Edit {
    pub(crate) fn unknown(path: &str) -> Self {
        Self { path: path.to_owned(), lines: None, patch: None }
    }

    pub(crate) fn between(path: &str, before: &[u8], after: &[u8]) -> Self {
        let mut edit = Self::unknown(path);
        let text = |bytes: &[u8]| bytes.len() <= MAX_TEXT && !bytes.contains(&0);
        if text(before) && text(after) && let (Ok(before), Ok(after)) = (std::str::from_utf8(before), std::str::from_utf8(after)) {
            let mut patch = crate::diff::patch(path, before, after);
            let mut counts = (0, 0);
            for line in patch.lines().skip(2) {
                counts.0 += usize::from(line.starts_with('+'));
                counts.1 += usize::from(line.starts_with('-'));
            }
            edit.lines = Some(counts);
            if patch.len() > MAX_PREVIEW {
                patch.truncate(patch.floor_char_boundary(MAX_PREVIEW));
                patch.push_str("\n… Diff preview shortened.\n");
            }
            edit.patch = Some(patch);
        }
        edit
    }
}
