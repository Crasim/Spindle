//! A build or test gets a disposable directory. Published executables stay in a
//! local store outside project copies and are never used as sandbox working files.
use crate::{
    sandbox::{self, Spec},
    tools::Call,
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const MAX_BINARY: u64 = 64 * 1024 * 1024;
pub(crate) const PROJECT_OUTPUTS: &str = "artifacts/builds";
const MAX_INPUT: u64 = 8 * 1024 * 1024;

struct Bundle {
    name: &'static str,
    label: &'static str,
    url: &'static str,
    sha256: &'static str,
    bytes: u64,
    executable: &'static str,
    target: &'static str,
}

fn bundle() -> Option<Bundle> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Bundle {
            name: "zig-x86_64-windows-0.15.2.zip",
            label: "Zig 0.15.2",
            url: "https://ziglang.org/download/0.15.2",
            sha256: "3a0ed1e8799a2f8ce2a6e6290a9ff22e6906f8227865911fb7ddedc3cc14cb0c",
            bytes: 92_614_574,
            executable: "zig.exe",
            target: "x86_64-windows-gnu",
        },
        ("linux", "x86_64") => Bundle {
            name: "zig-x86_64-linux-0.15.2.tar.xz",
            label: "Zig 0.15.2",
            url: "https://ziglang.org/download/0.15.2",
            sha256: "02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239",
            bytes: 53_733_924,
            executable: "zig",
            target: "x86_64-linux-musl",
        },
        ("linux", "aarch64") => Bundle {
            name: "zig-aarch64-linux-0.15.2.tar.xz",
            label: "Zig 0.15.2",
            url: "https://ziglang.org/download/0.15.2",
            sha256: "958ed7d1e00d0ea76590d27666efbf7a932281b3d7ba0c6b01b0ff26498f667f",
            bytes: 49_471_996,
            executable: "zig",
            target: "aarch64-linux-musl",
        },
        _ => return None,
    })
}

pub fn supported() -> bool {
    bundle().is_some()
}

pub fn compiler_label() -> &'static str {
    bundle()
        .map(|bundle| bundle.label)
        .unwrap_or("C/C++ compiler")
}

impl Bundle {
    fn directory(&self) -> &str {
        self.name
            .trim_end_matches(".zip")
            .trim_end_matches(".tar.xz")
    }

    fn install_dir(&self, data: &Path) -> PathBuf {
        data.join("runtime").join(self.directory())
    }

    fn compiler(&self, home: &Path) -> PathBuf {
        self.executable
            .split('/')
            .fold(home.to_path_buf(), |path, part| path.join(part))
    }
}

pub fn compiler(data: &Path) -> Option<PathBuf> {
    let bundle = bundle()?;
    let home = bundle.install_dir(data);
    if fs::read_to_string(home.join(".spindle-verified"))
        .ok()?
        .trim()
        != bundle.sha256
    {
        return None;
    }
    let file = bundle.compiler(&home);
    file.is_file().then_some(file)
}

fn unpack(data: &Path, archive: &Path) -> Result<()> {
    let bundle = bundle().context("Native builds aren't available on this platform")?;
    let file = File::open(archive)?;
    let mut reader = file.take(bundle.bytes + 1);
    let mut digest = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0; 64 * 1024];
    let mut count = 0;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        count += read as u64;
    }
    ensure!(
        count == bundle.bytes && crate::sync::core::hex(digest.finish().as_ref()) == bundle.sha256,
        "Compiler download failed SHA-256 verification"
    );
    let dest = bundle.install_dir(data);
    let parent = dest.parent().unwrap();
    fs::create_dir_all(parent)?;
    let stage = tempfile::tempdir_in(parent)?;
    let output = std::process::Command::new(crate::runtime::tar()?)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(stage.path())
        .output()?;
    ensure!(
        output.status.success(),
        "Couldn't unpack the verified compiler"
    );
    let home = stage.path().join(bundle.directory());
    ensure!(
        bundle.compiler(&home).is_file() && home.join("lib").is_dir(),
        "Compiler archive is incomplete"
    );
    fs::write(home.join(".spindle-verified"), bundle.sha256)?;
    match fs::rename(home, dest) {
        Ok(()) => Ok(()),
        Err(_) if compiler(data).is_some() => Ok(()),
        Err(err) => Err(err).context("Couldn't install the verified compiler"),
    }
}

