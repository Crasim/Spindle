use super::*;

#[derive(Default)]
pub(super) struct ChatSearch {
    query: String,
    focus: bool,
    results: Vec<SearchResult>,
}

struct SearchResult {
    id: u64,
    title: String,
    location: String,
    excerpt: Option<String>,
}

impl ChatSearch {
    pub(super) fn activate(&mut self) {
        self.focus = true;
    }
}

fn matches(state: &crate::state::State, query: &str) -> Vec<SearchResult> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return vec![];
    }
    std::iter::once(state.chat_id)
        .chain(state.chats.keys().rev().copied())
        .filter_map(|id| {
            let (thread, history) = state.thread_data(id)?;
            if history.is_empty() && thread.title.is_empty() {
                return None;
            }
            let title = thread.label(history);
            let title_match = title.to_lowercase().contains(&query);
            let excerpt = if title_match {
                None
            } else {
                history
                    .iter()
                    .enumerate()
                    .rev()
                    .filter(|(i, message)| {
                        matches!(message.role.as_str(), "user" | "assistant")
                            && !thread.tool_results.contains(i)
                    })
                    .find_map(|(_, message)| {
                        let start = message.content.to_lowercase().find(&query)?;
                        let mut from = start.saturating_sub(45).min(message.content.len());
                        while !message.content.is_char_boundary(from) {
                            from -= 1;
                        }
                        let snippet = message.content[from..]
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        let snippet = snippet.chars().take(155).collect::<String>();
                        Some(if from > 0 {
                            format!("…{snippet}")
                        } else {
                            snippet
                        })
                    })
            };
            if !title_match && excerpt.is_none() {
                return None;
            }
            let project = thread.project.as_deref().and_then(Path::file_name);
            let mut location = project.map_or_else(
                || "Chats".into(),
                |name| name.to_string_lossy().into_owned(),
            );
            if thread.archived {
                location.push_str(" · Archived");
            }
            Some(SearchResult {
                id,
                title: title.to_owned(),
                location,
                excerpt,
            })
        })
        .take(100)
        .collect()
}

impl Desktop {
    pub(super) fn search_chats(&mut self, ui: &mut egui::Ui) {
        let search = &mut self.chat_search;
        let width = ui.available_width().min(660.);
        let response = self.android_ime.field(
            ui,
            "chat-search",
            &mut search.query,
            Default::default(),
            |field| {
                field
                    .hint_text("Search chat titles and messages")
                    .desired_width(width)
                    .margin(egui::Margin::symmetric(12, 8))
            },
        );
        if std::mem::take(&mut search.focus) {
            response.request_focus();
            search.results = matches(&self.view.state, &search.query);
        }
        if response.changed() {
            search.results = matches(&self.view.state, &search.query);
        }
        ui.add_space(10.);
        if search.results.is_empty() {
            ui.weak(if search.query.trim().is_empty() {
                "Search this device’s project chats and archive."
            } else {
                "No matching chats."
            });
        }
        let chosen = egui::ScrollArea::vertical()
            .show(ui, |ui| {
                let mut chosen = None;
                for result in &search.results {
                    ui.push_id(result.id, |ui| {
                        egui::Frame::new()
                            .fill(raised())
                            .corner_radius(8)
                            .inner_margin(12)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width().min(635.));
                                if ui
                                    .add(
                                        egui::Button::new(Text::new(&result.title).strong())
                                            .wrap()
                                            .frame(false),
                                    )
                                    .clicked()
                                {
                                    chosen = Some(result.id);
                                }
                                ui.weak(&result.location);
                                if let Some(excerpt) = &result.excerpt {
                                    ui.label(excerpt);
                                }
                            });
                    });
                }
                chosen
            })
            .inner;
        if let Some(id) = chosen {
            self.send(format!("/open {id}"));
            self.page = Page::Chat;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_includes_archived_chat_text_and_skips_tool_results() {
        let mut state = crate::state::State::default();
        state.thread.title = "Release work".into();
        state.thread.archived = true;
        state.history.push(crate::ollama::Message::new(
            "user",
            "Remember the garden checklist",
        ));
        state
            .history
            .push(crate::ollama::Message::new("user", "hidden-tool-output"));
        state.thread.tool_results.insert(1);
        assert_eq!(matches(&state, "RELEASE")[0].id, state.chat_id);
        let results = matches(&state, "garden checklist");
        assert_eq!(results[0].location, "Chats · Archived");
        assert!(
            results[0]
                .excerpt
                .as_ref()
                .unwrap()
                .contains("garden checklist")
        );
        assert!(matches(&state, "hidden-tool-output").is_empty());
    }
}
