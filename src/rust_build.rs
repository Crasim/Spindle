//! Standard-library Rust builds. Only the selected sources are writable in the compiler job.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn host() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

fn sysroot() -> Option<PathBuf> {
    let host = host()?;
    let home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|p| PathBuf::from(p).join(".rustup"))
        })?;
    let root = home.join("toolchains").join(format!("stable-{host}"));
    root.join("bin")
        .join(format!("rustc{}", std::env::consts::EXE_SUFFIX))
        .is_file()
        .then_some(root)
}

pub fn available() -> bool {
    sysroot().is_some()
}

#[cfg(windows)]
fn newest_dir(parent: &Path, child: &str) -> Option<PathBuf> {
    fs::read_dir(parent)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(child).is_dir())
        .max_by_key(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .split('.')
                .filter_map(|s| s.parse::<u32>().ok())
                .collect::<Vec<_>>()
        })
}

#[cfg(windows)]
fn copy_files(from: &Path, to: &Path, allow: impl Fn(&Path) -> bool) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        if e.file_type()?.is_file() && allow(&e.path()) {
            fs::copy(e.path(), to.join(e.file_name()))?;
        }
    }
    Ok(())
}

pub fn prepare(
    stage: &Path,
    entry: &str,
    output: &str,
) -> Result<(PathBuf, Vec<PathBuf>, Vec<String>)> {
    let root = sysroot()
        .context("Install the stable Rust toolchain for this build host")?
        .canonicalize()?;
    let mut flags = vec![
        "--edition=2021".into(),
        "--crate-name".into(),
        "spindle_program".into(),
        format!("src/{entry}"),
        "-o".into(),
        output.into(),
    ];
    #[cfg(windows)]
    let root = {
        // Never change ACLs on the user's rustup installation or SDK. The temporary
        // AppContainer gets a private read-only copy of the compiler and libraries.
        let stage = stage.canonicalize()?;
        copy_files(&root.join("bin"), &stage.join("bin"), |p| {
            p.ends_with("rustc.exe") || p.extension().is_some_and(|e| e == "dll")
        })?;
        let rel = PathBuf::from("lib/rustlib").join(host().unwrap());
        copy_files(
            &root.join(&rel).join("lib"),
            &stage.join(&rel).join("lib"),
            |p| {
                p.extension()
                    .is_some_and(|e| e == "rlib" || e == "lib" || e == "rmeta")
            },
        )?;
        let tools = stage.join(&rel).join("bin");
        fs::create_dir_all(&tools)?;
        let x86 = PathBuf::from(
            std::env::var_os("ProgramFiles(x86)").context("Windows SDK path missing")?,
        );
        let kits = newest_dir(&x86.join("Windows Kits/10/Lib"), "um/x64")
            .context("Install a Windows 10 or 11 SDK")?;
        let mut vc = None;
        for pf in ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .filter_map(std::env::var_os)
        {
            for year in fs::read_dir(PathBuf::from(pf).join("Microsoft Visual Studio"))
                .into_iter()
                .flatten()
                .flatten()
            {
                for edition in fs::read_dir(year.path()).into_iter().flatten().flatten() {
                    vc =
                        vc.or_else(|| newest_dir(&edition.path().join("VC/Tools/MSVC"), "lib/x64"));
                }
            }
        }
        let vc = vc.context("Install the Visual C++ x64 build tools")?;
        let tool_dir = vc.join("bin/Hostx64/x64");
        copy_files(&tool_dir, &tools, |p| {
            p.ends_with("link.exe") || p.extension().is_some_and(|e| e == "dll")
        })?;
        flags.push(format!(
            "-Clinker={}",
            crate::projects::display_path(&tools.join("link.exe"))
        ));
        flags.push("-Clink-arg=/DEBUG:NONE".into());
        for lib in [
            kits.join("um/x64"),
            kits.join("ucrt/x64"),
            vc.join("lib/x64"),
        ] {
            flags.push(format!("-Lnative={}", crate::projects::display_path(&lib)));
        }
        stage
    };
    #[cfg(not(windows))]
    {
        let _ = stage;
        let cc = Path::new("/usr/bin/cc")
            .canonicalize()
            .context("Install a system C linker for Rust")?;
        flags.push(format!("-Clinker={}", cc.display()));
    }
    flags.push("--sysroot".into());
    flags.push(crate::projects::display_path(&root));
    let compiler = root
        .join("bin")
        .join(format!("rustc{}", std::env::consts::EXE_SUFFIX));
    ensure!(compiler.is_file(), "Missing Rust compiler");
    Ok((compiler, vec![root], flags))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "runs the installed host compiler inside the OS sandbox"]
    fn rust_program_runs_in_a_separate_sandbox() {
        use std::sync::{Arc, atomic::AtomicBool};
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().canonicalize().unwrap();
        let project = dir.join("source");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("main.rs"),r#"mod helper; fn main(){assert!(std::fs::read(std::env::args().nth(1).unwrap()).is_err()); println!("{}",helper::answer());}"#).unwrap();
        fs::write(project.join("helper.rs"), "pub fn answer() -> u32 { 42 }").unwrap();
        let data = dir.join("data");
        let call = crate::tools::Call::BuildBinary {
            path: "main.rs".into(),
            inputs: vec!["helper.rs".into()],
            name: "answer".into(),
            args: vec![crate::projects::display_path(&project.join("helper.rs"))],
        };
        let result = crate::native::execute(
            &data,
            &project,
            1,
            &project,
            &call,
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(result.ok && result.text.contains("42"), "{}", result.text);
    }
}
