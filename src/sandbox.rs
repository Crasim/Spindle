//! The sandbox: one contract, two launchers.
//!
//! A sandboxed program sees Spindle's Python read-only, one working folder read-write, and
//! nothing else of the user's files; it has no network, it can't leave processes behind, and it
//! is stopped at its time and memory limits. On Linux this is bubblewrap (user, mount, PID,
//! network and IPC namespaces, all capabilities dropped) plus resource limits; on Windows an
//! AppContainer with no capabilities inside a Job Object. `probe` checks each promise by trying
//! to break it, and execution stays off unless every check passes. There is no fallback to
//! running unsandboxed.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

#[cfg(windows)]
#[path = "sandbox_windows.rs"]
mod win;

#[cfg(windows)]
pub(crate) fn run_host(spec: &Spec, cancel: &Arc<AtomicBool>, raw_suffix: Option<&str>) -> Result<Outcome> {
    win::run_host(spec, cancel, raw_suffix)
}

pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The only folder the program can write.
    pub work: PathBuf,
    /// Folders the program can read (Spindle's Python and its helper modules).
    pub readonly: Vec<PathBuf>,
    /// Folders added to `PYTHONPATH`; they must also be in `readonly`.
    pub python_path: Vec<PathBuf>,
    pub timeout: Duration,
    pub memory_mb: u64,
    pub max_processes: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub code: Option<i32>,
    pub timed_out: bool,
    pub cancelled: bool,
    pub stdout: String,
    pub stderr: String,
    pub millis: u64,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.code == Some(0) && !self.timed_out && !self.cancelled
    }

    /// The combined output for the model, keeping the end, where failures are reported.
    pub fn report(&self, work: &Path, limit: usize) -> String {
        let status = if self.timed_out {
            "stopped at the time limit".to_owned()
        } else if self.cancelled {
            "stopped by the user".to_owned()
        } else {
            match self.code {
                Some(c) => format!("exit code {c}"),
                None => "ended by a signal".to_owned(),
            }
        };
        let mut body = String::new();
        if !self.stdout.trim().is_empty() {
            body.push_str(self.stdout.trim_end());
            body.push('\n');
        }
        if !self.stderr.trim().is_empty() {
            body.push_str(self.stderr.trim_end());
            body.push('\n');
        }
        let mut body = body;
        let display = crate::projects::display_path(work);
        for shown in [
            work.display().to_string(),
            display.clone(),
            display.replace('\\', "/"),
            SANDBOX_WORK.to_owned(),
        ] {
            body = body
                .replace(&format!("{shown}/"), "")
                .replace(&format!("{shown}\\"), "")
                .replace(&shown, ".");
        }
        let body = if body.len() > limit {
            let mut start = body.len() - limit;
            while !body.is_char_boundary(start) {
                start += 1;
            }
            format!("… (earlier output cut)\n{}", &body[start..])
        } else {
            body
        };
        format!("{status} · {:.1}s\n{body}", self.millis as f64 / 1000.)
    }
}

pub const OUTPUT_LIMIT: usize = 512 * 1024;

/// Where the working folder appears inside the Linux sandbox.
pub const SANDBOX_WORK: &str = "/work";

/// Keep at most `OUTPUT_LIMIT` bytes of a stream, reading it to the end so the program never
/// blocks on a full pipe.
pub(crate) fn drain(mut reader: impl std::io::Read) -> String {
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = OUTPUT_LIMIT.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
    String::from_utf8_lossy(&kept).into_owned()
}