pub async fn install(data: PathBuf) -> Result<()> {
    let bundle = bundle().context("Native builds aren't available on this platform")?;
    if compiler(&data).is_some() {
        return Ok(());
    }
    let url = format!("{}/{}", bundle.url, bundle.name);
    let mut response = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()?
        .get(url)
        .send()
        .await?
        .error_for_status()?;
    let runtime = data.join("runtime");
    tokio::fs::create_dir_all(&runtime).await?;
    let archive = tempfile::NamedTempFile::new_in(runtime)?;
    let mut file = tokio::fs::File::from_std(archive.reopen()?);
    let mut downloaded = 0;
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = response.chunk().await? {
        downloaded += chunk.len() as u64;
        ensure!(
            downloaded <= bundle.bytes,
            "Compiler download is larger than expected"
        );
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    tokio::task::spawn_blocking(move || unpack(&data, archive.path())).await?
}

fn component(name: &str) -> bool {
    let base = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !name.is_empty()
        && name.len() <= 80
        && !name.starts_with(['.', '-'])
        && !name.ends_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && !matches!(base.as_str(), "CON" | "NUL" | "AUX" | "PRN")
        && !(base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit())
}

pub(crate) fn stage_path(raw: &str) -> Result<String> {
    let path = raw.replace('\\', "/");
    ensure!(
        path.len() <= 200 && path.split('/').all(component),
        "Use a relative source path with simple names inside this conversation's working folder"
    );
    Ok(path)
}

fn source_kind(path: &str) -> Option<&str> {
    match path.rsplit_once('.')?.1 {
        ext @ ("rs" | "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx") => Some(ext),
        _ => None,
    }
}

fn output_extension() -> &'static str {
    if cfg!(windows) { "exe" } else { "elf" }
}

pub(crate) fn valid_name(name: &str) -> bool {
    component(name)
        && name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext == output_extension())
}

fn checked_args(args: &[String]) -> Result<()> {
    ensure!(
        args.len() <= 16
            && args
                .iter()
                .all(|arg| arg.len() <= 256 && !arg.contains('\0')),
        "Pass up to 16 arguments of 256 UTF-8 bytes each"
    );
    Ok(())
}

pub fn validate(call: &Call) -> Result<()> {
    ensure!(
        supported(),
        "Native binary tools aren't available on this platform"
    );
    match call {
        Call::BuildBinary {
            path,
            inputs,
            name,
            args,
        } => {
            ensure!(
                matches!(
                    source_kind(&stage_path(path)?),
                    Some("rs" | "c" | "cc" | "cpp" | "cxx")
                ),
                "Choose a .rs, .c, .cc, .cpp or .cxx entry file"
            );
            ensure!(
                component(name) && !name.contains('.') && name.len() <= 56,
                "Use a short output name without an extension"
            );
            ensure!(
                inputs.len() <= 15,
                "Select up to 15 additional source and header files"
            );
            let mut names =
                std::collections::BTreeSet::from([stage_path(path)?.to_ascii_lowercase()]);
            for input in inputs {
                let input_path = stage_path(input)?;
                ensure!(input_path.ends_with(".rs") == path.ends_with(".rs"), "Use Rust modules with Rust, or C/C++ sources together");
                let path = input_path;
                ensure!(
                    source_kind(&path).is_some(),
                    "Choose supported source files"
                );
                ensure!(
                    names.insert(path.to_ascii_lowercase()),
                    "List each source file once"
                );
            }
            checked_args(args)
        }
        Call::TestBinary { name, args } => {
            ensure!(
                valid_name(name),
                "Choose a binary from this conversation's Build outputs"
            );
            checked_args(args)
        }
        _ => bail!("Expected a native binary tool"),
    }
}

fn regular(path: &Path, root: &Path, limit: u64) -> Result<Vec<u8>> {
    let canonical = path.canonicalize()?;
    ensure!(
        canonical == path && canonical.starts_with(root),
        "File must stay within its folder and be free of links"
    );
    let meta = fs::symlink_metadata(&canonical)?;
    ensure!(
        meta.is_file() && meta.len() <= limit,
        "Expected a regular file within the size limit"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            meta.file_attributes() & 0x400 == 0,
            "Windows reparse files aren't accepted"
        );
    }
    let mut bytes = Vec::new();
    File::open(canonical)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "File exceeded the size limit");
    Ok(bytes)
}

