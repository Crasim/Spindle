//! Python packages for the sandbox, kept in Spindle's own folder.
//!
//! Packages are installed with Spindle's Python and pip into `<data>/packages` (never into the
//! runtime itself or any other Python), only as prebuilt wheels so no package build script runs,
//! and only after the user approves the exact names. Installing is the one step that reaches the
//! network (pypi.org); it runs outside the sandbox because the sandbox has no network. The
//! sandbox then sees the folder read-only on `PYTHONPATH`.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub fn folder(data: &Path) -> PathBuf {
    data.join("packages")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub bytes: u64,
    pub summary: String,
}

/// Packages people commonly need for documents and data, offered in Settings.
pub const SUGGESTED: &[(&str, &str)] = &[
    ("python-docx", "Richer Word documents"),
    ("openpyxl", "Excel workbooks with formatting and formulas"),
    ("python-pptx", "PowerPoint decks"),
    ("reportlab", "PDF files"),
    ("pandas", "Tables and data analysis"),
    ("matplotlib", "Charts as images"),
];

/// A package requirement the user can approve: a name, optionally `==version`.
pub fn valid(spec: &str) -> bool {
    let (name, version) = spec.split_once("==").unwrap_or((spec, ""));
    !name.is_empty()
        && name.len() <= 80
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with(['-', '.'])
        && version.len() <= 40
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.')
}

fn header(metadata: &str, key: &str) -> String {
    metadata
        .lines()
        .take_while(|l| !l.is_empty())
        .find_map(|l| l.strip_prefix(&format!("{key}: ")))
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// Installed packages, read from their metadata files (nothing is run).
pub fn list(data: &Path) -> Vec<Package> {
    let dir = folder(data);
    let Ok(read) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut out: Vec<Package> = read
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".dist-info"))
        .filter_map(|e| {
            let metadata = std::fs::read_to_string(e.path().join("METADATA")).ok()?;
            let bytes = std::fs::read_to_string(e.path().join("RECORD"))
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.rsplit(',').next()?.trim().parse::<u64>().ok())
                .sum();
            Some(Package {
                name: header(&metadata, "Name"),
                version: header(&metadata, "Version"),
                bytes,
                summary: header(&metadata, "Summary"),
            })
        })
        .filter(|p| !p.name.is_empty())
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

fn normal(name: &str) -> String {
    name.to_lowercase().replace(['-', '.'], "_")
}

/// Install packages (and what they depend on) from pypi.org as wheels. Only call after the user
/// approved these names. Returns pip's summary line.
pub fn install(python: &Path, data: &Path, specs: &[String]) -> Result<String> {
    ensure!(
        !specs.is_empty() && specs.len() <= 10,
        "Install 1 to 10 packages at a time"
    );
    for s in specs {
        ensure!(valid(s), "{s} isn't a valid package name");
    }
    let target = folder(data);
    std::fs::create_dir_all(&target)?;
    let mut cmd = std::process::Command::new(python);
    cmd.args(["-I", "-m", "pip", "install", "--target"])
        .arg(&target)
        .args([
            "--only-binary=:all:",
            "--no-input",
            "--disable-pip-version-check",
            "--no-warn-script-location",
            "--no-compile",
            "--upgrade",
            "--index-url",
            "https://pypi.org/simple",
        ])
        .args(specs)
        .env_remove("PYTHONPATH")
        .env_remove("PIP_INDEX_URL")
        .env_remove("PIP_EXTRA_INDEX_URL")
        .env(
            "PIP_CONFIG_FILE",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().context("Couldn't start pip")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    ensure!(
        out.status.success(),
        "pip couldn't install {}: {}",
        specs.join(", "),
        text.lines()
            .filter(|l| l.contains("ERROR"))
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(600)
            .collect::<String>()
    );
    Ok(text
        .lines()
        .rev()
        .find(|l| l.starts_with("Successfully installed"))
        .unwrap_or("Installed")
        .to_owned())
}

/// Remove an installed package's files, using its install record.
pub fn remove(data: &Path, name: &str) -> Result<()> {
    let dir = folder(data);
    let info = std::fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".dist-info"))
                && std::fs::read_to_string(p.join("METADATA"))
                    .is_ok_and(|m| normal(&header(&m, "Name")) == normal(name))
        })
        .with_context(|| format!("{name} isn't installed"))?;
    let record = std::fs::read_to_string(info.join("RECORD")).unwrap_or_default();
    let canonical = dir.canonicalize()?;
    for line in record.lines() {
        let Some(rel) = line.split(',').next().filter(|r| !r.is_empty()) else {
            continue;
        };
        let path = dir.join(rel);
        // Only files inside the packages folder are ever removed.
        if let Ok(real) = path.canonicalize()
            && real.starts_with(&canonical)
            && real.is_file()
        {
            std::fs::remove_file(real)?;
        }
    }
    // Remove folders the package left empty, deepest first.
    let mut dirs: Vec<PathBuf> = record
        .lines()
        .filter_map(|l| l.split(',').next())
        .filter_map(|r| Path::new(r).parent().map(|p| dir.join(p)))
        .collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    dirs.dedup();
    for d in dirs {
        if d != dir && d.starts_with(&dir) {
            let _ = std::fs::remove_dir(&d);
        }
    }
    if info.exists() {
        std::fs::remove_dir_all(&info)?;
    }
    Ok(())
}

pub fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{} KB", b >> 10),
        b => format!("{b} bytes"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        assert!(valid("python-docx") && valid("openpyxl==3.1.5") && valid("zope.interface"));
        assert!(
            !valid("--index-url=http://evil")
                && !valid("pkg; rm -rf /")
                && !valid("a==1;b")
                && !valid("")
        );
        assert!(!valid("./local") && !valid("git+https://x"));
    }

    /// Install and remove a small pure-Python package with this machine's Python, when pypi.org
    /// is reachable; skipped otherwise.
    #[test]
    fn packages_install_list_and_remove() {
        let Some(python) = ["/usr/local/bin/python3", "/usr/bin/python3"]
            .into_iter()
            .find(|p| Path::new(p).is_file())
        else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        match install(Path::new(python), tmp.path(), &["six".into()]) {
            Ok(summary) => assert!(summary.contains("six"), "{summary}"),
            Err(e) => {
                eprintln!("skipped: {e:#}");
                return;
            }
        }
        let found = list(tmp.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "six");
        assert!(found[0].bytes > 1000);
        assert!(folder(tmp.path()).join("six.py").is_file());
        remove(tmp.path(), "Six").unwrap();
        assert!(list(tmp.path()).is_empty());
        assert!(!folder(tmp.path()).join("six.py").exists());
    }
}
