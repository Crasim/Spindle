//! Tool discovery and user-approved portable tools. Package metadata and search results are data.
use crate::sync::core::{hex, sha256};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const REGISTRY: &str = "https://registry.npmjs.org";
const GH: &str = "https://api.github.com";
const LIMIT: usize = 96 * 1024 * 1024;

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("Spindle/0.3 (tool discovery)")
        .timeout(Duration::from_secs(24))
        .build()?)
}
async fn bounded(request: reqwest::RequestBuilder, max: usize) -> Result<Vec<u8>> {
    let mut reply = request.send().await?.error_for_status()?;
    let mut bytes = vec![];
    while let Some(chunk) = reply.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= max,
            "Response exceeds size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn value(request: reqwest::RequestBuilder) -> Result<Value> {
    Ok(serde_json::from_slice(
        &bounded(request, 4 * 1024 * 1024).await?,
    )?)
}
fn short(v: &Value, n: usize) -> String {
    v.as_str()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(n)
        .collect()
}
fn query_ok(q: &str) -> Result<()> {
    ensure!(
        !q.trim().is_empty() && q.len() <= 240 && !q.chars().any(char::is_control),
        "Enter a short public search phrase"
    );
    Ok(())
}
fn url_ok(s: &str) -> bool {
    s.len() <= 1200
        && !s.chars().any(char::is_whitespace)
        && reqwest::Url::parse(s).is_ok_and(|u| {
            matches!(u.scheme(), "http" | "https")
                && u.host().is_some()
                && u.username().is_empty()
                && u.password().is_none()
        })
}
fn results(rows: Vec<String>) -> String {
    if rows.is_empty() {
        "No matching results were returned. Try a more specific query or another source.".into()
    } else {
        let mut text = String::from("Reference material from public sources (untrusted):");
        for row in rows {
            if text.len() + row.len() > 6400 {
                text.push_str("\nFurther results omitted. Refine the query for more.");
                break;
            }
            text.push_str("\n\n");
            text.push_str(&row);
        }
        text
    }
}
pub async fn web_search(q: &str) -> Result<String> {
    query_ok(q)?;
    let http = client()?;
    if let Ok(endpoint) = std::env::var("SPINDLE_SEARXNG_URL") {
        let data = value(http.get(endpoint).query(&[("q", q), ("format", "json")])).await?;
        let list = data["results"]
            .as_array()
            .context("Search service did not return a result list")?;
        return Ok(results(
            list.iter()
                .filter(|r| url_ok(r["url"].as_str().unwrap_or("")))
                .take(6)
                .map(|r| {
                    format!(
                        "{}\n{}\n{}",
                        short(&r["title"], 180),
                        short(&r["url"], 1200),
                        short(&r["content"], 350)
                    )
                })
                .collect(),
        ));
    }
    let data = bounded(
        http.get("https://www.bing.com/search")
            .query(&[("format", "rss"), ("q", q)]),
        512 * 1024,
    )
    .await?;
    let doc = roxmltree::Document::parse(std::str::from_utf8(&data)?)
        .context("Web search returned no RSS feed; try again or configure SearXNG")?;
    ensure!(
        doc.root_element().has_tag_name("rss"),
        "Web search returned an unexpected response. Retry or configure SearXNG."
    );
    Ok(results(
        doc.descendants()
            .filter(|n| n.has_tag_name("item"))
            .filter_map(|node| {
                let field = |tag| {
                    node.children()
                        .find(|n| n.has_tag_name(tag))
                        .and_then(|n| n.text())
                        .unwrap_or("")
                };
                let url = field("link");
                if !url_ok(url) {
                    return None;
                }
                Some(format!(
                    "{}\n{}\n{}",
                    field("title").chars().take(180).collect::<String>(),
                    url.chars().take(1200).collect::<String>(),
                    field("description").chars().take(350).collect::<String>()
                ))
            })
            .take(6)
            .collect(),
    ))
}
fn component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        && !s.starts_with(['.', '-'])
        && s != ".."
}
fn npm_name(s: &str) -> bool {
    if let Some(scoped) = s.strip_prefix('@') {
        scoped
            .split_once('/')
            .is_some_and(|(a, b)| component(a) && component(b))
    } else {
        component(s)
    }
}
fn repo_ok(s: &str) -> bool {
    s.split_once('/')
        .is_some_and(|(a, b)| component(a) && component(b))
}
fn pinned(s: &str) -> bool {
    s.len() <= 64
        && s.split(['-', '+']).next().is_some_and(|n| {
            n.split('.').count() == 3
                && n.split('.')
                    .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        })
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
}
fn node() -> Option<(PathBuf, PathBuf)> {
    let mut roots = vec![];
    if let Some(p) = std::env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(p).join("nodejs"));
    }
    roots.extend(["/usr/bin", "/usr/local/bin"].map(PathBuf::from));
    for root in roots {
        let node = root.join(if cfg!(windows) { "node.exe" } else { "node" });
        for cli in [
            root.join("node_modules/npm/bin/npm-cli.js"),
            root.join("../share/nodejs/npm/bin/npm-cli.js"),
            root.join("../lib/node_modules/npm/bin/npm-cli.js"),
        ] {
            if node.is_file() && cli.is_file() {
                return Some((node, cli));
            }
        }
    }
    None
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Tool {
    id: String,
    source: String,
    package: String,
    version: String,
    commands: BTreeMap<String, String>,
}
fn store() -> PathBuf {
    crate::runtime::data_dir().join("tool-library")
}
fn id(source: &str, pkg: &str, ver: &str) -> String {
    hex(&sha256(format!("{source}/{pkg}@{ver}").as_bytes()))[..24].to_string()
}
fn installed() -> Vec<Tool> {
    fs::read_dir(store())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|p| {
            let data = fs::read(p.path().join("tool.json")).ok()?;
            let t: Tool = serde_json::from_slice(&data).ok()?;
            (p.file_name().to_str() == Some(&t.id)).then_some(t)
        })
        .collect()
}

