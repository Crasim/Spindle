fn main() {
    // When this copy was built, shown with the version (Settings, at the bottom), so an
    // old build is easy to spot.
    let built = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=SPINDLE_BUILT={built}");
    // Stamp again whenever the source changes (the icon line below would otherwise limit
    // reruns to the icon).
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SPINDLE_RELEASE_REPO");
    println!("cargo:rerun-if-env-changed=GITHUB_REPOSITORY");
    println!("cargo:rerun-if-changed=.git/config");
    println!("cargo:rustc-env=SPINDLE_RELEASE_REPO={}", release_repository().unwrap_or_default());
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/spindle.ico");
        winresource::WindowsResource::new()
            .set_icon("assets/spindle.ico")
            .set("ProductName", "Spindle")
            .set("FileDescription", "Spindle desktop workspace")
            .compile()
            .expect("embed Spindle Windows icon");
    }
}

fn release_repository() -> Option<String> {
    let configured = std::env::var("SPINDLE_RELEASE_REPO").ok()
        .or_else(|| std::env::var("GITHUB_REPOSITORY").ok());
    let origin = || {
        let output = std::process::Command::new("git")
            .args(["config", "--get", "remote.origin.url"])
            .output().ok()?;
        let value = std::str::from_utf8(&output.stdout).ok()?.trim();
        let path = value.strip_prefix("https://github.com/")
            .or_else(|| value.strip_prefix("git@github.com:"))?;
        Some(path.strip_suffix(".git").unwrap_or(path).to_owned())
    };
    let repo = configured.or_else(origin)?;
    let valid = |part: &str| !part.is_empty() && part != "." && part != ".."
        && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    let (owner, name) = repo.split_once('/')?;
    (valid(owner) && valid(name)).then_some(repo)
}
