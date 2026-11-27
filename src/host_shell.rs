//! One-shot commands on the current device, after explicit approval.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shell {
    #[serde(rename = "powershell", alias = "power_shell")]
    PowerShell,
    Cmd,
    Bash,
    Sh,
}

impl Shell {
    const ALL: [Self; 4] = [Self::PowerShell, Self::Cmd, Self::Bash, Self::Sh];

    pub fn name(self) -> &'static str {
        match self {
            Self::PowerShell => "PowerShell",
            Self::Cmd => "Command Prompt",
            Self::Bash => "Bash",
            Self::Sh => "sh",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::PowerShell => "powershell",
            Self::Cmd => "cmd",
            Self::Bash => "bash",
            Self::Sh => "sh",
        }
    }

    fn path(self) -> Option<PathBuf> {
        #[cfg(windows)]
        let paths = match self {
            Self::PowerShell => system_path("WindowsPowerShell/v1.0/powershell.exe"),
            Self::Cmd => system_path("cmd.exe"),
            Self::Bash => ["ProgramFiles", "ProgramFiles(x86)"]
                .into_iter()
                .filter_map(std::env::var_os)
                .map(|p| PathBuf::from(p).join("Git/bin/bash.exe"))
                .find(|p| p.is_file()),
            Self::Sh => None,
        };
        #[cfg(target_os = "linux")]
        let paths = match self {
            Self::Bash => Some(PathBuf::from("/bin/bash")),
            Self::Sh => Some(PathBuf::from("/bin/sh")),
            _ => None,
        };
        #[cfg(target_os = "android")]
        let paths = matches!(self, Self::Sh).then(|| PathBuf::from("/system/bin/sh"));
        #[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
        let paths: Option<PathBuf> = None;
        paths.filter(|p| p.is_absolute() && p.is_file())
    }

    fn validate(self, command: &str) -> Result<PathBuf> {
        let program = self.path().with_context(|| format!("{} is unavailable on this device. Check find_tools with source local for the available shells.", self.name()))?;
        ensure!(
            !command.trim().is_empty() && command.len() <= 4_000,
            "Supply a command with 1–4,000 bytes."
        );
        ensure!(
            !command
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\r' | '\n' | '\t')),
            "The command contains unsupported control characters."
        );
        ensure!(
            !matches!(self, Self::Cmd) || !command.contains(['\r', '\n']),
            "Command Prompt accepts one command line. Use PowerShell for a multiline script."
        );
        Ok(program)
    }
}

#[cfg(windows)]
fn system_path(file: &str) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut path = [0u16; 260];
    let len = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            path.as_mut_ptr(),
            path.len() as u32,
        )
    } as usize;
    (len > 0 && len < path.len())
        .then(|| PathBuf::from(std::ffi::OsString::from_wide(&path[..len])).join(file))
}