fn store(data: &Path, workspace: &Path, chat: u64) -> PathBuf {
    let digest = ring::digest::digest(
        &ring::digest::SHA256,
        workspace.as_os_str().as_encoded_bytes(),
    );
    let scope = crate::sync::core::hex(digest.as_ref());
    data.join("builds").join(scope).join(chat.to_string())
}

pub fn artifact(
    data: &Path,
    workspace: &Path,
    chat: u64,
    name: &str,
) -> Result<(PathBuf, Vec<u8>)> {
    ensure!(valid_name(name), "Invalid build output name");
    let dir = store(data, workspace, chat).canonicalize()?;
    let path = dir.join(name);
    let bytes = regular(&path, &dir, MAX_BINARY)?;
    Ok((path, bytes))
}

pub fn outputs(workspace: &Path, chat: u64) -> Vec<String> {
    let dir = store(&crate::runtime::data_dir(), workspace, chat);
    let Ok(entries) = fs::read_dir(dir) else {
        return vec![];
    };
    let mut names: Vec<_> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().into_string().ok()?;
            (valid_name(&name) && e.file_type().ok()?.is_file()).then_some(name)
        })
        .collect();
    names.sort();
    names
}

pub fn export(workspace: &Path, chat: u64, name: &str, destination: &Path) -> Result<()> {
    let (_, bytes) = artifact(&crate::runtime::data_dir(), workspace, chat, name)?;
    ensure!(
        is_native(&bytes),
        "The stored file is not a native executable for this device"
    );
    write_executable(destination, &bytes)
}

fn write_executable(destination: &Path, bytes: &[u8]) -> Result<()> {
    let parent = destination.parent().context("Choose an output folder")?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    file.as_file().sync_all()?;
    file.persist_noclobber(destination)
        .context("Choose an unused filename for the executable")?;
    Ok(())
}

/// Copy a passing build from this conversation's managed store to its project.
pub(crate) fn save_to_project(workspace: &Path, chat: u64, project: &Path, name: &str) -> Result<String> {
    save_to_project_in(&crate::runtime::data_dir(), workspace, chat, project, name)
}

fn save_to_project_in(data: &Path, workspace: &Path, chat: u64, project: &Path, name: &str) -> Result<String> {
    let (_, bytes) = artifact(data, workspace, chat, name).context("Choose a tested output from this conversation's Build outputs")?;
    ensure!(is_native(&bytes), "The stored file is not a native executable for this device");
    let project = crate::projects::existing(project)?.context("The conversation's project folder is unavailable")?;
    let folder = project.path.join(PROJECT_OUTPUTS);
    // Verify each parent before creating or writing anything below it.
    for dir in [project.path.join("artifacts"), folder.clone()] {
        match fs::create_dir(&dir) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error).context("Create the project build folder"),
        }
        ensure!(dir.canonicalize()? == dir && dir.is_dir(), "The project build folder must stay inside this project");
    }
    let path = folder.join(name);
    if fs::symlink_metadata(&path).is_ok() {
        ensure!(regular(&path, &folder, MAX_BINARY)? == bytes, "A different output already uses this project filename");
    } else {
        write_executable(&path, &bytes)?;
    }
    Ok(format!("{PROJECT_OUTPUTS}/{name}"))
}

pub(crate) fn is_native(bytes: &[u8]) -> bool {
    if cfg!(windows) {
        if !bytes.starts_with(b"MZ") {
            return false;
        }
        let Some(offset) = bytes
            .get(60..64)
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
        else {
            return false;
        };
        // PE signature and little-endian x64 machine value.
        bytes
            .get(offset as usize..)
            .is_some_and(|head| head.starts_with(b"PE\0\0\x64\x86"))
    } else {
        let machine = if cfg!(target_arch = "aarch64") {
            183
        } else {
            62
        };
        bytes.starts_with(b"\x7fELF\x02\x01") && bytes.get(18..20) == Some(&[machine, 0])
    }
}

pub struct Report {
    pub text: String,
    pub ok: bool,
    pub file: Option<PathBuf>,
}

fn sandbox_program(work: &Path, file: &str) -> PathBuf {
    if cfg!(windows) {
        work.join(file)
    } else {
        Path::new(sandbox::SANDBOX_WORK).join(file)
    }
}

