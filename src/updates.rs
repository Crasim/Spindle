//! Published GitHub releases and verified, staged updates.
use anyhow::{Context, Result, ensure};
use ring::digest::{Context as Hash, SHA256};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(not(target_os = "android"))]
const REPOSITORY: &str = env!("SPINDLE_RELEASE_REPO");
// This APK uses the public release channel even when built from a source archive.
#[cfg(target_os = "android")]
const REPOSITORY: &str = "Crasim/Spindle";
const MAX_BINARY: u64 = 128 * 1024 * 1024;

pub fn releases_url() -> String {
    format!("https://github.com/{REPOSITORY}/releases")
}

#[derive(Deserialize)]
struct Release {
    draft: bool,
    prerelease: bool,
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    digest: Option<String>,
    size: u64,
    browser_download_url: String,
}

#[derive(Clone)]
pub struct Update {
    pub version: Version,
    pub notes_url: String,
    url: String,
    record: Install,
}

#[derive(Clone, Deserialize, Serialize)]
struct Install {
    version: String,
    digest: String,
    size: u64,
}

fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo package version")
}

fn newer(version: &Version, installed: &Version) -> bool {
    version.build.is_empty() && version.cmp_precedence(installed).is_gt()
}

fn asset_name() -> String {
    if cfg!(target_os = "android") {
        return "Spindle.apk".into();
    }
    let extension = if cfg!(windows) { ".exe" } else { "" };
    format!(
        "spindle-{}-{}{extension}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn select(releases: Vec<Release>, installed: &Version, filename: &str) -> Option<Update> {
    let releases_url = releases_url();
    releases
        .into_iter()
        .filter_map(|release| {
            if release.draft || (release.prerelease && installed.pre.is_empty()) {
                return None;
            }
            let version = Version::parse(release.tag_name.strip_prefix('v')?).ok()?;
            if !newer(&version, installed) || (!version.pre.is_empty() && installed.pre.is_empty())
            {
                return None;
            }
            let url = format!("{releases_url}/download/{}/{filename}", release.tag_name);
            let asset = release.assets.into_iter().find(|asset| {
                asset.name == filename
                    && asset.browser_download_url == url
                    && (1..=MAX_BINARY).contains(&asset.size)
            })?;
            let digest = asset.digest.as_deref()?.strip_prefix("sha256:")?;
            if !valid_digest(digest) {
                return None;
            }
            Some(Update {
                record: Install {
                    version: version.to_string(),
                    digest: digest.into(),
                    size: asset.size,
                },
                version,
                notes_url: format!("{releases_url}/tag/{}", release.tag_name),
                url,
            })
        })
        .max_by(|a, b| a.version.cmp_precedence(&b.version))
}

fn client(timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("Spindle/", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            if url.scheme() == "https"
                && matches!(
                    url.host_str(),
                    Some(
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                )
                && attempt.previous().len() < 4
            {
                attempt.follow()
            } else {
                attempt.error("Unexpected update download redirect")
            }
        }))
        .build()?)
}

pub async fn check() -> Result<Option<Update>> {
    ensure!(!REPOSITORY.is_empty(), "This build has no GitHub release repository configured");
    let mut response = client(Duration::from_secs(15))?
        .get(format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=50"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?;
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            data.len() + chunk.len() <= 4 * 1024 * 1024,
            "GitHub release list is too large"
        );
        data.extend_from_slice(&chunk);
    }
    Ok(select(
        serde_json::from_slice(&data)?,
        &current(),
        &asset_name(),
    ))
}

pub fn releases_configured() -> bool { !REPOSITORY.is_empty() }

fn directory() -> PathBuf {
    crate::runtime::data_dir().join("updates")
}

fn update_file(root: &Path, version: &Version) -> PathBuf {
    root.join(version.to_string()).join(if cfg!(target_os = "android") {
        "Spindle.apk"
    } else if cfg!(windows) {
        "spindle.exe"
    } else {
        "spindle"
    })
}

pub async fn download(update: &Update) -> Result<()> {
    download_to(update, &directory()).await
}

async fn download_to(update: &Update, root: &Path) -> Result<()> {
    let path = update_file(root, &update.version);
    let folder = path.parent().context("Missing update folder")?;
    fs::create_dir_all(folder)?;
    let mut temporary = tempfile::NamedTempFile::new_in(folder)?;
    let mut response = client(Duration::from_secs(180))?
        .get(&update.url)
        .send()
        .await?
        .error_for_status()?;
    let mut size = 0u64;
    let mut hash = Hash::new(&SHA256);
    while let Some(chunk) = response.chunk().await? {
        size += chunk.len() as u64;
        ensure!(
            size <= update.record.size,
            "GitHub update exceeded its published size"
        );
        temporary.write_all(&chunk)?;
        hash.update(&chunk);
    }
    ensure!(
        size == update.record.size
            && crate::sync::core::hex(hash.finish().as_ref()) == update.record.digest,
        "GitHub update did not match its published SHA-256 digest"
    );
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    let mut pending = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer(&mut pending, &update.record)?;
    pending.as_file().sync_all()?;
    pending.persist(root.join("ready.json"))?;
    Ok(())
}