pub fn available() -> String {
    Shell::ALL
        .into_iter()
        .filter(|shell| shell.path().is_some())
        .map(Shell::key)
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn preview(shell: Shell, command: &str, root: &Path) -> Result<String> {
    let program = shell.validate(command)?;
    ensure!(root.is_dir(), "The working folder is unavailable.");
    let limits = if cfg!(target_os = "android") {
        "Runs with Spindle’s Android app permissions."
    } else {
        "Runs outside the sandbox with the current account’s permissions, without requesting elevation."
    };
    Ok(format!(
        "Run on This device · {}\n{}\nWorking folder: {}\n\n{command}\n\n{limits} Output is returned to the selected model.",
        shell.name(),
        program.display(),
        crate::projects::display_path(root)
    ))
}

pub(crate) fn environment() -> Vec<(String, String)> {
    let mut clean: Vec<_> = [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "LANG",
        "LC_ALL",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
    .collect();
    if cfg!(target_os = "android") {
        clean.push(("ANDROID_ROOT".into(), "/system".into()));
        clean.push(("ANDROID_DATA".into(), "/data".into()));
    }
    clean.sort();
    clean
}

pub fn run(shell: Shell, command: &str, root: &Path, cancel: &Arc<AtomicBool>) -> Result<String> {
    let program = shell.validate(command)?;
    ensure!(
        !cancel.load(Ordering::Relaxed),
        "Shell command stopped before launch."
    );
    let timeout = Duration::from_secs(120);
    #[cfg(windows)]
    let result = {
        use base64::Engine as _;
        let raw = matches!(shell, Shell::Cmd).then(|| format!("/D /S /C \"{command}\""));
        let args = match shell {
            Shell::PowerShell => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(
                    command
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect::<Vec<_>>(),
                );
                vec![
                    "-NoLogo".into(),
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-EncodedCommand".into(),
                    encoded,
                ]
            }
            Shell::Bash => vec![
                "--noprofile".into(),
                "--norc".into(),
                "-c".into(),
                command.into(),
            ],
            _ => vec![],
        };
        let spec = crate::sandbox::Spec {
            program,
            args,
            work: root.to_owned(),
            readonly: vec![],
            python_path: vec![],
            timeout,
            memory_mb: 4096,
            max_processes: 32,
        };
        crate::sandbox::run_host(&spec, cancel, raw.as_deref())?
    };
    #[cfg(unix)]
    let result = run_unix(&program, command, root, timeout, cancel)?;
    let report = result.report(root, 12_000);
    if !result.ok() {
        bail!("{} on this device: {report}", shell.name());
    }
    Ok(format!("{} on this device: {report}", shell.name()))
}

#[cfg(unix)]
fn run_unix(
    program: &Path,
    code: &str,
    root: &Path,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<crate::sandbox::Outcome> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Instant;
    let mut command = Command::new(program);
    command
        .args(["-c", code])
        .current_dir(root)
        .env_clear()
        .envs(environment())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let started = Instant::now();
    let mut child = command
        .spawn()
        .context("Couldn't launch the shell under Spindle's current account.")?;
    struct Group(i32);
    impl Drop for Group {
        fn drop(&mut self) {
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }
    let group = Group(child.id() as i32);
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    for fd in [out.as_raw_fd(), err.as_raw_fd()] {
        if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    fn collect(pipe: &mut impl Read, buf: &mut Vec<u8>) {
        let mut chunk = [0u8; 8192];
        for _ in 0..32 {
            let Ok(n) = pipe.read(&mut chunk) else { break };
            if n == 0 {
                break;
            }
            let room = crate::sandbox::OUTPUT_LIMIT.saturating_sub(buf.len());
            buf.extend_from_slice(&chunk[..n.min(room)]);
        }
    }
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut result = crate::sandbox::Outcome::default();
    let status = loop {
        collect(&mut out, &mut stdout);
        collect(&mut err, &mut stderr);
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        result.timed_out = started.elapsed() >= timeout;
        result.cancelled = cancel.load(Ordering::Relaxed);
        if result.timed_out || result.cancelled {
            break None;
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    drop(group);
    if status.is_none() {
        child.wait()?;
    }
    result.code = status.and_then(|status| status.code());
    result.millis = started.elapsed().as_millis() as u64;
    collect(&mut out, &mut stdout);
    collect(&mut err, &mut stderr);
    result.stdout = String::from_utf8_lossy(&stdout).into_owned();
    result.stderr = String::from_utf8_lossy(&stderr).into_owned();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_shells_run_on_the_local_host() -> Result<()> {
        let folder = tempfile::tempdir()?;
        for shell in Shell::ALL
            .into_iter()
            .filter(|shell| shell.path().is_some())
        {
            let code = match shell {
                Shell::PowerShell => "Write-Output spindle-shell-fixture",
                Shell::Cmd => "echo spindle-shell-fixture",
                _ => "printf 'spindle-shell-fixture\\n'",
            };
            assert!(preview(shell, code, folder.path())?.contains(code));
            let response = run(shell, code, folder.path(), &Arc::default())?;
            assert!(response.contains("spindle-shell-fixture"), "{response}");
        }
        Ok(())
    }
}
