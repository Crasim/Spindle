//! `spindle <command>`: use the Spindle that owns a workspace from a terminal or a script. Each
//! command is one request over the control channel (control.rs); a windowless Spindle starts
//! if none is running. `--json` prints the raw reply for scripts.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::IsTerminal;
use std::path::PathBuf;

pub const USAGE: &str = "spindle — local AI workspace

Use the Spindle that owns this folder's workspace (a windowless one starts if needed):
  spindle ask \"prompt\" [--project DIR] [--model M] [--goal] [--no-wait]
  spindle send CHAT \"message\" [--goal] [--no-wait]
  spindle status [CHAT] [--wait SECONDS]
  spindle chats [--project DIR] [--limit N]
  spindle show CHAT [--full]
  spindle file CHAT NAME [--save PATH]
  spindle approve ID [once|conversation|project]   (at a terminal only)
  spindle deny ID
  spindle projects
  spindle models
  spindle sync [now | status | devices | create NAME | folder PATH | server URL [ADMIN]
               | account USER PASSWORD | join-account URL USER DEVICE PASSWORD
               | pair | join LINK-OR-CODE | remove DEVICE | switch NAME on|off
               | project DIR conversations=on|off|global files=… hidden=on|off]
  spindle sync-server --data DIR [--listen ADDR] [--mode store|relay|both]
                      [--mailbox-hours N] [--admin-token T]   (hosted sync server)
  spindle mcp                                        (MCP server on stdio)
  spindle serve                                      (windowless owner; stops when idle)
Options: --dir DIR (workspace; default the current folder), --json (raw output), --wait N";

struct Args {
    rest: Vec<String>,
    dir: Option<PathBuf>,
    json: bool,
    flags: Vec<String>,
    values: Vec<(String, String)>,
}

fn parse(args: &[String]) -> Args {
    let takes = [
        "--dir",
        "--project",
        "--model",
        "--limit",
        "--wait",
        "--save",
    ];
    let mut a = Args {
        rest: vec![],
        dir: None,
        json: false,
        flags: vec![],
        values: vec![],
    };
    let mut it = args.iter();
    while let Some(x) = it.next() {
        if x == "--json" {
            a.json = true;
        } else if takes.contains(&x.as_str()) {
            if let Some(v) = it.next() {
                if x == "--dir" {
                    a.dir = Some(PathBuf::from(v));
                } else {
                    a.values.push((x.clone(), v.clone()));
                }
            }
        } else if x.starts_with("--") {
            a.flags.push(x.clone());
        } else {
            a.rest.push(x.clone());
        }
    }
    a
}

impl Args {
    fn value(&self, k: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.as_str())
    }
    fn flag(&self, k: &str) -> bool {
        self.flags.iter().any(|f| f == k)
    }
}

/// A project folder as Spindle records it (absolute).
fn project(p: &str) -> Result<String> {
    Ok(PathBuf::from(p)
        .canonicalize()
        .with_context(|| format!("No folder {p}"))?
        .to_string_lossy()
        .into_owned())
}

