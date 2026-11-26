//! `spindle mcp`: an MCP server on stdio, so Claude Code, Codex, VS Code and other agents can
//! give Spindle work and read the results. Each tool is a control-channel request (control.rs)
//! to the Spindle that owns the workspace. There is no approve tool: risky actions wait for a
//! person in the Spindle window or at a terminal.

use anyhow::Result;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;

fn tools() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    let n = |d: &str| json!({"type": "integer", "description": d});
    let b = |d: &str| json!({"type": "boolean", "description": d});
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({"name": name, "description": description,
               "inputSchema": {"type": "object", "properties": properties, "required": required}})
    };
    json!([
        tool(
            "spindle_ask",
            "Start a new Spindle chat with a task. Spindle works on it with local models and its own tools (files, sandboxed Python, browser checks). Returns the chat id, its state, the steps taken, the answer and files made. If it isn't finished within wait_seconds, call spindle_status later.",
            json!({"prompt": s("The task"), "project": s("Optional Spindle project folder to work in"), "model": s("Optional model, e.g. qwen3.5:9b"), "goal": b("Keep working until the task is done and checked"), "wait_seconds": n("How long to wait for it to finish (default 120, max 600)")}),
            &["prompt"]
        ),
        tool(
            "spindle_send",
            "Send a follow-up message to an existing Spindle chat.",
            json!({"chat": n("Chat id"), "message": s("The message"), "goal": b("Treat it as a goal to keep working on"), "wait_seconds": n("How long to wait (default 120)")}),
            &["chat", "message"]
        ),
        tool(
            "spindle_status",
            "A chat's state (running, queued, waiting_for_approval, idle), steps, answer and files; or, with no chat, what Spindle is doing overall.",
            json!({"chat": n("Chat id (optional)"), "wait_seconds": n("Wait up to this long for the chat to finish")}),
            &[]
        ),
        tool(
            "spindle_read_chat",
            "A chat's transcript, with tool steps as one line each (full shows tool results too).",
            json!({"chat": n("Chat id"), "full": b("Include tool results")}),
            &["chat"]
        ),
        tool(
            "spindle_list_chats",
            "Recent Spindle chats with their state.",
            json!({"project": s("Only chats in this project folder"), "limit": n("How many (default 30)")}),
            &[]
        ),
        tool(
            "spindle_list_projects",
            "Spindle projects: name, folder and description.",
            json!({}),
            &[]
        ),
        tool(
            "spindle_list_models",
            "Models Spindle can use, with size and whether they see images.",
            json!({}),
            &[]
        ),
        tool(
            "spindle_get_file",
            "Read a file a chat made (text, or base64 for binary files such as .docx).",
            json!({"chat": n("Chat id"), "name": s("File name, as listed in the chat's files")}),
            &["chat", "name"]
        ),
    ])
}

/// What a chat's status reads like for an agent.
fn describe(s: &Value) -> String {
    let mut out = format!(
        "Chat {} · {} · {}\n",
        s["chat"],
        s["title"].as_str().unwrap_or_default(),
        s["state"].as_str().unwrap_or_default()
    );
    for step in s["steps"].as_array().into_iter().flatten() {
        out.push_str(&format!("- {}\n", step.as_str().unwrap_or_default()));
    }
    for w in s["waiting_for_approval"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "Waiting for the user's approval (#{}): {} ({}). The user approves in the Spindle window or with `spindle approve {}` at a terminal.\n",
            w["id"], w["what"].as_str().unwrap_or_default(), w["risk"].as_str().unwrap_or_default(), w["id"]
        ));
    }
    if s["state"] == "idle" {
        let answer = s["answer"].as_str().unwrap_or_default().trim();
        if !answer.is_empty() {
            out.push_str(&format!("\nAnswer:\n{answer}\n"));
        }
    } else if s["state"] != "waiting_for_approval" {
        out.push_str("Not finished yet: call spindle_status with this chat id and wait_seconds.\n");
    }
    let files: Vec<&str> = s["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !files.is_empty() {
        out.push_str(&format!(
            "\nFiles: {} (folder {})\n",
            files.join(", "),
            s["folder"].as_str().unwrap_or_default()
        ));
    }
    out
}

fn call(root: &PathBuf, name: &str, a: &Value) -> Result<String> {
    let wait = a["wait_seconds"].as_u64().unwrap_or(120).min(600);
    let req = match name {
        "spindle_ask" => {
            json!({"op": "ask", "text": a["prompt"], "project": a.get("project"), "model": a.get("model"), "goal": a["goal"].as_bool().unwrap_or(false), "wait": wait})
        }
        "spindle_send" => {
            json!({"op": "send", "chat": a["chat"], "text": a["message"], "goal": a["goal"].as_bool().unwrap_or(false), "wait": wait})
        }
        "spindle_status" => {
            json!({"op": "status", "chat": a.get("chat"), "wait": a["wait_seconds"].as_u64().unwrap_or(0).min(600)})
        }
        "spindle_read_chat" => {
            json!({"op": "show", "chat": a["chat"], "full": a["full"].as_bool().unwrap_or(false)})
        }
        "spindle_list_chats" => {
            json!({"op": "chats", "project": a.get("project"), "limit": a.get("limit")})
        }
        "spindle_list_projects" => json!({"op": "projects"}),
        "spindle_list_models" => json!({"op": "models"}),
        "spindle_get_file" => json!({"op": "file", "chat": a["chat"], "name": a["name"]}),
        other => anyhow::bail!("Unknown tool {other}"),
    };
    let reply = crate::control::request(root, req)?;
    Ok(match name {
        "spindle_ask" | "spindle_send" => describe(&reply),
        "spindle_status" if reply.get("chat").is_some() => describe(&reply),
        "spindle_read_chat" => reply["text"].as_str().unwrap_or_default().to_owned(),
        "spindle_get_file" => match reply["text"].as_str() {
            Some(t) => t.to_owned(),
            None => format!(
                "(binary file, base64)\n{}",
                reply["base64"].as_str().unwrap_or_default()
            ),
        },
        _ => serde_json::to_string_pretty(&reply)?,
    })
}

pub fn main(args: &[String]) -> Result<()> {
    let dir = args
        .iter()
        .position(|a| a == "--dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SPINDLE_WORKSPACE").map(PathBuf::from));
    let root = match dir {
        Some(d) => d,
        None => std::env::current_dir()?,
    }
    .canonicalize()?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            writeln!(
                stdout,
                "{}",
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}})
            )?;
            stdout.flush()?;
            continue;
        };
        let Some(id) = msg.get("id").cloned() else {
            continue; // a notification
        };
        let method = msg["method"].as_str().unwrap_or_default();
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18"),
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "spindle", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Spindle runs tasks with local models in its own sandbox. Give it work with spindle_ask, check on it with spindle_status, read results with spindle_read_chat and spindle_get_file. Risky actions wait for the user's approval in Spindle.",
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or_default();
                let args = msg["params"]["arguments"].clone();
                Ok(match call(&root, name, &args) {
                    Ok(text) => json!({"content": [{"type": "text", "text": text}]}),
                    Err(e) => {
                        json!({"content": [{"type": "text", "text": format!("Error: {e:#}")}], "isError": true})
                    }
                })
            }
            _ => Err(json!({"code": -32601, "message": format!("Unknown method {method}")})),
        };
        let reply = match result {
            Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
            Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
        };
        writeln!(stdout, "{reply}")?;
        stdout.flush()?;
    }
    Ok(())
}