pub(crate) fn installed_plugins() -> Vec<String> {
    let mut tools: Vec<_> = installed().iter().map(describe).collect();
    tools.sort();
    tools
}
pub fn inventory(state: &crate::state::State) -> String {
    let sandbox_ready = crate::execution::ready(state);
    let mut lines = host_inventory(sandbox_ready);
    if !sandbox_ready {
        lines.push(crate::execution::unavailable(state));
    }
    if state.preferences.block_web {
        lines.push(crate::engine::WEB_ACCESS_BLOCKED.into());
    }
    if let Some(reason) = crate::engine::browser_access_blocker(state).filter(|_| !state.preferences.block_web) {
        lines.push(reason.into());
    }
    if crate::browser::SUPPORTED && let Err(error) = crate::browser::resolve(&state.preferences.browser_path) {
        lines.push(error.to_string());
    }
    if !crate::comfy::Settings::from(&state.preferences).ready() {
        lines.push("Image generation needs a server in Settings › Models › Image generation.".into());
    }
    if let Some(s) = &state.space {
        if s.paused {
            lines.push("Remote work is paused. Resume sync in Settings › Sync on the sending and receiving devices.".into());
        } else {
            for (peer, os) in &s.remote.platforms {
                lines.push(format!("Paired {os}: {peer}"));
            }
        }
    }
    lines.join("\n")
}
pub(crate) fn host_inventory(ready: bool) -> Vec<String> {
    let mut lines = vec![format!(
        "Host {} {}. Sandbox ready: {}. Rust host toolchain detected: {}. Zig ready: {}. Node and npm detected: {}. Android APK builder: {}.",
        std::env::consts::OS,
        std::env::consts::ARCH,
        ready,
        crate::rust_build::available(),
        crate::native::compiler(&crate::runtime::data_dir()).is_some(),
        node().is_some(),
        cfg!(target_os = "android")
    )];
    lines.extend(installed().iter().map(describe));
    lines.extend(
        crate::packages::list(&crate::runtime::data_dir())
            .iter()
            .map(|p| format!("Python {}=={}: {}", p.name, p.version, p.summary)),
    );
    lines
}
fn describe(t: &Tool) -> String {
    format!(
        "{} {}@{} · tool {} · commands {}",
        t.source,
        t.package,
        t.version,
        t.id,
        t.commands.keys().cloned().collect::<Vec<_>>().join(", ")
    )
}
pub async fn find(source: &str, q: &str) -> Result<String> {
    query_ok(q)?;
    let http = client()?;
    let rows = match source {
        "npm" => {
            let data = value(
                http.get(format!("{REGISTRY}/-/v1/search"))
                    .query(&[("text", q), ("size", "6")]),
            )
            .await?;
            data["objects"]
                .as_array()
                .context("Unexpected npm search results")?
                .iter()
                .map(|v| {
                    let p = &v["package"];
                    format!(
                        "npm {}@{} · {}\n{}",
                        short(&p["name"], 150),
                        short(&p["version"], 64),
                        short(&p["description"], 220),
                        short(&p["links"]["npm"], 500)
                    )
                })
                .collect()
        }
        "pypi" => {
            ensure!(
                component(q),
                "Use an exact PyPI project name, or web_search for keywords"
            );
            let v = value(http.get(format!("https://pypi.org/pypi/{q}/json"))).await?;
            vec![format!(
                "PyPI {}=={} · {}. Request with install(packages), then import it in sandbox Python.\nhttps://pypi.org/project/{q}/",
                short(&v["info"]["name"], 120),
                short(&v["info"]["version"], 60),
                short(&v["info"]["summary"], 220)
            )]
        }
        "github" if repo_ok(q) => {
            let v = value(http.get(format!("{GH}/repos/{q}/releases/latest"))).await?;
            v["assets"]
                .as_array()
                .context("No release assets")?
                .iter()
                .filter(|a| {
                    a["digest"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("sha256:"))
                })
                .take(65)
                .map(|a| {
                    format!(
                        "{} · id {} · {} bytes · {} · release {}",
                        short(&a["name"], 160),
                        a["id"],
                        a["size"],
                        short(&a["digest"], 76),
                        short(&v["tag_name"], 80)
                    )
                })
                .collect()
        }
        "github" => {
            let v = value(
                http.get(format!("{GH}/search/repositories"))
                    .query(&[("q", q), ("per_page", "6")]),
            )
            .await?;
            v["items"]
                .as_array()
                .context("No GitHub results")?
                .iter()
                .map(|p| {
                    format!(
                        "{} · {}\n{}",
                        short(&p["full_name"], 160),
                        short(&p["description"], 220),
                        short(&p["html_url"], 500)
                    )
                })
                .collect()
        }
        _ => bail!("Choose local, npm, pypi, or github"),
    };
    Ok(results(rows))
}