pub fn main(args: &[String]) -> Result<()> {
    let a = parse(args);
    let root = match &a.dir {
        Some(d) => d.clone(),
        None => std::env::current_dir()?,
    }
    .canonicalize()?;
    let cmd = a.rest.first().map(String::as_str).unwrap_or_default();
    let arg = |i: usize, what: &str| -> Result<String> {
        a.rest
            .get(i)
            .cloned()
            .with_context(|| format!("Give {what}. {USAGE}"))
    };
    let wait_for = |default: u64| -> u64 {
        if a.flag("--no-wait") {
            0
        } else {
            a.value("--wait")
                .and_then(|w| w.parse().ok())
                .unwrap_or(default)
        }
    };
    let req = match cmd {
        "ask" => json!({
            "op": "ask",
            "text": a.rest[1..].join(" "),
            "project": a.value("--project").map(project).transpose()?,
            "model": a.value("--model"),
            "goal": a.flag("--goal"),
            "wait": wait_for(600),
        }),
        "send" => json!({
            "op": "send",
            "chat": arg(1, "the chat id")?.parse::<u64>()?,
            "text": a.rest[2..].join(" "),
            "goal": a.flag("--goal"),
            "wait": wait_for(600),
        }),
        "status" => json!({
            "op": "status",
            "chat": a.rest.get(1).map(|c| c.parse::<u64>()).transpose()?,
            "wait": wait_for(0),
        }),
        "chats" => json!({
            "op": "chats",
            "project": a.value("--project").map(project).transpose()?,
            "limit": a.value("--limit").and_then(|l| l.parse::<u64>().ok()),
        }),
        "show" => {
            json!({"op": "show", "chat": arg(1, "the chat id")?.parse::<u64>()?, "full": a.flag("--full")})
        }
        "file" => {
            json!({"op": "file", "chat": arg(1, "the chat id")?.parse::<u64>()?, "name": arg(2, "the file name")?})
        }
        "approve" | "deny" => {
            // Someone at a keyboard decides, never a script or an agent's shell.
            if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
                bail!(
                    "Approving or denying needs a person at a terminal (or use the Spindle window)."
                );
            }
            json!({"op": cmd, "id": arg(1, "the request id")?.parse::<u64>()?, "level": a.rest.get(2)})
        }
        "projects" => json!({"op": "projects"}),
        "models" => json!({"op": "models"}),
        "sync" => {
            json!({"op": "sync", "line": format!("/sync {}", a.rest[1..].join(" ")).trim_end()})
        }
        _ => bail!("{USAGE}"),
    };
    let reply = crate::control::request(&root, req)?;
    if a.json {
        println!("{reply}");
        return Ok(());
    }
    match cmd {
        "sync" => println!("{}", reply["text"].as_str().unwrap_or("")),
        "ask" | "send" | "status" if reply.get("chat").is_some() => print_status(&reply),
        "status" => {
            println!(
                "{} · {} queued{}",
                if reply["busy"].as_bool() == Some(true) {
                    "Working"
                } else {
                    "Idle"
                },
                reply["queued"],
                reply["job"]
                    .as_str()
                    .map(|j| format!(" · {j}"))
                    .unwrap_or_default()
            );
            for w in reply["waiting_for_approval"]
                .as_array()
                .into_iter()
                .flatten()
            {
                println!(
                    "  waiting for approval #{} (chat {}): {}",
                    w["id"],
                    w["chat"],
                    w["what"].as_str().unwrap_or_default()
                );
            }
        }
        "chats" => {
            for c in reply.as_array().into_iter().flatten() {
                println!(
                    "{:>6}  {:<10} {}  ({})",
                    c["chat"],
                    c["state"].as_str().unwrap_or_default(),
                    c["title"].as_str().unwrap_or_default(),
                    c["model"].as_str().unwrap_or_default()
                );
            }
        }
        "show" => println!("{}", reply["text"].as_str().unwrap_or_default()),
        "file" => {
            let bytes = match (reply["text"].as_str(), reply["base64"].as_str()) {
                (Some(t), _) => t.as_bytes().to_vec(),
                (_, Some(b)) => {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.decode(b)?
                }
                _ => vec![],
            };
            match a.value("--save") {
                Some(p) => {
                    std::fs::write(p, &bytes)?;
                    println!("Saved {p} ({} bytes)", bytes.len());
                }
                None => match String::from_utf8(bytes) {
                    Ok(t) => print!("{t}"),
                    Err(_) => bail!("A binary file: use --save PATH"),
                },
            }
        }
        "projects" => {
            for p in reply.as_array().into_iter().flatten() {
                println!(
                    "{}  {}",
                    p["name"].as_str().unwrap_or_default(),
                    p["path"].as_str().unwrap_or_default()
                );
            }
        }
        "models" => {
            for m in reply.as_array().into_iter().flatten() {
                println!(
                    "{}{}{}",
                    if m["current"].as_bool() == Some(true) {
                        "* "
                    } else {
                        "  "
                    },
                    m["model"].as_str().unwrap_or_default(),
                    m["size_b"]
                        .as_f64()
                        .map(|b| format!("  ({b:.1}B)"))
                        .unwrap_or_default()
                );
            }
        }
        _ => println!("Done"),
    }
    Ok(())
}

fn print_status(s: &Value) {
    let state = s["state"].as_str().unwrap_or_default();
    println!(
        "Chat {} · {} · {}",
        s["chat"],
        s["title"].as_str().unwrap_or_default(),
        state
    );
    for step in s["steps"].as_array().into_iter().flatten() {
        println!("  · {}", step.as_str().unwrap_or_default());
    }
    for w in s["waiting_for_approval"].as_array().into_iter().flatten() {
        println!(
            "  waiting for your approval #{}: {} ({}) · spindle approve {} / deny {}",
            w["id"],
            w["what"].as_str().unwrap_or_default(),
            w["risk"].as_str().unwrap_or_default(),
            w["id"],
            w["id"]
        );
    }
    if state == "idle" {
        let answer = s["answer"].as_str().unwrap_or_default();
        if !answer.trim().is_empty() {
            println!("\n{}", answer.trim());
        }
        let files: Vec<&str> = s["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if !files.is_empty() {
            println!(
                "\nFiles: {} (in {})",
                files.join(", "),
                s["folder"].as_str().unwrap_or_default()
            );
        }
    } else {
        println!("Check again with: spindle status {} --wait 120", s["chat"]);
    }
}
