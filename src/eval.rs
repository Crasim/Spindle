//! Headless test runs: `spindle --eval --model M [--ollama URL] [--dir DIR] [--minutes N]
//! [--goal] [--allow-setup] [--seed DIR] PROMPT`. Setup and package installs
//! are denied unless --allow-setup. Runs one chat end to end with no window, approving every request once
//! (it's meant for a throwaway folder), then writes a report of every reply, tool call and
//! result. Used to see how a model gets on with Spindle without anyone at the keyboard.

use crate::engine::{Engine, View};
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn main(args: &[String]) -> Result<()> {
    let mut model = None;
    let mut provider_url = None;
    let mut thinking = crate::state::Thinking::Default;
    let mut dir = None;
    let mut minutes = 15u64;
    let mut goal = false;
    let mut allow_setup = false;
    let mut seed: Option<PathBuf> = None;
    let mut prompt = vec![];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--model" => model = it.next().cloned(),
            "--provider-url" => provider_url = Some(it.next().context("--provider-url needs a chat completions base URL")?.clone()),
            "--thinking" => {
                let value = it.next().context("--thinking needs default, off, low, medium or high")?;
                thinking = crate::state::Thinking::ALL.into_iter().find(|t| format!("{t:?}").eq_ignore_ascii_case(value))
                    .context("--thinking needs default, off, low, medium or high")?;
            }
            "--ollama" => {
                // Read by the Ollama client the first time it connects.
                unsafe { std::env::set_var("SPINDLE_OLLAMA", it.next().context("--ollama URL")?) };
            }
            "--dir" => dir = it.next().map(PathBuf::from),
            "--minutes" => minutes = it.next().context("--minutes N")?.parse()?,
            "--goal" => goal = true,
            "--allow-setup" => allow_setup = true,
            "--seed" => seed = it.next().map(PathBuf::from),
            other => prompt.push(other.to_owned()),
        }
    }
    let model = model.context("Give --model, for example --model lfm3-omni:latest")?;
    let prompt = prompt.join(" ");
    if prompt.trim().is_empty() {
        bail!("Give the prompt after the options");
    }
    let dir = match dir {
        Some(d) => d,
        None => crate::runtime::data_dir()
            .join("evals")
            .join(chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()),
    };
    std::fs::create_dir_all(&dir)?;
    let dir = dir.canonicalize()?;
    if std::env::var_os("SPINDLE_TRACE").is_none() {
        unsafe { std::env::set_var("SPINDLE_TRACE", dir.join("trace")) };
    }
    let runtime = tokio::runtime::Runtime::new()?;
    let api = crate::ollama::Ollama::new()?;
    let mut profiles = Vec::new();
    let model = if let Some(base) = provider_url {
        let connection = crate::providers::Connection { name: "Evaluation".into(), base, ..Default::default() };
        let profile = runtime.block_on(crate::providers::Provider::connect(connection, "eval".into(), String::new(), &api.0))?;
        anyhow::ensure!(profile.models.contains(&model), "The evaluation provider does not list {model}");
        profiles.push(profile);
        format!("@eval/{model}")
    } else { model };
    let mut state = crate::state::State {
        model: model.clone(),
        providers: profiles,
        ..Default::default()
    };
    state.thread.cowork = true;
    state.tuning.entry(model.clone()).or_default().thinking = thinking;
    // Running code works when Spindle's Python is here and the sandbox check passes.
    if let Some(rt) = crate::runtime::installed() {
        let report = crate::sandbox::probe(&rt, &crate::runtime::data_dir().join("probe"));
        println!(
            "Sandbox: {}",
            if report.ready { "ready" } else { "not ready" }
        );
        state.sandbox = Some(report);
    } else {
        println!("Sandbox: Spindle's Python isn't installed here (spindle --setup-python)");
    }
    // Image generation for evals: SPINDLE_COMFYUI=http://host:8188.
    if let Ok(url) = std::env::var("SPINDLE_COMFYUI") {
        state.preferences.comfyui = url;
    }
    if let Ok(path) = std::env::var("SPINDLE_COMFY_WORKFLOW") {
        state.preferences.comfy_workflow = path;
    }
    // Starting files for the task go into the chat's own folder, where its tools work.
    if let Some(seed) = &seed {
        let folder =
            crate::artifacts::folder(&state.files_root(&dir, state.chat_id), state.chat_id)?;
        for entry in
            std::fs::read_dir(seed).with_context(|| format!("--seed {}", seed.display()))?
        {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                std::fs::copy(entry.path(), folder.join(entry.file_name()))?;
            }
        }
    }
    let engine = Engine {
        root: dir.clone(),
        state,
        models: vec![],
        store: None,
        api,
        jobs: Default::default(),
    };
    let (tx, input) = tokio::sync::mpsc::unbounded_channel();
    let (output, views) = std::sync::mpsc::channel::<View>();
    let runner = std::thread::spawn(move || runtime.block_on(engine.run(input, output, false)));
    let send = |s: String| {
        let _ = tx.send(s);
    };
    send(format!("/model {model}"));
    send(if goal {
        format!("/goal {prompt}")
    } else {
        prompt.clone()
    });
    println!("Running {model} in {} · up to {minutes} min", dir.display());
    let deadline = Instant::now() + Duration::from_secs(minutes * 60);
    let mut last: Option<View> = None;
    let mut approved = BTreeSet::new();
    let mut idle_since: Option<Instant> = None;
    let mut seen_work = false;
    let mut notes = 0usize;
    let mut timed_out = false;
    loop {
        if Instant::now() > deadline {
            timed_out = true;
            break;
        }
        match views.recv_timeout(Duration::from_millis(500)) {
            Ok(v) => {
                for r in v.state.requests.iter().filter(|r| !r.approved) {
                    if approved.insert(r.id) {
                        // Downloads (Python, packages) only with --allow-setup.
                        let setup = matches!(
                            r.call.call,
                            crate::tools::Call::Setup {} | crate::tools::Call::Install { .. } | crate::tools::Call::LoadTool { .. }
                        );
                        if setup && !allow_setup {
                            println!("  deny · {}", r.call.call.summary());
                            send(format!("/deny {}", r.id));
                        } else {
                            println!("  approve · {}", r.call.call.summary());
                            send(format!("/approve {} once", r.id));
                        }
                    }
                }
                let now_notes = v.state.thread.turns.len();
                if now_notes > notes {
                    for (_, t) in v.state.thread.turns.iter().skip(notes) {
                        if let Some(tool) = &t.tool {
                            println!("  step · {} · {}", tool.summary, tool.status);
                        }
                    }
                    notes = now_notes;
                }
                let working = v.busy
                    || v.job.is_some()
                    || !v.state.queue.is_empty()
                    || v.state.requests.iter().any(|r| !r.approved);
                seen_work |= working || !v.state.history.is_empty();
                idle_since = if working {
                    None
                } else {
                    idle_since.or(Some(Instant::now()))
                };
                last = Some(v);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
        if seen_work && idle_since.is_some_and(|t| t.elapsed() > Duration::from_secs(4)) {
            break;
        }
    }
    send("/exit".into());
    // Take the last word from the engine as it shuts down, so the final reply is included.
    while let Ok(v) = views.recv_timeout(Duration::from_secs(5)) {
        last = Some(v);
    }
    let view = last.context("The engine never reported back")?;
    let report = report(&view, &dir, timed_out);
    let file = dir.join("eval-report.md");
    std::fs::write(&file, &report)?;
    println!("{report}");
    println!("Report: {}", file.display());
    drop(runner);
    Ok(())
}

fn report(view: &View, dir: &std::path::Path, timed_out: bool) -> String {
    let s = &view.state;
    let mut out = String::new();
    let _ = writeln!(out, "# Eval · {}\n", s.model);
    if timed_out {
        let _ = writeln!(out, "**Stopped at the time limit.**\n");
    }
    for (i, m) in s.history.iter().enumerate() {
        if m.role == "user" && s.thread.tool_results.contains(&i) {
            let body: String = m.content.chars().take(2500).collect();
            let _ = writeln!(out, "## [{i}] Result\n```\n{body}\n```\n");
            continue;
        }
        let who = if m.role == "user" { "User" } else { "Model" };
        let _ = writeln!(out, "## [{i}] {who}\n{}\n", m.content.trim());
        if let Some(t) = s.thread.turns.get(&i) {
            if let Some(call) = &t.call {
                let call: String = call.chars().take(1500).collect();
                let _ = writeln!(out, "Call: `{call}`\n");
            }
            if let Some(tool) = &t.tool {
                let _ = writeln!(out, "Step: {} · {}\n", tool.summary, tool.status);
            }
            if let Some(a) = &t.applied {
                let _ = writeln!(out, "Applied: {}\n", a.summary);
            }
            let _ = writeln!(
                out,
                "_{} output tokens{}_\n",
                t.output,
                t.tokens_per_second
                    .map(|r| format!(" · {r:.0} tok/s"))
                    .unwrap_or_default()
            );
        }
    }
    let folder = crate::artifacts::folder(&s.files_root(dir, s.chat_id), s.chat_id);
    if let Ok(folder) = folder {
        let _ = writeln!(out, "## Files in the chat folder ({})", folder.display());
        if let Ok(read) = std::fs::read_dir(&folder) {
            for e in read.flatten() {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                let _ = writeln!(out, "- {} · {size} bytes", e.file_name().to_string_lossy());
                if size <= 4000
                    && let Ok(text) = std::fs::read_to_string(e.path())
                {
                    let _ = writeln!(out, "```\n{}\n```", text.trim_end());
                }
            }
        }
    }
    out
}