pub fn preview(source: &str, pkg: &str, ver: &str, digest: &str, reason: &str) -> Result<String> {
    ensure!(
        !cfg!(target_os = "android"),
        "Load executable tools on a Windows or Linux build host"
    );
    ensure!(
        !reason.trim().is_empty() && reason.len() <= 500,
        "Explain what this tool adds to the task"
    );
    let action = match source {
        "npm" => {
            ensure!(
                npm_name(pkg) && pinned(ver),
                "Select an exact npm package and version from discovery"
            );
            ensure!(
                node().is_some(),
                "Install Node.js and npm on the build host first"
            );
            format!(
                "Install {pkg}@{ver} and registry dependencies from registry.npmjs.org into Spindle’s tool library. Package lifecycle scripts are disabled."
            )
        }
        "github" => {
            ensure!(
                repo_ok(pkg)
                    && ver.parse::<u64>().is_ok()
                    && digest.len() == 64
                    && digest.bytes().all(|b| b.is_ascii_hexdigit()),
                "Use the repository, numeric release asset ID and SHA-256 from GitHub discovery"
            );
            format!(
                "Download GitHub release asset {pkg} #{ver} into Spindle’s tool library. SHA-256 {digest}. Supports raw executables and ZIP archives."
            )
        }
        _ => bail!(
            "Managed tool loaders: npm, github. For Python use install with the exact PyPI package."
        ),
    };
    Ok(format!(
        "{reason}\n\n{action}\nRuns in a separate sandbox when requested."
    ))
}
fn npm_cmd(node: &Path, cli: &Path, work: &Path, cache: &Path) -> std::process::Command {
    let mut c = std::process::Command::new(node);
    c.arg(cli).current_dir(work).env_clear();
    for key in ["SystemRoot", "windir", "SystemDrive", "ComSpec", "LANG"] {
        if let Some(v) = std::env::var_os(key) {
            c.env(key, v);
        }
    }
    c.env("HOME", work)
        .env("USERPROFILE", work)
        .env("PATH", node.parent().unwrap())
        .env("TMP", cache)
        .env("TEMP", cache)
        .env("TMPDIR", cache)
        .env("NPM_CONFIG_USERCONFIG", work.join("empty.npmrc"))
        .env("NPM_CONFIG_GLOBALCONFIG", work.join("empty-global.npmrc"))
        .env("NPM_CONFIG_CACHE", cache)
        .env("NPM_CONFIG_GIT", work.join("git-disabled"))
        .env("NPM_CONFIG_REGISTRY", REGISTRY)
        .env("NPM_CONFIG_IGNORE_SCRIPTS", "true")
        .env("NPM_CONFIG_FETCH_RETRIES", "1")
        .env("NPM_CONFIG_FETCH_TIMEOUT", "30000")
        .env("NPM_CONFIG_AUDIT", "false")
        .env("NPM_CONFIG_FUND", "false");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    c
}
fn npm_install(
    stage: &Path,
    pkg: &str,
    ver: &str,
    cancel: &AtomicBool,
) -> Result<BTreeMap<String, String>> {
    let (node, cli) = node().context("Node.js and npm are required on this host")?;
    let cache = tempfile::tempdir()?;
    // Older npm silently ignores unknown configuration. Check its supported defaults
    // before allowing it to resolve untrusted transitive dependency specifications.
    let config = npm_cmd(&node, &cli, stage, cache.path())
        .args(["config", "list", "--long", "--json"])
        .output()?;
    let config: Value =
        serde_json::from_slice(&config.stdout).context("Cannot inspect npm registry safeguards")?;
    let restrictions = ["allow-git", "allow-directory", "allow-file", "allow-remote"];
    ensure!(
        restrictions.iter().all(|key| config.get(*key).is_some()),
        "This npm lacks registry-only dependency controls. Use npm 11.16 or newer, or choose a portable release asset."
    );
    fs::write(
        stage.join("package.json"),
        serde_json::to_vec(
            &json!({"name":"spindle-tool","version":"1.0.0","private":true,"dependencies":{(pkg):ver}}),
        )?,
    )?;
    for args in [
        vec!["install", "--package-lock-only"],
        vec!["ci", "--omit=dev", "--no-bin-links"],
    ] {
        let mut cmd = npm_cmd(&node, &cli, stage, cache.path());
        for key in restrictions {
            cmd.arg(format!("--{key}=none"));
        }
        let log = tempfile::NamedTempFile::new_in(cache.path())?;
        let mut child = cmd
            .args(args)
            .arg("--ignore-scripts")
            .stdout(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .stderr(log.reopen()?)
            .spawn()?;
        let started = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) || started.elapsed() > Duration::from_secs(180) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("The tool installation was stopped or exceeded its time limit");
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    let mut error = vec![];
                    log.reopen()?.take(2048).read_to_end(&mut error)?;
                    bail!("npm failed: {}", String::from_utf8_lossy(&error));
                }
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let lock: Value = serde_json::from_slice(&fs::read(stage.join("package-lock.json"))?)?;
        let list = lock["packages"]
            .as_object()
            .context("npm 7 or newer is required")?;
        ensure!(
            list.len() <= 512,
            "Tool dependency graph exceeds the 512-package limit"
        );
        for (path, p) in list.iter().filter(|(p, _)| !p.is_empty()) {
            let url = reqwest::Url::parse(
                p["resolved"]
                    .as_str()
                    .context("Tool has a non-registry dependency")?,
            )?;
            ensure!(
                path.starts_with("node_modules/")
                    && !path.split('/').any(|s| s == "..")
                    && url.scheme() == "https"
                    && url.host_str() == Some("registry.npmjs.org")
                    && p["integrity"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("sha512-")),
                "Only integrity-pinned npm registry packages are supported"
            );
        }
    }
    let dir = stage.join("node_modules").join(pkg);
    let manifest: Value = serde_json::from_slice(&fs::read(dir.join("package.json"))?)?;
    let mut commands = BTreeMap::new();
    let mut add = |name: &str, entry: &str| -> Result<()> {
        ensure!(component(name), "Invalid npm executable name");
        let path = dir.join(entry).canonicalize()?;
        ensure!(
            path.starts_with(dir.canonicalize()?) && path.is_file(),
            "Invalid npm executable path"
        );
        commands.insert(
            name.to_owned(),
            path.strip_prefix(stage.canonicalize()?)?
                .to_string_lossy()
                .into(),
        );
        Ok(())
    };
    if let Some(bin) = manifest["bin"].as_str() {
        add(pkg.rsplit('/').next().unwrap(), bin)?;
    } else if let Some(bins) = manifest["bin"].as_object() {
        for (k, v) in bins {
            add(k, v.as_str().context("Invalid npm executable")?)?;
        }
    }
    ensure!(
        !commands.is_empty(),
        "This npm package has no runnable CLI. Choose its CLI package instead"
    );
    fs::copy(
        node,
        stage.join(format!("node{}", std::env::consts::EXE_SUFFIX)),
    )?;
    Ok(commands)
}

