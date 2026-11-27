//! Running Spindle in the background: `spindle service install|uninstall|status|stop`.
//!
//! The service is `spindle service --dir WORKSPACE`, a windowless Spindle that never stops on
//! its own: it keeps the workspace, syncs, runs schedules and shows the tray icon. It starts
//! when you sign in: a per-user Task Scheduler task on Windows (not a SYSTEM service: the
//! sandbox and stored keys belong to your account), a `systemd --user` unit on Linux.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// A short name for this workspace's service.
fn name(root: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut h);
    format!("spindle-{:08x}", h.finish() as u32)
}

fn exe() -> Result<PathBuf> {
    std::env::current_exe().context("Can't find Spindle's program file")
}

#[cfg(target_os = "linux")]
fn unit_path(root: &Path) -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .context("No config folder")?;
    Ok(base
        .join("systemd/user")
        .join(format!("{}.service", name(root))))
}

/// The unit file for Linux.
#[cfg(any(target_os = "linux", test))]
pub fn unit(root: &Path, exe: &Path) -> String {
    format!(
        "[Unit]\nDescription=Spindle in the background ({root})\nAfter=network-online.target\n\n[Service]\nExecStart=\"{exe}\" service --dir \"{root}\"\nRestart=on-failure\nRestartSec=10\n# The workspace passphrase (SPINDLE_SESSION_PASSPHRASE=…), readable only by you.\nEnvironmentFile=-%h/.config/spindle/service.env\n\n[Install]\nWantedBy=default.target\n",
        root = root.display(),
        exe = exe.display(),
    )
}

/// Start at sign-in, and start now.
pub fn install(root: &Path) -> Result<String> {
    register(root, true)
}

pub fn register(root: &Path, start: bool) -> Result<String> {
    let exe = exe()?;
    #[cfg(windows)]
    {
        let task = format!("Spindle {}", name(root));
        let run = format!("\"{}\" service --dir \"{}\"", exe.display(), root.display());
        let out = scheduler()
            .args([
                "/Create", "/F", "/SC", "ONLOGON", "/RL", "LIMITED", "/TN", &task, "/TR", &run,
            ])
            .output()
            .context("Couldn't run schtasks")?;
        if !out.status.success() {
            bail!(
                "Task Scheduler: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        if start {
            let _ = scheduler().args(["/Run", "/TN", &task]).output();
        }
        return Ok(format!(
            "Spindle starts at sign-in (Task Scheduler: \"{task}\")."
        ));
    }
    #[cfg(target_os = "linux")]
    {
        let path = unit_path(root)?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, unit(root, &exe))?;
        let unit_name = format!("{}.service", name(root));
        let ok = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .output()
            .is_ok_and(|o| o.status.success())
            && std::process::Command::new("systemctl")
                .args(["--user", "enable"])
                .args(start.then_some("--now"))
                .arg(&unit_name)
                .output()
                .is_ok_and(|o| o.status.success());
        let mut msg = format!("Installed {}", path.display());
        if !ok {
            msg.push_str(". systemd --user isn't available here; start it with `spindle service --dir …` yourself.");
        }
        msg.push_str(&format!(
            "\nPut SPINDLE_SESSION_PASSPHRASE=… in ~/.config/spindle/service.env (chmod 600) so it can open the workspace.\nTo keep it running when you're signed out (a headless host): loginctl enable-linger {}",
            std::env::var("USER").unwrap_or_else(|_| "$USER".into())
        ));
        return Ok(msg);
    }
    #[allow(unreachable_code)]
    {
        let _ = (root, exe, start);
        bail!("Running in the background isn't supported on this system yet")
    }
}

#[cfg(windows)]
fn scheduler() -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new("schtasks");
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    command
}

pub fn uninstall(root: &Path) -> Result<String> {
    #[cfg(windows)]
    {
        let task = format!("Spindle {}", name(root));
        let out = scheduler()
            .args(["/Delete", "/F", "/TN", &task])
            .output()?;
        if !out.status.success() && installed(root) {
            bail!("Task Scheduler: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        return Ok("Spindle no longer starts in the background.".into());
    }
    #[cfg(target_os = "linux")]
    {
        let unit_name = format!("{}.service", name(root));
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", &unit_name])
            .output();
        let path = unit_path(root)?;
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .output();
        return Ok("Spindle no longer starts in the background.".into());
    }
    #[allow(unreachable_code)]
    {
        let _ = root;
        bail!("Running in the background isn't supported on this system yet")
    }
}

/// Whether it's set to start at sign-in.
pub fn installed(root: &Path) -> bool {
    #[cfg(windows)]
    {
        let task = format!("Spindle {}", name(root));
        return scheduler()
            .args(["/Query", "/TN", &task])
            .output()
            .is_ok_and(|o| o.status.success());
    }
    #[cfg(target_os = "linux")]
    {
        return unit_path(root).is_ok_and(|p| p.exists());
    }
    #[allow(unreachable_code)]
    {
        let _ = root;
        false
    }
}

pub fn status(root: &Path) -> String {
    let running = crate::control::ping(root);
    format!(
        "{} · {}",
        if running { "Running" } else { "Not running" },
        if installed(root) {
            "starts when you sign in"
        } else {
            "not set to start when you sign in"
        }
    )
}

/// A full exit of whatever owns the workspace, holding running work for later.
pub fn stop(root: &Path) -> Result<String> {
    crate::control::exit_owner(root, "exit")?;
    Ok("Spindle stopped. Queued work is held for later.".into())
}
