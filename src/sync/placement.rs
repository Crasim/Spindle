//! Local choices for where this workspace submits sandboxed work.
use crate::{state::State, tools::Call};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Default,
    Build,
    Python,
    Tool,
    Apk,
}

impl Kind {
    pub const OPERATIONS: [Self; 4] = [Self::Build, Self::Python, Self::Tool, Self::Apk];

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default device",
            Self::Build => "Native builds and smoke tests",
            Self::Python => "Python scripts",
            Self::Tool => "Installed tools",
            Self::Apk => "Android APK builds",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Build => "build",
            Self::Python => "python",
            Self::Tool => "tool",
            Self::Apk => "apk",
        }
    }

    pub fn supports(self, os: &str) -> bool {
        match self {
            Self::Default => matches!(os, "windows" | "linux" | "android"),
            Self::Apk => os == "android",
            _ => matches!(os, "windows" | "linux"),
        }
    }

    pub fn for_call(call: &Call) -> Option<Self> {
        match call {
            Call::BuildBinary { .. } | Call::TestBinary { .. } => Some(Self::Build),
            Call::Python { .. } | Call::Test { .. } => Some(Self::Python),
            Call::RunTool { .. } => Some(Self::Tool),
            Call::BuildApk { .. } => Some(Self::Apk),
            Call::RemoteJob { operation, .. } => match operation.as_str() {
                "build" => Some(Self::Build),
                "run" => Some(Self::Python),
                "tool" => Some(Self::Tool),
                "apk" => Some(Self::Apk),
                _ => None,
            },
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Local,
    Device(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Chat(u64),
    Project(PathBuf),
}

type Routes = BTreeMap<Kind, Target>;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Placement {
    #[serde(default)]
    chats: BTreeMap<u64, Routes>,
    #[serde(default)]
    projects: BTreeMap<PathBuf, Routes>,
}

impl Placement {
    pub fn is_empty(&self) -> bool {
        self.chats.is_empty() && self.projects.is_empty()
    }

    pub fn configured(&self, scope: &Scope, kind: Kind) -> Option<&Target> {
        match scope {
            Scope::Chat(id) => self.chats.get(id),
            Scope::Project(path) => self.projects.get(path),
        }?
        .get(&kind)
    }

    pub fn effective(&self, state: &State, scope: &Scope, kind: Kind) -> Option<&Target> {
        self.configured(scope, kind)
            .or_else(|| self.configured(scope, Kind::Default))
            .or_else(|| {
                let Scope::Chat(id) = scope else { return None };
                let project = state.thread_data(*id)?.0.project.as_ref()?;
                let routes = self.projects.get(project)?;
                routes.get(&kind).or_else(|| routes.get(&Kind::Default))
            })
    }

    pub fn forget_chat(&mut self, id: u64) {
        self.chats.remove(&id);
    }
}

pub fn set(state: &mut State, mut scope: Scope, kind: Kind, target: Option<Target>) -> Result<()> {
    match &mut scope {
        Scope::Chat(id) => ensure!(
            state.thread_data(*id).is_some(),
            "Conversation no longer exists"
        ),
        Scope::Project(path) => {
            *path = crate::projects::existing(path)?
                .context("Project no longer exists")?
                .path;
        }
    }
    if let Some(Target::Device(id)) = &target {
        let sync = state
            .space
            .as_ref()
            .context("Pair the target in Settings › Sync first")?;
        let os = sync
            .remote
            .platforms
            .get(id)
            .filter(|_| id != &sync.identity.device.id)
            .context("The paired device has not reported its platform yet")?;
        ensure!(
            kind.supports(os),
            "This device's platform does not support {}",
            kind.label().to_lowercase()
        );
    }
    match scope {
        Scope::Chat(id) => change(&mut state.placement.chats, id, kind, target),
        Scope::Project(path) => change(&mut state.placement.projects, path, kind, target),
    }
    Ok(())
}

fn change<K: Ord>(by_scope: &mut BTreeMap<K, Routes>, key: K, kind: Kind, target: Option<Target>) {
    if let Some(target) = target {
        by_scope.entry(key).or_default().insert(kind, target);
    } else if let Some(routes) = by_scope.get_mut(&key) {
        routes.remove(&kind);
        if routes.is_empty() {
            by_scope.remove(&key);
        }
    }
}

pub fn target_label(state: &State, target: Option<&Target>) -> String {
    match target {
        Some(Target::Device(id)) => state.space.as_ref().map_or_else(
            || {
                format!(
                    "Unavailable device · {}",
                    id.chars().take(8).collect::<String>()
                )
            },
            |sync| sync.remote.host_label(id),
        ),
        _ => "This device".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_override_beats_project_routes_and_can_be_removed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let project = crate::projects::create(temp.path(), "Project")?;
        let space = crate::sync::space::Space::create("Source", None)?;
        let mut sync = crate::sync::adapter::SyncState::new(space.id);
        sync.remote
            .platforms
            .insert("desktop".into(), "windows".into());
        sync.remote
            .platforms
            .insert("phone".into(), "android".into());
        let mut state = State {
            space: Some(sync),
            ..Default::default()
        };
        state.thread.project = Some(project.path.clone());
        let project_scope = Scope::Project(project.path);
        let chat = Scope::Chat(state.chat_id);
        let desktop = Target::Device("desktop".into());
        let phone = Target::Device("phone".into());
        set(
            &mut state,
            project_scope.clone(),
            Kind::Default,
            Some(desktop.clone()),
        )?;
        set(
            &mut state,
            project_scope.clone(),
            Kind::Apk,
            Some(phone.clone()),
        )?;
        assert!(set(&mut state, project_scope, Kind::Build, Some(phone.clone())).is_err());
        assert_eq!(
            state.placement.effective(&state, &chat, Kind::Python),
            Some(&desktop)
        );
        assert_eq!(
            state.placement.effective(&state, &chat, Kind::Apk),
            Some(&phone)
        );
        set(&mut state, chat.clone(), Kind::Default, Some(Target::Local))?;
        assert_eq!(
            state.placement.effective(&state, &chat, Kind::Apk),
            Some(&Target::Local)
        );
        set(&mut state, chat.clone(), Kind::Apk, Some(phone.clone()))?;
        let saved = serde_json::from_str::<State>(&serde_json::to_string(&state)?)?;
        assert_eq!(
            saved.placement.effective(&saved, &chat, Kind::Apk),
            Some(&phone)
        );
        set(&mut state, chat.clone(), Kind::Default, None)?;
        assert_eq!(
            state.placement.effective(&state, &chat, Kind::Python),
            Some(&desktop)
        );
        Ok(())
    }
}