fn staged(root: &Path) -> Result<Option<(Version, Install)>> {
    let mut input = match fs::File::open(root.join("ready.json")) {
        Ok(input) => input.take(4096),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes)?;
    let record: Install = serde_json::from_slice(&bytes)?;
    let version = Version::parse(&record.version)?;
    if !newer(&version, &current()) {
        return Ok(None);
    }
    ensure!(
        (1..=MAX_BINARY).contains(&record.size) && valid_digest(&record.digest),
        "Invalid staged update record"
    );
    Ok(Some((version, record)))
}

pub fn staged_version() -> Option<String> {
    staged(&directory())
        .ok()
        .flatten()
        .map(|(version, _)| version.to_string())
}

fn verify(path: &Path, record: &Install) -> Result<()> {
    let file = fs::File::open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "The staged update is missing its download"
    );
    let mut input = file.take(record.size + 1);
    let mut hash = Hash::new(&SHA256);
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        hash.update(&buffer[..count]);
    }
    ensure!(
        size == record.size && crate::sync::core::hex(hash.finish().as_ref()) == record.digest,
        "The staged update failed verification. Download it again from Settings › Preferences › Updates."
    );
    Ok(())
}

fn verified_staged_update(root: &Path) -> Result<Option<(Version, PathBuf)>> {
    let Some((version, record)) = staged(root)? else {
        return Ok(None);
    };
    let path = update_file(root, &version);
    verify(&path, &record)?;
    Ok(Some((version, path)))
}

#[cfg(target_os = "android")]
pub fn open_apk() -> Result<()> {
    let (version, _) = verified_staged_update(&directory())?
        .context("No verified APK is ready. Check for updates and download it again.")?;
    crate::android::open_update_apk(&version.to_string())
}

#[cfg(any(windows, target_os = "linux"))]
pub fn verify_restart() -> Result<()> {
    verified_staged_update(&directory())?
        .context("No verified update is ready. Check for updates and download it again.")?;
    Ok(())
}

/// Leave the running executable in place. A normal launch hands over to the verified version.
#[cfg(any(windows, target_os = "linux"))]
pub fn launch_staged() -> Result<bool> {
    let Some((_, path)) = verified_staged_update(&directory())? else {
        return Ok(false);
    };
    if std::env::current_exe()?.canonicalize()? == path.canonicalize()? {
        return Ok(false);
    }
    std::process::Command::new(path).spawn()?;
    Ok(true)
}

/// Reenter the normal startup path only after the workspace lock is released.
#[cfg(any(windows, target_os = "linux"))]
pub fn restart(workspace: &Path) -> Result<()> {
    std::process::Command::new(std::env::current_exe()?)
        .current_dir(workspace)
        .spawn()
        .context("Could not reopen Spindle after saving the workspace")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_a_newer_release_for_the_current_channel_and_platform() {
        let filename = "spindle-windows-x86_64.exe";
        let fixture = |tag: &str, prerelease, digest: Option<String>| Release {
            draft: false,
            prerelease,
            tag_name: tag.into(),
            assets: vec![Asset {
                name: filename.into(),
                size: 2048,
                digest,
                browser_download_url: format!("{}/download/{tag}/{filename}", releases_url()),
            }],
        };
        let releases = [
            ("v1.0.0", false, true),
            ("v1.2.0-beta.1", true, true),
            ("v1.3.0", false, false),
            ("v0.9.0", false, true),
        ];
        for (installed, expected) in [("0.9.1", "1.0.0"), ("1.1.0-beta.1", "1.2.0-beta.1")] {
            let candidates = releases
                .iter()
                .map(|(tag, pre, hash)| {
                    fixture(
                        tag,
                        *pre,
                        hash.then(|| format!("sha256:{}", "a".repeat(64))),
                    )
                })
                .collect();
            let update = select(candidates, &Version::parse(installed).unwrap(), filename).unwrap();
            assert_eq!(update.version.to_string(), expected);
        }
    }

    #[test]
    fn verifies_the_staged_update_again_before_launch() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let record = Install {
            version: "1.0.0".into(),
            size: 4,
            digest: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".into(),
        };
        fs::write(file.path(), b"test").unwrap();
        verify(file.path(), &record).unwrap();
        fs::write(file.path(), b"bad!").unwrap();
        assert!(verify(file.path(), &record).is_err());
    }

    #[test]
    fn selects_only_published_android_apks() {
        let installed = Version::parse("0.3.0-beta.15").unwrap();
        let fixture = |tag: &str, filename: &str, origin: &str| Release {
            draft: false,
            prerelease: true,
            tag_name: tag.into(),
            assets: vec![Asset {
                name: filename.into(),
                size: 2048,
                digest: Some(format!("sha256:{}", "a".repeat(64))),
                browser_download_url: format!("{origin}/download/{tag}/{filename}"),
            }],
        };
        let origin = releases_url();
        let releases = vec![
            fixture("v0.3.0-beta.16", "Spindle.apk", &origin),
            fixture("v0.3.0-beta.17", "spindle-windows-x86_64.exe", &origin),
            fixture("v0.3.0-beta.18", "Spindle.apk", "https://example.com"),
        ];
        let update = select(releases, &installed, "Spindle.apk").unwrap();
        assert_eq!(update.version.to_string(), "0.3.0-beta.16");
    }
}