fn unpack_portable(data: &[u8], name: &str, stage: &Path) -> Result<BTreeMap<String, String>> {
    let mut commands = BTreeMap::new();
    let mut add = |rel: &str, bytes: &[u8]| -> Result<()> {
        let p = stage.join("files").join(rel);
        fs::create_dir_all(p.parent().unwrap())?;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)?;
        f.write_all(bytes)?;
        let basename = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if crate::native::is_native(bytes)
            && (!cfg!(windows) || p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")))
            && component(basename)
        {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&p, fs::Permissions::from_mode(0o755))?;
            }
            commands.insert(basename.to_owned(), format!("files/{rel}"));
        }
        Ok(())
    };
    if name.to_ascii_lowercase().ends_with(".zip") {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(data))?;
        ensure!(z.len() <= 2048, "Too many archive entries");
        let mut remaining = LIMIT * 3;
        for i in 0..z.len() {
            let entry = z.by_index(i)?;
            if entry.is_dir() {
                continue;
            }
            let rel = crate::native::stage_path(entry.name())?;
            ensure!(
                entry
                    .unix_mode()
                    .is_none_or(|m| matches!(m & 0o170000, 0 | 0o100000))
                    && entry.size() <= remaining as u64,
                "Unsupported or oversized archive entry"
            );
            let mut bytes = vec![];
            entry.take(remaining as u64 + 1).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= remaining, "Tool archive exceeds limit");
            remaining -= bytes.len();
            add(&rel, &bytes)?;
        }
    } else {
        let rel = crate::native::stage_path(name)?;
        ensure!(!rel.contains('/'), "Invalid asset filename");
        add(&rel, data)?;
    }
    ensure!(
        !commands.is_empty(),
        "No runnable executable for this host in the asset"
    );
    Ok(commands)
}