fn spec(
    program: PathBuf,
    work: PathBuf,
    args: Vec<String>,
    readonly: Vec<PathBuf>,
    seconds: u64,
) -> Spec {
    Spec {
        program,
        work,
        args,
        readonly,
        python_path: vec![],
        timeout: Duration::from_secs(seconds),
        memory_mb: 2048,
        max_processes: 24,
    }
}

fn test(tmp: &Path, bytes: &[u8], args: &[String], cancel: &Arc<AtomicBool>) -> Result<Report> {
    ensure!(
        is_native(bytes),
        "The build did not produce a native executable for this device"
    );
    let work = tempfile::tempdir_in(tmp)?;
    let root = work.path().canonicalize()?;
    let file = format!("program.{}", output_extension());
    let path = root.join(&file);
    fs::write(&path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    let spec = spec(
        sandbox_program(&root, &file),
        root,
        args.to_vec(),
        vec![],
        20,
    );
    let result = sandbox::run_private(&spec, cancel)?;
    Ok(Report {
        text: format!("Test run\n{}", result.report(&spec.work, 3600)),
        ok: result.ok(),
        file: None,
    })
}

pub fn execute(
    data: &Path,
    workspace: &Path,
    chat: u64,
    work: &Path,
    call: &Call,
    cancel: &Arc<AtomicBool>,
) -> Result<Report> {
    execute_in(data, workspace, chat, work, call, cancel, None)
}

pub(crate) fn execute_in(data: &Path, workspace: &Path, chat: u64, work: &Path, call: &Call,
    cancel: &Arc<AtomicBool>, output_dir: Option<&Path>) -> Result<Report> {
    validate(call)?;
    let tmp = data.join("native-jobs");
    fs::create_dir_all(&tmp)?;
    let Call::BuildBinary {
        path,
        inputs,
        name,
        args,
    } = call
    else {
        let Call::TestBinary { name, args } = call else {
            unreachable!()
        };
        let (_, bytes) = artifact(data, workspace, chat, name)?;
        return test(&tmp, &bytes, args, cancel);
    };
    let bundle = bundle().unwrap();
    let stage = tempfile::tempdir_in(&tmp)?;
    let root = stage.path().canonicalize()?;
    let source = work.canonicalize()?;
    let mut remaining = MAX_INPUT;
    let mut units = Vec::new();
    let mut cpp = false;
    for raw in std::iter::once(path).chain(inputs) {
        let path = stage_path(raw)?;
        let relative: PathBuf = path.split('/').collect();
        let bytes = regular(&source.join(&relative), &source, remaining)
            .with_context(|| format!("Couldn't stage {path}"))?;
        remaining -= bytes.len() as u64;
        let staged = root.join("src").join(relative);
        fs::create_dir_all(staged.parent().unwrap())?;
        fs::write(staged, bytes)?;
        match source_kind(&path) {
            Some("c") => units.push(("c", format!("src/{path}"))),
            Some("cc" | "cpp" | "cxx") => {
                cpp = true;
                units.push(("c++", format!("src/{path}")));
            }
            _ => {}
        }
    }
    let file = format!("program.{}", output_extension());
    let rust = path.ends_with(".rs");
    let rust_stage;
    let (compiler, readonly, rust_flags) = if rust {
        rust_stage = tempfile::tempdir_in(&tmp)?;
        crate::rust_build::prepare(rust_stage.path(), path, &file)?
    } else {
        (compiler(data).context("Install Zig in Settings › Sandbox for C/C++")?.canonicalize()?,
         vec![bundle.install_dir(data).canonicalize()?], vec![])
    };
    let mut command = if rust { rust_flags } else if cfg!(windows) {
        vec![
            if cpp { "c++".into() } else { "cc".into() },
            "-target".into(),
            bundle.target.into(),
            "-fintegrated-cc1".into(),
            "-O1".into(),
            "-o".into(),
            file.clone(),
        ]
    } else {
        let mut flags = vec![
            "build-exe".into(),
            "-target".into(),
            bundle.target.into(),
            "-O".into(),
            "ReleaseFast".into(),
            "-j4".into(),
            "-lc".into(),
            "-static".into(),
            format!("-femit-bin={file}"),
        ];
        if cpp {
            flags.push("-lc++".into());
        }
        flags
    };
    for (language, path) in units {
        if cfg!(windows) {
            command.extend(["-x".into(), language.into()]);
        }
        command.push(path);
    }
    let mut build = spec(compiler, root, command, readonly, 180);
    if cfg!(windows) {
        // The MinGW runtime sources compile in parallel. The smoke test keeps tighter limits.
        build.memory_mb = 4096;
        build.max_processes = 64;
    }
    let lock = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(tmp.join("compiler.lock"))?;
    crate::state::try_lock(&lock).context("The native compiler is busy in another workspace")?;
    let result = sandbox::run_private(&build, cancel)?;
    drop(lock);
    let mut report = format!("Compilation\n{}", result.report(&build.work, 3600));
    if !result.ok() || cancel.load(Ordering::Relaxed) {
        return Ok(Report {
            text: report,
            ok: false,
            file: None,
        });
    }
    let bytes = regular(&build.work.join(file), &build.work, MAX_BINARY)?;
    let tested = test(&tmp, &bytes, args, cancel)?;
    report.push_str(&format!("\n{}", tested.text));
    let ok = tested.ok && !cancel.load(Ordering::Relaxed);
    let mut saved = None;
    if ok {
        let dir = output_dir.map(Path::to_path_buf).unwrap_or_else(||store(data, workspace, chat));
        fs::create_dir_all(&dir)?;
        let tag = crate::sync::core::hex(&crate::vault::random::<6>()?);
        let name = format!("{name}-{tag}.{}", output_extension());
        let mut output = tempfile::NamedTempFile::new_in(&dir)?;
        output.write_all(&bytes)?;
        output.as_file().sync_all()?;
        output.persist_noclobber(dir.join(&name))?;
        saved = Some(dir.join(&name));
        report.push_str(&format!(
            "\nSaved {name} to this chat's Build outputs. The user can download it from there."
        ));
    }
    Ok(Report { text: report, ok, file: saved })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_build_can_be_saved_only_from_its_chat_without_replacing_project_files() -> Result<()> {
        if !supported() { return Ok(()); }
        let temp = tempfile::tempdir()?;
        let workspace = temp.path().canonicalize()?;
        let project = workspace.join("project");
        std::fs::create_dir(&project)?;
        crate::projects::create(&project, "Build fixture")?;
        let data = workspace.join("data");
        let chat = 21;
        let name = format!("fixture.{}", output_extension());
        let dir = store(&data, &workspace, chat);
        std::fs::create_dir_all(&dir)?;
        let mut bytes = vec![0; 128];
        if cfg!(windows) {
            bytes[..2].copy_from_slice(b"MZ");
            bytes[60] = 64;
            bytes[64..70].copy_from_slice(b"PE\0\0\x64\x86");
        } else {
            bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
            let machine = if cfg!(target_arch = "aarch64") { 183u16 } else { 62u16 };
            bytes[18..20].copy_from_slice(&machine.to_le_bytes());
        }
        std::fs::write(dir.join(&name), &bytes)?;
        assert!(save_to_project_in(&data, &workspace, chat + 1, &project, &name).is_err());
        let relative = save_to_project_in(&data, &workspace, chat, &project, &name)?;
        assert_eq!(relative, format!("{PROJECT_OUTPUTS}/{name}"));
        assert_eq!(std::fs::read(project.join(&relative))?, bytes);
        assert_eq!(save_to_project_in(&data, &workspace, chat, &project, &name)?, relative);
        std::fs::write(project.join(&relative), b"keep this")?;
        assert!(save_to_project_in(&data, &workspace, chat, &project, &name).is_err());
        assert_eq!(std::fs::read(project.join(&relative))?, b"keep this");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "invoked only in the isolated native pipeline fixture"]
    fn checks_windows_temp_folder() {
        assert_eq!(std::env::var("SPINDLE_SANDBOX").unwrap(), "1");
        let dir = std::env::temp_dir();
        assert!(dir.is_dir(), "{dir:?}");
        fs::write(dir.join("probe.txt"), "writable temporary directory").unwrap();
        let address = fs::read_to_string("local-listener.txt")
            .unwrap()
            .parse()
            .unwrap();
        assert!(std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_err());
    }

    #[test]
    fn native_tools_reject_paths_and_require_one_time_approval() {
        if !supported() {
            return;
        }
        for path in [
            "../main.c",
            "/main.c",
            "C:/main.c",
            "lib/../main.c",
            "lib/NUL.h",
            "-evil.c",
            "file.c:stream",
        ] {
            assert!(stage_path(path).is_err(), "{path}");
        }
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        let call = Call::BuildBinary {
            path: "main.c".into(),
            name: "calculator".into(),
            inputs: vec![],
            args: vec![],
        };
        assert!(crate::tools::classify(&path, &call, false).is_err());
        let (risk, target) = crate::tools::classify(&path, &call, true).unwrap();
        use crate::policy::{Decision, Level, Request, Trust};
        assert_eq!(crate::policy::levels(risk, Trust::Trusted), &[Level::Once]);
        assert_eq!(
            crate::policy::decide(
                &[],
                &Request {
                    risk,
                    target: &target,
                    chat: 1,
                    project: Some(&path),
                    trust: Trust::Trusted,
                }
            ),
            Decision::Ask
        );
    }

    /// Uses the official archive and the OS sandbox, with a disposable state directory.
    #[test]
    #[ignore = "requires the pinned compiler archive and a working OS sandbox"]
    fn native_pipeline_compiles_runs_and_denies_access_to_the_project() {
        let base = std::env::var_os("SPINDLE_TEST_NATIVE_DIR").map(PathBuf::from);
        let tmp = match base {
            Some(base) => tempfile::tempdir_in(base),
            None => tempfile::tempdir(),
        }
        .unwrap();
        let path = tmp.path().canonicalize().unwrap();
        let data = path.join("data");
        #[cfg(windows)]
        {
            let dir = path.join("windows-temp-check");
            fs::create_dir(&dir).unwrap();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            fs::write(
                dir.join("local-listener.txt"),
                listener.local_addr().unwrap().to_string(),
            )
            .unwrap();
            let probe = dir.join("probe.exe");
            fs::copy(std::env::current_exe().unwrap(), &probe).unwrap();
            let outcome = sandbox::run_private(
                &spec(
                    probe,
                    dir,
                    vec![
                        "checks_windows_temp_folder".into(),
                        "--ignored".into(),
                        "--nocapture".into(),
                    ],
                    vec![],
                    10,
                ),
                &Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
            assert!(outcome.ok(), "{} {}", outcome.stdout, outcome.stderr);
        }
        let archive = std::env::var_os("SPINDLE_TEST_ZIG_ARCHIVE")
            .expect("Path to official compiler archive");
        let invalid = path.join("unverified.zip");
        fs::write(&invalid, b"unverified").unwrap();
        assert!(unpack(&data, &invalid).is_err());
        unpack(&data, &PathBuf::from(archive)).unwrap();
        let project = path.join("project");
        fs::create_dir(&project).unwrap();
        fs::write(project.join("private.txt"), "outside the sandbox").unwrap();
        fs::write(
            project.join("main.c"),
            r#"
            #include <stdio.h>
            #include "lib/value.h"
            int main(int argc, char **argv) {
                if (argc != 2) return 3;
                FILE *outside = fopen(argv[1], "rb");
                if (outside) { fclose(outside); return 4; }
                if (value() != 7) return 5;
                puts("Spindle native sandbox OK");
                return 0;
            }
        "#,
        )
        .unwrap();
        fs::create_dir(project.join("lib")).unwrap();
        fs::write(project.join("lib/value.h"), "int value(void);\n").unwrap();
        fs::write(
            project.join("lib/value.cpp"),
            "extern \"C\" int value() { constexpr int constant = 7; return constant; }\n",
        )
        .unwrap();
        let args = vec![crate::projects::display_path(&project.join("private.txt"))];
        let build = Call::BuildBinary {
            path: "main.c".into(),
            name: "smoke".into(),
            inputs: vec!["lib/value.h".into(), "lib/value.cpp".into()],
            args: args.clone(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let result = execute(&data, &project, 1, &project, &build, &cancel).unwrap();
        assert!(result.ok, "{}", result.text);
        assert!(
            result.text.contains("Spindle native sandbox OK"),
            "{}",
            result.text
        );
        let dir = store(&data, &project, 1);
        let name = fs::read_dir(dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name()
            .into_string()
            .unwrap();
        assert!(artifact(&data, &project, 2, &name).is_err());
        let rerun = Call::TestBinary { name, args };
        assert!(
            execute(&data, &project, 1, &project, &rerun, &cancel)
                .unwrap()
                .ok
        );
        assert_eq!(fs::read_dir(project).unwrap().count(), 3);
    }
}
