//! The on-device Java APK profile. The engine supplies only explicitly approved project inputs.

use crate::tools::Call;
use anyhow::{Result, bail, ensure};
use std::path::{Component, Path};

const MAX_INPUTS: usize = 64;
const MAX_SOURCE_BYTES: usize = 128 * 1024;

fn java_identifier(part: &str) -> bool {
    !part.is_empty()
        && part.as_bytes()[0].is_ascii_alphabetic()
        && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub fn validate(root: &Path, call: &Call) -> Result<()> {
    let Call::BuildApk {
        package,
        label,
        activity,
        sources,
    } = call
    else {
        bail!("Expected an Android APK build");
    };
    ensure!(
        package.len() <= 60 && package.contains('.') && package.split('.').all(java_identifier),
        "Use an Android package ID such as com.example.counter"
    );
    ensure!(
        activity.len() <= 120
            && activity.starts_with(&format!("{package}."))
            && activity.split('.').all(java_identifier),
        "Use the launcher Activity's full class name within {package}"
    );
    ensure!(
        !label.trim().is_empty()
            && label.chars().count() <= 80
            && !label.chars().any(char::is_control),
        "Use a single-line app label of 1 to 80 characters"
    );
    ensure!(
        (1..=MAX_INPUTS).contains(&sources.len()),
        "List 1 to {MAX_INPUTS} Java source files"
    );
    let mut total = 0;
    let mut seen = std::collections::BTreeSet::new();
    for s in sources {
        let source = source_path(root, s)?;
        ensure!(seen.insert(source.clone()), "Duplicate Java source {s}");
        total += std::fs::metadata(source)?.len();
        ensure!(
            total <= MAX_SOURCE_BYTES as u64,
            "Java sources together must be at most 128 KiB"
        );
    }
    Ok(())
}

fn source_path(root: &Path, raw: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(raw);
    ensure!(
        !raw.contains(['\\', ':'])
            && raw.len() <= 240
            && path
                .extension()
                .is_some_and(|extension| extension == "java")
            && path.components().all(|component| match component {
                Component::Normal(part) => part.to_str().is_some_and(|name| java_identifier(
                    name.strip_suffix(".java").unwrap_or(name)
                )),
                _ => false,
            }),
        "Use a relative .java path with Java file and directory names"
    );
    let (resolved, inside) = crate::tools::resolve(root, raw)?;
    ensure!(
        inside && resolved.is_file(),
        "{raw} must be an existing Java file inside this conversation's folder"
    );
    Ok(resolved)
}

#[cfg(target_os = "android")]
pub fn build(
    call: &Call,
    root: &Path,
    folder: &Path,
    id: u64,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<String> {
    use anyhow::Context;
    use base64::Engine as _;
    use std::{
        io::{Read, Write},
        sync::atomic::Ordering,
    };
    validate(root, call)?;
    let Call::BuildApk {
        package,
        activity,
        label,
        sources,
    } = call
    else {
        unreachable!()
    };
    let mut files = std::collections::BTreeMap::new();
    let mut limit = MAX_SOURCE_BYTES;
    for name in sources {
        let path = source_path(root, name)?;
        let mut contents = String::new();
        std::fs::File::open(path)?
            .take((limit + 1) as u64)
            .read_to_string(&mut contents)
            .with_context(|| format!("Read Java source {name} as UTF-8"))?;
        ensure!(
            contents.len() <= limit,
            "Java sources together must be at most 128 KiB"
        );
        limit -= contents.len();
        files.insert(name, contents);
    }
    ensure!(
        !cancelled.load(Ordering::Relaxed),
        "APK build was cancelled"
    );
    let request = serde_json::json!({
        "package": package,
        "activity": activity,
        "label": label,
        "sources": files,
    });
    let result: serde_json::Value =
        serde_json::from_str(&crate::android::build_apk(&request.to_string())?)?;
    if let Some(error) = result["error"].as_str() {
        bail!("{error}");
    }
    ensure!(
        !cancelled.load(Ordering::Relaxed),
        "APK build was cancelled"
    );
    let encoded = result["apk"]
        .as_str()
        .context("The builder returned no APK")?;
    ensure!(encoded.len() <= 768 * 1024, "APK exceeds the output limit");
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    let name = format!("{package}-debug-{id}.apk");
    let mut temp = tempfile::NamedTempFile::new_in(folder)?;
    temp.write_all(&bytes)?;
    temp.persist_noclobber(folder.join(&name))?;
    Ok(name)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apk_inputs_must_be_existing_java_files_inside_the_project() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("project");
        std::fs::create_dir(&root)?;
        std::fs::write(root.join("MainActivity.java"), "class MainActivity {}")?;
        std::fs::write(dir.path().join("Outside.java"), "class Outside {}")?;
        let call = |path: &str| Call::BuildApk {
            package: "com.example.fixture".into(),
            activity: "com.example.fixture.MainActivity".into(),
            label: "Fixture".into(),
            sources: vec![path.into()],
        };
        assert!(validate(&root, &call("MainActivity.java")).is_ok());
        assert!(validate(&root, &call("../Outside.java")).is_err());
        assert!(validate(&root, &call("build.gradle")).is_err());
        Ok(())
    }
}