pub async fn load(
    source: &str,
    pkg: &str,
    ver: &str,
    digest: &str,
    reason: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<String> {
    preview(source, pkg, ver, digest, reason)?;
    let key = id(source, pkg, ver);
    let base = store();
    fs::create_dir_all(&base)?;
    let dest = base.join(&key);
    if dest.is_dir() {
        let t: Tool = serde_json::from_slice(&fs::read(dest.join("tool.json"))?)?;
        return Ok(describe(&t));
    }
    let stage = tempfile::tempdir_in(&base)?;
    let commands = if source == "npm" {
        let (dir, p, v) = (stage.path().to_owned(), pkg.to_owned(), ver.to_owned());
        let cancel = cancel.clone();
        tokio::task::spawn_blocking(move || npm_install(&dir, &p, &v, &cancel)).await??
    } else {
        let http = client()?;
        let url = format!("{GH}/repos/{pkg}/releases/assets/{ver}");
        let meta = value(http.get(&url)).await?;
        ensure!(
            meta["digest"] == format!("sha256:{digest}")
                && meta["size"].as_u64().is_some_and(|n| n <= LIMIT as u64),
            "Release digest changed or asset exceeds 96 MiB"
        );
        let data = bounded(
            http.get(&url).header("Accept", "application/octet-stream"),
            LIMIT,
        )
        .await?;
        ensure!(
            hex(&sha256(&data)) == digest,
            "Release asset failed SHA-256 verification"
        );
        unpack_portable(
            &data,
            meta["name"].as_str().context("Missing asset filename")?,
            stage.path(),
        )?
    };
    let tool = Tool {
        id: key,
        source: source.into(),
        package: pkg.into(),
        version: ver.into(),
        commands,
    };
    fs::write(stage.path().join("tool.json"), serde_json::to_vec(&tool)?)?;
    ensure!(!cancel.load(Ordering::Relaxed), "Tool installation stopped");
    if let Err(err) = fs::rename(stage.path(), &dest)
        && !dest.join("tool.json").is_file()
    {
        return Err(err.into());
    }
    Ok(describe(&tool))
}

pub fn run(
    id: &str,
    command: &str,
    args: &[String],
    work: &Path,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<String> {
    ensure!(
        id.len() == 24
            && id.bytes().all(|b| b.is_ascii_hexdigit())
            && args.len() <= 48
            && args.iter().all(|a| a.len() <= 2000 && !a.contains('\0')),
        "Invalid tool ID or arguments"
    );
    let dir = store().join(id).canonicalize()?;
    let t: Tool = serde_json::from_slice(&fs::read(dir.join("tool.json"))?)?;
    let entry = dir
        .join(
            t.commands
                .get(command)
                .context("Command is not provided by this tool")?,
        )
        .canonicalize()?;
    ensure!(
        entry.starts_with(&dir),
        "Tool entry escaped its installation folder"
    );
    let mut argv = vec![];
    let exe = if t.source == "npm" {
        // AppContainer can read the approved tool directory, but cannot enumerate drive roots.
        // Keep Node's module resolution within the paths npm materialized in the tool library.
        argv.extend([
            "--preserve-symlinks".into(),
            "--preserve-symlinks-main".into(),
        ]);
        argv.push(crate::projects::display_path(&entry));
        dir.join(format!("node{}", std::env::consts::EXE_SUFFIX))
    } else {
        entry
    };
    argv.extend_from_slice(args);
    let spec = crate::sandbox::Spec {
        program: exe,
        work: work.canonicalize()?,
        args: argv,
        readonly: vec![dir],
        python_path: vec![],
        timeout: Duration::from_secs(120),
        memory_mb: 3072,
        max_processes: 32,
    };
    let o = crate::sandbox::run_private(&spec, cancel)?;
    let report = o.report(&spec.work, 4800);
    ensure!(o.ok(), "{report}");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loads_require_exact_names_versions_and_digests() {
        for name in [
            "../x",
            "git+https://example.test",
            "x@latest",
            "--help",
            "@scope/../../x",
        ] {
            assert!(!npm_name(name));
        }
        assert!(npm_name("@scope/new-language"));
        assert!(pinned("1.2.3-beta.1"));
        assert!(!pinned("latest"));
        assert!(preview("github", "owner/repo", "124", "bad", "compile").is_err());
        assert!(!url_ok("file:///tmp/settings"));
    }

    #[tokio::test]
    #[ignore = "requires public search and npm, runs a real CLI in an isolated test profile"]
    async fn discover_load_and_run_a_registry_tool() {
        assert!(std::env::var_os("SPINDLE_CATALOG_QA").is_some());
        let web = web_search("Tauri desktop framework official")
            .await
            .unwrap();
        assert!(
            web.contains("https://") && web.to_lowercase().contains("tauri"),
            "{web}"
        );
        let found = find("npm", "@tauri-apps/cli").await.unwrap();
        assert!(found.contains("@tauri-apps/cli"));
        let loaded = load(
            "npm",
            "semver",
            "7.7.2",
            "",
            "Compare versions in a build",
            &Default::default(),
        )
        .await
        .unwrap();
        println!("{loaded}");
        let tmp = tempfile::tempdir().unwrap();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let result = run(
            &id("npm", "semver", "7.7.2"),
            "semver",
            &["1.2.3".into(), "-r".into(), ">=1".into()],
            tmp.path(),
            &cancel,
        )
        .unwrap();
        assert!(result.contains("1.2.3"), "{result}");
        println!("Web and registry search returned results. Version tool: {result}");
        let release = value(
            client()
                .unwrap()
                .get(format!("{REGISTRY}/@tauri-apps%2fcli/latest")),
        )
        .await
        .unwrap();
        let version = release["version"].as_str().unwrap();
        load(
            "npm",
            "@tauri-apps/cli",
            version,
            "",
            "Check the desktop build CLI",
            &cancel,
        )
        .await
        .unwrap();
        let tauri = run(
            &id("npm", "@tauri-apps/cli", version),
            "tauri",
            &["--version".into()],
            tmp.path(),
            &cancel,
        )
        .unwrap();
        assert!(tauri.contains(version), "{tauri}");
        println!("{tauri}");
    }

    #[tokio::test]
    #[ignore = "requires the public GitHub release service and a working OS sandbox"]
    async fn load_and_run_a_portable_release() -> Result<()> {
        ensure!(
            std::env::var_os("SPINDLE_CATALOG_QA").is_some(),
            "Set the QA profile"
        );
        let repo = "jqlang/jq";
        let name = if cfg!(windows) {
            "jq-windows-amd64.exe"
        } else {
            "jq-linux-amd64"
        };
        let meta = value(client()?.get(format!("{GH}/repos/{repo}/releases/latest"))).await?;
        let asset = meta["assets"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["name"] == name))
            .context("No matching release")?;
        let digest = asset["digest"]
            .as_str()
            .and_then(|s| s.strip_prefix("sha256:"))
            .context("Missing release digest")?;
        let version = asset["id"]
            .as_u64()
            .context("Missing asset ID")?
            .to_string();
        load(
            "github",
            repo,
            &version,
            digest,
            "Inspect JSON build reports",
            &Default::default(),
        )
        .await?;
        let tmp = tempfile::tempdir()?;
        let result = run(
            &id("github", repo, &version),
            name.trim_end_matches(".exe"),
            &["--version".into()],
            tmp.path(),
            &Default::default(),
        )?;
        ensure!(result.contains("jq-"), "{result}");
        println!("{result}");
        Ok(())
    }
}
