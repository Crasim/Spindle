//! Spindle's own Python, the same build on Windows and Linux.
//!
//! The runtime is a pinned python-build-standalone release (the CPython builds uv uses). It is
//! downloaded only when the user clicks to set it up, checked against the SHA-256 recorded
//! here, and unpacked into Spindle's data folder, so no other Python on the computer is used or
//! changed. Code runs in it only through the sandbox.

use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};

pub struct Pin {
    pub version: &'static str,
    pub release: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

/// CPython 3.14.10 from python-build-standalone release 20261110 (hash cross-checked against
/// the pbs-installer catalog and the downloaded archive).
#[cfg(windows)]
pub const PIN: Pin = Pin {
    version: "3.14.10",
    release: "20261110",
    url: "https://github.com/astral-sh/python-build-standalone/releases/download/20261110/cpython-3.14.10%2B20261110-x86_64-pc-windows-msvc-install_only_stripped.tar.gz",
    sha256: "32b6cf90a03e2bc15fda8d2ab7ff78c4870f138887d612932650ce764efaab8f",
};

#[cfg(not(windows))]
pub const PIN: Pin = Pin {
    version: "3.14.10",
    release: "20261110",
    url: "https://github.com/astral-sh/python-build-standalone/releases/download/20261110/cpython-3.14.10%2B20261110-x86_64-unknown-linux-gnu-install_only_stripped.tar.gz",
    sha256: "c2ff2e97422f5d3896a16e0317fd212ba977bbf2688d93a6f7a03d817506bd30",
};

/// Spindle's per-user data folder: `%LOCALAPPDATA%\Spindle` on Windows,
/// `$XDG_DATA_HOME/spindle` or `~/.local/share/spindle` elsewhere. `SPINDLE_DATA_DIR` overrides
/// it (tests use this).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SPINDLE_DATA_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join("Spindle");
        }
    }
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("spindle");
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".local/share/spindle"))
        .unwrap_or_else(|| std::env::temp_dir().join("spindle"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Runtime {
    /// The folder the sandbox mounts read-only.
    pub home: PathBuf,
    pub python: PathBuf,
    pub version: String,
}

pub fn folder() -> PathBuf {
    data_dir()
        .join("runtime")
        .join(format!("cpython-{}", PIN.version))
}

fn python_in(home: &Path) -> PathBuf {
    if cfg!(windows) {
        home.join("python.exe")
    } else {
        home.join("bin").join("python3")
    }
}

/// The installed runtime, if set up. `SPINDLE_PYTHON` points at another interpreter for
/// development and tests; it is still only ever run inside the sandbox.
pub fn installed() -> Option<Runtime> {
    if let Some(python) = std::env::var_os("SPINDLE_PYTHON") {
        let python = PathBuf::from(python);
        let home = python.parent()?.parent()?.to_owned();
        return Some(Runtime {
            home,
            python,
            version: "development".into(),
        });
    }
    let home = folder().join("python");
    let python = python_in(&home);
    (python.is_file() && home.join(".spindle-verified").is_file()).then(|| Runtime {
        home,
        python,
        version: PIN.version.into(),
    })
}

/// Spindle's helper modules for sandboxed scripts (`spindle_docs` writes .docx and .xlsx with
/// the standard library). Written into the data folder and mounted read-only on `PYTHONPATH`.
pub const DOCS_MODULE: &str = include_str!("pylib/spindle_docs.py");

pub fn library() -> Result<PathBuf> {
    let dir = data_dir().join("pylib");
    let file = dir.join("spindle_docs.py");
    if std::fs::read_to_string(&file).ok().as_deref() != Some(DOCS_MODULE) {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&file, DOCS_MODULE)?;
    }
    Ok(dir)
}

fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The system `tar`, by absolute path so nothing on PATH can stand in for it.
pub(crate) fn tar() -> Result<PathBuf> {
    let candidates: Vec<PathBuf> = if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        vec![PathBuf::from(root).join("System32").join("tar.exe")]
    } else {
        vec!["/usr/bin/tar".into(), "/bin/tar".into()]
    };
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .context("tar was not found; it ships with Windows 10 1803+ and every Linux distribution")
}

/// Check and unpack an archive into the runtime folder. Separate from the download so the hash
/// check can be tested without the network.
pub fn unpack(archive: &[u8], expected: &str, dest: &Path) -> Result<Runtime> {
    let actual = sha256_hex(archive);
    ensure!(
        actual == expected,
        "The download doesn't match the pinned hash (got {actual}); nothing was installed"
    );
    let parent = dest.parent().context("No parent folder")?;
    std::fs::create_dir_all(parent)?;
    let staging = tempfile::tempdir_in(parent)?;
    let file = staging.path().join("python.tar.gz");
    std::fs::write(&file, archive)?;
    let out = staging.path().join("unpacked");
    std::fs::create_dir(&out)?;
    let status = std::process::Command::new(tar()?)
        .arg("-xzf")
        .arg(&file)
        .arg("-C")
        .arg(&out)
        .status()
        .context("Couldn't run tar")?;
    ensure!(status.success(), "Unpacking the runtime failed");
    let home = out.join("python");
    ensure!(
        python_in(&home).is_file(),
        "The archive doesn't contain the expected Python"
    );
    std::fs::write(home.join(".spindle-verified"), format!("{expected}\n"))?;
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::create_dir_all(dest)?;
    std::fs::rename(&home, dest.join("python"))?;
    let home = dest.join("python");
    Ok(Runtime {
        python: python_in(&home),
        home,
        version: PIN.version.into(),
    })
}

/// Download and install the pinned runtime. Only call this when the user asked for it.
pub async fn install() -> Result<Runtime> {
    let client = reqwest::Client::builder()
        .user_agent("Spindle")
        .timeout(std::time::Duration::from_secs(600))
        .build()?;
    let response = client.get(PIN.url).send().await?.error_for_status()?;
    if let Some(len) = response.content_length() {
        if len > 200 * 1024 * 1024 {
            bail!("The runtime download is unexpectedly large ({len} bytes)");
        }
    }
    let bytes = response.bytes().await?;
    let dest = folder();
    tokio::task::spawn_blocking(move || unpack(&bytes, PIN.sha256, &dest)).await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrong_hash_installs_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("runtime/cpython-x");
        let err = unpack(b"not the archive", PIN.sha256, &dest).unwrap_err();
        assert!(err.to_string().contains("doesn't match"));
        assert!(!dest.exists());
        assert_eq!(PIN.sha256.len(), 64);
        assert!(PIN.url.contains(PIN.release) && PIN.url.contains(PIN.version));
    }
}