/// The environment a sandboxed program gets, and nothing else.
pub(crate) fn environment(spec: &Spec) -> Vec<(String, String)> {
    let work = spec.work.display().to_string();
    let bin = spec
        .program
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let mut env: Vec<(String, String)> = vec![
        ("HOME".into(), work.clone()),
        ("PYTHONDONTWRITEBYTECODE".into(), "1".into()),
        ("PYTHONNOUSERSITE".into(), "1".into()),
        ("PYTHONUTF8".into(), "1".into()),
        ("PYTHONIOENCODING".into(), "utf-8".into()),
        ("SPINDLE_SANDBOX".into(), "1".into()),
    ];
    if !spec.python_path.is_empty() {
        let sep = if cfg!(windows) { ";" } else { ":" };
        let joined: Vec<String> = spec
            .python_path
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        env.push(("PYTHONPATH".into(), joined.join(sep)));
    }
    if cfg!(windows) {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let tmp = spec.work.join(".tmp").display().to_string();
        // Windows needs its standard system variables to start a process in an AppContainer
        // (a missing one fails with error 203). These describe the system, not the user's
        // secrets; everything else in Spindle's own environment is left out.
        for key in [
            "SystemDrive",
            "windir",
            "ComSpec",
            "PATHEXT",
            "OS",
            "NUMBER_OF_PROCESSORS",
            "PROCESSOR_ARCHITECTURE",
            "PROCESSOR_IDENTIFIER",
            "PROCESSOR_LEVEL",
            "PROCESSOR_REVISION",
            "ProgramData",
            "ALLUSERSPROFILE",
            "PUBLIC",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "CommonProgramFiles",
            "CommonProgramFiles(x86)",
            "CommonProgramW6432",
            "LOCALAPPDATA",
            "APPDATA",
            "HOMEDRIVE",
            "HOMEPATH",
            "USERNAME",
            "USERDOMAIN",
            "COMPUTERNAME",
        ] {
            if let Ok(value) = std::env::var(key) {
                env.push((key.into(), value));
            }
        }
        env.extend([
            ("SystemRoot".into(), root.clone()),
            ("PATH".into(), format!("{bin};{root}\\System32")),
            ("TEMP".into(), tmp.clone()),
            ("TMP".into(), tmp),
            ("USERPROFILE".into(), work),
        ]);
        // Windows expects the block sorted by name, ignoring case.
        env.sort_by_key(|(k, _)| k.to_uppercase());
    } else {
        env.extend([
            ("PATH".into(), format!("{bin}:/usr/bin:/bin")),
            ("LANG".into(), "C.UTF-8".into()),
            ("TMPDIR".into(), "/tmp".into()),
        ]);
    }
    env
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Backend {
    Bubblewrap(PathBuf),
    AppContainer,
}

impl Backend {
    pub fn label(&self) -> String {
        match self {
            Backend::Bubblewrap(p) => format!("bubblewrap ({})", p.display()),
            Backend::AppContainer => "AppContainer + Job Object".into(),
        }
    }
}

/// The launcher for this system, or why there is none.
pub fn backend() -> Result<Backend> {
    if cfg!(target_os = "android") {
        bail!(
            "The desktop code sandbox is unavailable on Android. Ask for a Java Android APK to use the isolated on-device builder."
        );
    }
    if cfg!(windows) {
        return Ok(Backend::AppContainer);
    }
    let found = ["/usr/bin/bwrap", "/bin/bwrap", "/usr/local/bin/bwrap"]
        .into_iter()
        .map(PathBuf::from)
        .find(|p| p.is_file());
    match found {
        Some(p) => Ok(Backend::Bubblewrap(p)),
        None => bail!(
            "bubblewrap isn't installed. Install the bubblewrap package (for example `sudo dnf install bubblewrap` or `sudo apt install bubblewrap`), then run the check again."
        ),
    }
}

/// Run a program in the sandbox. `cancel` stops it early.
pub fn run(spec: &Spec, cancel: &Arc<AtomicBool>) -> Result<Outcome> {
    launch(spec, cancel, true)
}

pub fn run_private(spec: &Spec, cancel: &Arc<AtomicBool>) -> Result<Outcome> {
    launch(spec, cancel, true)
}

fn launch_environment(spec: &Spec, private: bool) -> Vec<(String, String)> {
    let mut env = environment(spec);
    if private {
        env.retain(|(name, _)| {
            !matches!(
                name.as_str(),
                "APPDATA"
                    | "LOCALAPPDATA"
                    | "HOMEDRIVE"
                    | "HOMEPATH"
                    | "USERNAME"
                    | "USERDOMAIN"
                    | "COMPUTERNAME"
            )
        });
        let work = if cfg!(windows) {
            spec.work.clone()
        } else {
            PathBuf::from(SANDBOX_WORK)
        };
        let tmp = work.join(".tmp");
        env.extend([
            (
                "ZIG_GLOBAL_CACHE_DIR".into(),
                crate::projects::display_path(&tmp.join("zig-global")),
            ),
            (
                "ZIG_LOCAL_CACHE_DIR".into(),
                crate::projects::display_path(&tmp.join("zig-local")),
            ),
        ]);
        if cfg!(windows) {
            let local = crate::projects::display_path(&tmp);
            env.retain(|(name, _)| !matches!(name.as_str(), "TEMP" | "TMP" | "USERPROFILE"));
            for name in ["TEMP", "TMP", "LOCALAPPDATA", "APPDATA"] {
                env.push((name.into(), local.clone()));
            }
            env.push(("USERPROFILE".into(), crate::projects::display_path(&work)));
            env.sort_by_key(|(name, _)| name.to_uppercase());
        }
    }
    env
}

fn launch(spec: &Spec, cancel: &Arc<AtomicBool>, private: bool) -> Result<Outcome> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(Outcome {
            cancelled: true,
            ..Default::default()
        });
    }
    std::fs::create_dir_all(&spec.work)?;
    std::fs::create_dir_all(spec.work.join(".tmp"))?;
    if private && !spec.readonly.is_empty() {
        std::fs::create_dir_all(spec.work.join(".tmp").join("zig-global"))?;
        std::fs::create_dir_all(spec.work.join(".tmp").join("zig-local"))?;
    }
    match backend()? {
        #[cfg(unix)]
        Backend::Bubblewrap(bwrap) => linux(&bwrap, spec, cancel, private),
        #[cfg(windows)]
        Backend::AppContainer => win::run(spec, cancel, private),
        #[allow(unreachable_patterns)]
        other => bail!("{} isn't available on this system", other.label()),
    }
}

