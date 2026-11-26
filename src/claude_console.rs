use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

pub const API: &str = "https://api.anthropic.com/v1";

#[derive(Clone, Serialize, Deserialize)]
pub struct Login {
    program: PathBuf,
    vault: String,
}

impl Login {
    pub fn new(program: &str, vault: String) -> Result<Self> {
        let program = Path::new(program.trim());
        let name = if cfg!(windows) { "ant.exe" } else { "ant" };
        ensure!(
            program.is_absolute()
                && program
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(|s| s.eq_ignore_ascii_case(name)),
            "Choose the official {name} executable by its full path"
        );
        let program = program
            .canonicalize()
            .context("Locate the installed Anthropic CLI")?;
        ensure!(
            program.is_file(),
            "Choose an installed Anthropic CLI executable"
        );
        #[cfg(windows)]
        ensure!(
            !crate::projects::display_path(&program).starts_with("\\\\"),
            "Use a local copy of the Anthropic CLI"
        );
        Ok(Self { program, vault })
    }

    async fn auth(&self, action: &str, flags: &[&str], seconds: u64) -> Result<Vec<u8>> {
        ensure!(
            !self.vault.is_empty()
                && self
                    .vault
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid Claude Console account reference"
        );
        let folder = crate::runtime::data_dir()
            .join("accounts")
            .join("claude")
            .join(&self.vault);
        std::fs::create_dir_all(&folder)?;
        let mut command = tokio::process::Command::new(&self.program);
        // Discard ambient credentials and endpoint overrides so this connection uses its own login.
        for (name, _) in std::env::vars_os() {
            if name
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("ANTHROPIC_")
            {
                command.env_remove(name);
            }
        }
        command
            .env("ANTHROPIC_CONFIG_DIR", &folder)
            .current_dir(&folder)
            .args(["auth", action, "--profile", "spindle"])
            .args(flags)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        let output = tokio::time::timeout(Duration::from_secs(seconds), command.output())
            .await
            .context("Claude Console operation timed out")?
            .context("Could not open the Anthropic CLI")?;
        ensure!(
            output.status.success(),
            "Anthropic CLI could not {action}. Check that it is up to date and that your Console workspace permits this operation."
        );
        Ok(output.stdout)
    }

    pub async fn sign_in(&self) -> Result<()> {
        self.auth("login", &[], 240).await?;
        Ok(())
    }

    pub async fn access(&self) -> Result<String> {
        static REFRESH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _guard = REFRESH.lock().await;
        let output = self
            .auth("print-credentials", &["--access-token"], 30)
            .await?;
        let token = std::str::from_utf8(&output)
            .context("Invalid Claude Console credential")?
            .trim();
        ensure!(
            token.starts_with("sk-ant-oat")
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "Sign in to the Claude Console connection to use an OAuth workspace token"
        );
        Ok(token.to_owned())
    }

    pub async fn sign_out(&self) -> Result<()> {
        self.auth("logout", &[], 30).await?;
        Ok(())
    }
}
