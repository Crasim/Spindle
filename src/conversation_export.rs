use crate::{
    diagnostics::{Failure, SyncEvent},
    ollama::Message,
    state::{ActionRecord, Pending, State, Task, ThreadInfo},
    sync::adapter::BranchInfo,
};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Serialize)]
struct Archive<'a> {
    format: &'static str,
    app_version: &'static str,
    exported_at_utc: String,
    selected_chat: u64,
    chats: Vec<Chat<'a>>,
    tool_actions: Vec<&'a ActionRecord>,
    pending_tools: Vec<&'a Pending>,
    queued_tasks: Vec<&'a Task>,
    failures: Vec<&'a Failure>,
    sync_events: Vec<&'a SyncEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    streaming_reply: Option<(u64, &'a str)>,
}

#[derive(Serialize)]
struct Chat<'a> {
    id: u64,
    title: &'a str,
    messages: &'a [Message],
    thread: &'a ThreadInfo,
    goal: &'a str,
    plan: &'a Option<(u64, String)>,
    sync_branches: &'a [BranchInfo],
}

/// Export the selected chat and all saved branches related to it, without serializing the
/// workspace, provider configuration, or other conversations.
pub(crate) fn archive(state: &State, id: u64, stream: Option<(u64, &str)>) -> Result<Vec<u8>> {
    state
        .thread_data(id)
        .context("Conversation no longer exists")?;
    let families = state.prompt_families();
    let mut ids = BTreeSet::from([id]);
    loop {
        let previous = ids.len();
        let linked: Vec<_> = state
            .chats
            .keys()
            .copied()
            .chain([state.chat_id])
            .filter_map(|chat| {
                let (thread, _) = state.thread_data(chat)?;
                let family = families.get(&chat);
                let fork = thread.forked_from.map(|(parent, _)| parent);
                if ids.contains(&chat) {
                    return fork.filter(|parent| state.thread_data(*parent).is_some());
                }
                (fork.is_some_and(|parent| ids.contains(&parent))
                    || family.is_some_and(|root| {
                        ids.iter().any(|member| families.get(member) == Some(root))
                    }))
                .then_some(chat)
            })
            .collect();
        ids.extend(linked);
        if ids.len() == previous {
            break;
        }
    }
    let chats = ids
        .iter()
        .map(|&chat| {
            let (thread, messages) = state.thread_data(chat).expect("chat IDs were resolved");
            let (goal, plan) = if chat == state.chat_id {
                (state.goal.as_str(), &state.plan)
            } else {
                let c = &state.chats[&chat];
                (c.goal.as_str(), &c.plan)
            };
            let sync_branches = state
                .space
                .as_ref()
                .and_then(|space| space.branches.get(&chat))
                .map(Vec::as_slice)
                .unwrap_or_default();
            Chat {
                id: chat,
                title: thread.label(messages),
                messages,
                thread,
                goal,
                plan,
                sync_branches,
            }
        })
        .collect();
    let export = Archive {
        format: "spindle-conversation-1",
        app_version: env!("CARGO_PKG_VERSION"),
        exported_at_utc: chrono::Utc::now().to_rfc3339(),
        selected_chat: id,
        chats,
        tool_actions: state
            .actions
            .iter()
            .filter(|action| ids.contains(&action.chat))
            .collect(),
        pending_tools: state
            .requests
            .iter()
            .filter(|request| ids.contains(&request.chat))
            .collect(),
        queued_tasks: state
            .queue
            .iter()
            .filter(|task| ids.contains(&task.origin.unwrap_or(state.chat_id)))
            .collect(),
        failures: state
            .failures
            .iter()
            .filter(|failure| ids.contains(&failure.chat))
            .collect(),
        sync_events: state
            .sync_diagnostics
            .iter()
            .filter(|event| ids.contains(&event.chat))
            .collect(),
        streaming_reply: stream.filter(|(chat, content)| ids.contains(chat) && !content.is_empty()),
    };
    Ok(serde_json::to_vec_pretty(&export)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        policy::Risk,
        state::{Conversation, ModelChange, PromptVersion, Turn},
    };

    #[test]
    fn archive_includes_tool_steps_timestamps_and_all_saved_branches_for_the_selected_chat()
    -> Result<()> {
        let mut state = State {
            chat_id: 10,
            ..State::default()
        };
        state.thread.prompt_family = Some("versions".into());
        state.history = vec![
            Message::new("user", "Find the entry"),
            Message::new("assistant", "I'll read it."),
            Message::new("user", "Tool result: line 13"),
        ];
        state.thread.tool_results.insert(2);
        state.thread.turns.insert(
            1,
            Turn {
                request_at: Some(100),
                reply_at: Some(105),
                call: Some(r#"{"name":"read","args":{"path":"test.txt"}}"#.into()),
                model_selection: Some(ModelChange {
                    from: "model one".into(),
                    to: "model two".into(),
                }),
                ..Default::default()
            },
        );
        let mut edit = Conversation::default();
        edit.thread.prompt_versions.insert(
            0,
            PromptVersion {
                group: "versions".into(),
                number: 2,
            },
        );
        edit.history.push(Message::new("user", "edited prompt"));
        state.chats.insert(11, edit);
        let mut fork = Conversation::default();
        fork.thread.forked_from = Some((11, 1));
        fork.history
            .push(Message::new("assistant", "explicit branch reply"));
        state.chats.insert(12, fork);
        let mut unrelated = Conversation::default();
        unrelated
            .history
            .push(Message::new("user", "unrelated private chat"));
        state.chats.insert(13, unrelated);
        for (id, chat, ok) in [(1, 10, false), (2, 11, true), (3, 13, false)] {
            state.actions.push(ActionRecord {
                id,
                time: 106 + id as i64,
                chat,
                ok,
                model: "model two".into(),
                tool: "read".into(),
                summary: "Read test.txt".into(),
                risk: Risk::Read,
                decision: "Allowed".into(),
                outcome: if ok { "Read succeeded" } else { "Read failed" }.into(),
            });
        }
        let space = crate::sync::space::Space::create("Fixture device", None)?;
        let mut sync = crate::sync::adapter::SyncState::new(space.id);
        sync.branches.insert(
            10,
            vec![BranchInfo {
                id: "remote-branch".into(),
                messages: vec![Message::new("assistant", "synced branch reply")],
                recorded_at_ms: vec![120_000],
                ..Default::default()
            }],
        );
        state.space = Some(sync);
        let bytes = archive(&state, 10, Some((10, "reply still streaming")))?;
        let output: serde_json::Value = serde_json::from_slice(&bytes)?;
        assert_eq!(output["chats"].as_array().unwrap().len(), 3);
        assert_eq!(output["chats"][0]["thread"]["turns"]["1"]["reply_at"], 105);
        assert_eq!(
            output["chats"][0]["thread"]["turns"]["1"]["model_selection"]["to"],
            "model two"
        );
        assert_eq!(
            output["chats"][0]["sync_branches"][0]["recorded_at_ms"][0],
            120_000
        );
        assert_eq!(
            output["chats"][2]["thread"]["forked_from"],
            serde_json::json!([11, 1])
        );
        assert_eq!(output["tool_actions"].as_array().unwrap().len(), 2);
        assert_eq!(output["tool_actions"][0]["outcome"], "Read failed");
        assert_eq!(output["tool_actions"][1]["outcome"], "Read succeeded");
        let text = String::from_utf8(bytes)?;
        for value in [
            "Tool result: line 13",
            "test.txt",
            "edited prompt",
            "reply still streaming",
        ] {
            assert!(text.contains(value));
        }
        assert!(!text.contains("unrelated private chat"));
        assert!(output.get("providers").is_none());
        Ok(())
    }
}