#[cfg(unix)]
fn linux(bwrap: &Path, spec: &Spec, cancel: &Arc<AtomicBool>, private: bool) -> Result<Outcome> {
    use anyhow::Context;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::atomic::Ordering;
    let mut cmd = Command::new(bwrap);
    cmd.args([
        "--unshare-all",
        "--die-with-parent",
        "--new-session",
        "--cap-drop",
        "ALL",
    ])
    .args(["--hostname", "spindle", "--clearenv"]);
    // System libraries, read-only; no home folders, no /etc beyond what the loader needs.
    for dir in ["/usr", "/lib", "/lib64", "/bin", "/sbin"] {
        cmd.args(["--ro-bind-try", dir, dir]);
    }
    cmd.args(["--ro-bind-try", "/etc/ld.so.cache", "/etc/ld.so.cache"]);
    // Mount the fresh /tmp, /proc and /dev before the folders, so a folder that happens to live
    // under /tmp isn't hidden by them.
    cmd.args(["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"]);
    for dir in &spec.readonly {
        cmd.arg("--ro-bind").arg(dir).arg(dir);
    }
    // The working folder appears as /work, so no host path (or user name) is visible inside.
    cmd.arg("--bind").arg(&spec.work).arg(SANDBOX_WORK);
    cmd.args(["--chdir", SANDBOX_WORK]);
    for (k, v) in launch_environment(spec, private) {
        let v = if k == "HOME" {
            SANDBOX_WORK.to_owned()
        } else {
            v
        };
        cmd.args(["--setenv", &k, &v]);
    }
    cmd.arg("--").arg(&spec.program).args(&spec.args);
    cmd.env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let memory = spec.memory_mb * 1024 * 1024;
    let processes = u64::from(spec.max_processes);
    let cpu = spec.timeout.as_secs() + 5;
    // SAFETY: only async-signal-safe calls (setrlimit) between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            let limit = |res, value: u64| {
                let r = libc::rlimit {
                    rlim_cur: value as libc::rlim_t,
                    rlim_max: value as libc::rlim_t,
                };
                if libc::setrlimit(res, &r) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            };
            limit(libc::RLIMIT_AS, memory + 256 * 1024 * 1024)?;
            limit(libc::RLIMIT_CPU, cpu)?;
            limit(libc::RLIMIT_FSIZE, 512 * 1024 * 1024)?;
            limit(libc::RLIMIT_CORE, 0)?;
            // Counted per user, so it also bounds a fork bomb inside the namespace.
            let current = {
                let mut r = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                libc::getrlimit(libc::RLIMIT_NPROC, &mut r);
                r.rlim_cur as u64
            };
            limit(libc::RLIMIT_NPROC, current.min(processes + 4096))?;
            Ok(())
        });
    }
    let started = Instant::now();
    let mut child = cmd.spawn().context("Couldn't start bubblewrap")?;
    let out = child
        .stdout
        .take()
        .map(|s| std::thread::spawn(move || drain(s)));
    let err = child
        .stderr
        .take()
        .map(|s| std::thread::spawn(move || drain(s)));
    let mut outcome = Outcome::default();
    loop {
        if let Some(status) = child.try_wait()? {
            outcome.code = status.code();
            break;
        }
        let over = started.elapsed() > spec.timeout;
        if over || cancel.load(Ordering::Relaxed) {
            // Killing bubblewrap ends the sandbox's PID namespace and every process in it.
            let _ = child.kill();
            let _ = child.wait();
            outcome.timed_out = over;
            outcome.cancelled = !over;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    outcome.millis = started.elapsed().as_millis() as u64;
    outcome.stdout = out.and_then(|h| h.join().ok()).unwrap_or_default();
    outcome.stderr = err.and_then(|h| h.join().ok()).unwrap_or_default();
    // bubblewrap reports its own setup failures on stderr with exit code 1 before running
    // anything; surface those as errors rather than as the program's output.
    if outcome.code == Some(1) && outcome.stderr.starts_with("bwrap:") && outcome.stdout.is_empty()
    {
        bail!("The sandbox couldn't start: {}", outcome.stderr.trim());
    }
    Ok(outcome)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub backend: String,
    pub runtime: String,
    pub checks: Vec<Check>,
    pub ready: bool,
    pub time: i64,
    /// Why execution is off, when it is.
    #[serde(default)]
    pub problem: String,
}

impl Report {
    /// The report as plain text, to copy into a message.
    pub fn text(&self) -> String {
        let mut out = format!(
            "Spindle sandbox check · {} · {}\n",
            self.backend, self.runtime
        );
        for c in &self.checks {
            out.push_str(&format!(
                "{} {} · {}\n",
                if c.passed { "PASS" } else { "FAIL" },
                c.name,
                c.detail
            ));
        }
        if !self.problem.is_empty() {
            out.push_str(&self.problem);
            out.push('\n');
        }
        out.push_str(if self.ready { "READY\n" } else { "NOT READY\n" });
        out
    }
}

const PROBE: &str = r#"
import json, os, socket, subprocess, sys, time
mode, outside, home, work = sys.argv[1], sys.argv[2], sys.argv[3], os.getcwd()
def attempt(f):
    try:
        f()
        return "allowed"
    except Exception as e:
        return "blocked: " + type(e).__name__
if mode == "checks":
    r = {}
    r["write_work"] = attempt(lambda: open(os.path.join(work, "probe.txt"), "w").write("ok"))
    r["read_outside"] = attempt(lambda: open(os.path.join(outside, "secret.txt")).read())
    r["write_outside"] = attempt(lambda: open(os.path.join(outside, "escaped.txt"), "w").write("x"))
    r["list_home"] = attempt(lambda: os.listdir(home))
    def connect():
        s = socket.create_connection(("1.1.1.1", 53), timeout=3)
        s.close()
    r["network"] = attempt(connect)
    r["dns"] = attempt(lambda: socket.getaddrinfo("example.com", 443))
    print(json.dumps(r))
elif mode == "orphan":
    beat = os.path.join(work, "heartbeat.txt")
    code = "import time\nwhile True:\n    open(%r, 'a').write('.')\n    time.sleep(0.1)\n" % beat
    subprocess.Popen([sys.executable, "-c", code], start_new_session=True)
    time.sleep(0.5)
    print("left a child running")
elif mode == "spin":
    while True:
        pass
elif mode == "memory":
    block = bytearray(2 * 1024 * 1024 * 1024)
    print("allocated", len(block))
"#;

fn check(name: &str, passed: bool, detail: impl Into<String>) -> Check {
    Check {
        name: name.into(),
        passed,
        detail: detail.into(),
    }
}

/// Try to break each promise the sandbox makes. Execution is enabled only if every check passes.
pub fn probe(runtime: &crate::runtime::Runtime, scratch: &Path) -> Report {
    let mut report = Report {
        runtime: format!("Python {} ({})", runtime.version, runtime.python.display()),
        time: crate::automation::now(),
        ..Default::default()
    };
    match backend() {
        Ok(b) => report.backend = b.label(),
        Err(e) => {
            report.problem = format!("{e:#}");
            return report;
        }
    }
    let _ = std::fs::create_dir_all(scratch);
    let base = match tempfile::Builder::new()
        .prefix("spindle-probe-")
        .tempdir_in(scratch)
    {
        Ok(t) => t,
        Err(e) => {
            report.problem = format!("Couldn't make a test folder: {e}");
            return report;
        }
    };
    let work = base.path().join("work");
    let outside = base.path().join("outside");
    let _ = std::fs::create_dir_all(&work);
    let _ = std::fs::create_dir_all(&outside);
    let _ = std::fs::write(outside.join("secret.txt"), "private");
    let _ = std::fs::write(work.join("probe.py"), PROBE);
    let cancel = Arc::new(AtomicBool::new(false));
    // The user's own home folder: the sandbox must not be able to list it.
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .filter(|h| h.is_dir() && !work.starts_with(h.join("nonexistent")))
        .unwrap_or_else(|| outside.clone());
    let spec = |mode: &str, seconds: u64| Spec {
        program: runtime.python.clone(),
        args: vec![
            "probe.py".into(),
            mode.into(),
            outside.display().to_string(),
            home.display().to_string(),
        ],
        work: work.clone(),
        readonly: vec![runtime.home.clone()],
        timeout: Duration::from_secs(seconds),
        memory_mb: 512,
        max_processes: 32,
        python_path: vec![],
    };
    match run(&spec("checks", 30), &cancel) {
        Ok(out) if out.ok() => {
            let found: serde_json::Value =
                serde_json::from_str(out.stdout.trim()).unwrap_or_default();
            let get = |k: &str| {
                found
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("no answer")
                    .to_owned()
            };
            report.checks.push(check(
                "Starts Spindle's Python",
                true,
                format!("{:.1}s", out.millis as f64 / 1000.),
            ));
            let wrote = std::fs::read_to_string(work.join("probe.txt")).is_ok_and(|s| s == "ok");
            report.checks.push(check(
                "Can write its working folder",
                wrote && get("write_work") == "allowed",
                get("write_work"),
            ));
            report.checks.push(check(
                "Can't read other files",
                get("read_outside").starts_with("blocked"),
                get("read_outside"),
            ));
            let escaped = outside.join("escaped.txt").exists();
            report.checks.push(check(
                "Can't write other folders",
                get("write_outside").starts_with("blocked") && !escaped,
                if escaped {
                    "a file appeared outside".into()
                } else {
                    get("write_outside")
                },
            ));
            report.checks.push(check(
                "Can't list your home folder",
                get("list_home").starts_with("blocked"),
                get("list_home"),
            ));
            report.checks.push(check(
                "No network",
                get("network").starts_with("blocked"),
                get("network"),
            ));
            report.checks.push(check(
                "No name lookups",
                get("dns").starts_with("blocked"),
                get("dns"),
            ));
        }
        Ok(out) => report.checks.push(check(
            "Starts Spindle's Python",
            false,
            out.report(&work, 600),
        )),
        Err(e) => report
            .checks
            .push(check("Starts Spindle's Python", false, format!("{e:#}"))),
    }
    if report.checks.first().is_some_and(|c| c.passed) {
        let beat = work.join("heartbeat.txt");
        let orphan = run(&spec("orphan", 20), &cancel);
        std::thread::sleep(Duration::from_millis(400));
        let before = std::fs::metadata(&beat).map(|m| m.len()).unwrap_or(0);
        std::thread::sleep(Duration::from_millis(1200));
        let after = std::fs::metadata(&beat).map(|m| m.len()).unwrap_or(0);
        report.checks.push(check(
            "Leaves no processes behind",
            orphan.is_ok() && before > 0 && after == before,
            if before == 0 {
                "the test child never started".to_owned()
            } else if after > before {
                "a child kept running after the program ended".to_owned()
            } else {
                "the child ended with the program".to_owned()
            },
        ));
        let started = Instant::now();
        let spin = run(&spec("spin", 2), &cancel);
        let took = started.elapsed();
        report.checks.push(check(
            "Stops at the time limit",
            spin.as_ref().is_ok_and(|o| o.timed_out) && took < Duration::from_secs(8),
            format!("{:.1}s", took.as_secs_f64()),
        ));
        let memory = run(&spec("memory", 20), &cancel);
        report.checks.push(check(
            "Stops at the memory limit",
            memory
                .as_ref()
                .is_ok_and(|o| !o.ok() && !o.stdout.contains("allocated")),
            match &memory {
                Ok(o) if o.stderr.contains("MemoryError") => "MemoryError".to_owned(),
                Ok(o) => o
                    .report(&work, 200)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned(),
                Err(e) => format!("{e:#}"),
            },
        ));
    }
    report.ready = !report.checks.is_empty()
        && report.checks.iter().all(|c| c.passed)
        && report.checks.len() >= 10;
    if !report.ready && report.problem.is_empty() {
        report.problem = "Some checks failed, so running code stays off.".into();
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The probe on this machine, when bubblewrap and a Python are available. Skipped (not
    /// failed) elsewhere; the report is what the user runs on their own computer.
    #[test]
    fn the_sandbox_keeps_its_promises() {
        if backend().is_err() {
            return;
        }
        let Some(python) = ["/usr/local/bin/python3", "/usr/bin/python3"]
            .into_iter()
            .find(|p| Path::new(p).is_file())
        else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let runtime = crate::runtime::Runtime {
            home: Path::new(python)
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .to_owned(),
            python: python.into(),
            version: "test".into(),
        };
        let report = probe(&runtime, tmp.path());
        for c in &report.checks {
            eprintln!(
                "{} {} · {}",
                if c.passed { "PASS" } else { "FAIL" },
                c.name,
                c.detail
            );
        }
        assert!(report.ready, "{report:#?}");
    }

    #[test]
    fn reports_keep_the_end_and_hide_the_work_path() {
        let work = PathBuf::from("/data/work/7");
        let out = Outcome {
            code: Some(1),
            stdout: format!(
                "{}\nFAIL: test_edge ({}/tests/test_a.py)",
                "x".repeat(100),
                work.display()
            ),
            millis: 1500,
            ..Default::default()
        };
        let text = out.report(&work, 60);
        assert!(text.starts_with("exit code 1 · 1.5s"));
        assert!(text.contains("FAIL: test_edge (tests/test_a.py)"), "{text}");
        assert!(text.contains("earlier output cut"));
    }
}
