//! Native Windows notifications for finished replies and pending approvals.

use anyhow::{Context, Result};
use std::{collections::{BTreeMap, BTreeSet}, os::windows::ffi::OsStrExt, path::Path};
use windows::{
    Data::Xml::Dom::XmlDocument,
    UI::Notifications::{ToastNotification, ToastNotificationManager},
    Win32::{
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile},
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
        UI::Shell::{
            IShellLinkW,
            PropertiesSystem::{IPropertyStore, PROPERTYKEY, PSCoerceToCanonicalValue},
            SHCNE_CREATE, SHCNE_UPDATEITEM, SHCNF_FLUSH, SHCNF_PATHW, SHChangeNotify, ShellLink,
        },
    },
    core::{GUID, HSTRING, Interface, PCWSTR, PROPVARIANT},
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const APP_ID: &str = "SpindleApp.Spindle";
const APP_ID_PROPERTY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

struct Runtime;
impl Runtime {
    fn initialize() -> windows::core::Result<Self> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        Ok(Self)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

#[derive(Default)]
pub struct Notifications {
    registered: bool,
    approvals: ApprovalAlerts,
}

fn spindle_is_foreground() -> bool {
    let mut foreground = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut foreground) };
    foreground == std::process::id()
}

#[derive(Default)]
struct ApprovalAlerts {
    announced: BTreeSet<(u64, crate::attention::Decision)>,
}

impl ApprovalAlerts {
    fn update(&mut self, waiting: &BTreeMap<u64, Vec<crate::attention::Decision>>, viewed: Option<u64>, enabled: bool) -> BTreeSet<u64> {
        self.announced.retain(|(chat, decision)| waiting.get(chat).is_some_and(|active| active.contains(decision)));
        let mut chats = BTreeSet::new();
        if enabled {
            for (&chat, decisions) in waiting {
                if viewed != Some(chat) {
                    for &decision in decisions {
                        if self.announced.insert((chat, decision)) { chats.insert(chat); }
                    }
                }
            }
        }
        chats
    }
}

impl Notifications {
    pub(crate) fn reply(
        &mut self,
        workspace: &Path,
        prefs: crate::state::ReplyNotifications,
        response: &str,
    ) {
        if let Some(body) = notification_body(prefs, spindle_is_foreground(), response)
            && let Err(error) = self.show(workspace, "Spindle reply ready", &body)
        {
            eprintln!("Spindle notification: {error:#}");
        }
    }

    pub(crate) fn attention(&mut self, workspace: &Path, view: &crate::engine::View) {
        let waiting = crate::attention::waiting(view);
        let viewed = if spindle_is_foreground() { view.state.viewed_chat } else { None };
        let prefs = view.state.preferences.reply_notifications;
        let chats = self.approvals.update(&waiting, viewed, prefs.approvals);
        if chats.is_empty() { return; }
        let body = approval_body(&view.state, &chats, prefs.show_text);
        if let Err(error) = self.show(workspace, "Spindle needs your attention", &body) {
            eprintln!("Spindle approval notification: {error:#}");
        }
    }

    fn show(&mut self, workspace: &Path, title: &str, body: &str) -> Result<()> {
        let _runtime = Runtime::initialize()?;
        if !self.registered {
            register(workspace).context("Register Spindle for Windows notifications")?;
            self.registered = true;
        }
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?;
        notifier
            .Show(&ToastNotification::CreateToastNotification(
                &toast_document(title, body)?,
            )?)
            .context("Send native Windows notification")
    }
}

fn register(workspace: &Path) -> Result<()> {
    // The unpackaged WinRT API identifies the app through its user Start menu shortcut.
    let folder = std::path::PathBuf::from(
        std::env::var_os("APPDATA").context("No Windows user application data directory")?,
    )
    .join("Microsoft/Windows/Start Menu/Programs/Spindle");
    std::fs::create_dir_all(&folder)?;
    let executable = std::env::current_exe()?;
    let wide_path = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>()
    };
    let executable = wide_path(&executable);
    let workspace = wide_path(workspace);
    let shortcut = folder.join("Spindle.lnk");
    let event = if shortcut.exists() {
        SHCNE_UPDATEITEM
    } else {
        SHCNE_CREATE
    };
    let shortcut = wide_path(&shortcut);
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(executable.as_ptr()))?;
        link.SetWorkingDirectory(PCWSTR(workspace.as_ptr()))?;
        let properties: IPropertyStore = link.cast()?;
        let mut value = PROPVARIANT::from(APP_ID);
        PSCoerceToCanonicalValue(&APP_ID_PROPERTY, &mut value)?;
        properties.SetValue(&APP_ID_PROPERTY, &value)?;
        properties.Commit()?;
        link.cast::<IPersistFile>()?
            .Save(PCWSTR(shortcut.as_ptr()), true)?;
        SHChangeNotify(
            event,
            SHCNF_PATHW | SHCNF_FLUSH,
            Some(shortcut.as_ptr().cast()),
            None,
        );
    }
    Ok(())
}

fn toast_document(title: &str, body: &str) -> windows::core::Result<XmlDocument> {
    let document = XmlDocument::new()?;
    document.LoadXml(&HSTRING::from(concat!(
        r#"<toast><visual><binding template="ToastGeneric">"#,
        r#"<text/><text hint-maxLines="4"/>"#,
        "</binding></visual></toast>",
    )))?;
    let nodes = document.GetElementsByTagName(&HSTRING::from("text"))?;
    for (index, line) in [title, body].into_iter().enumerate() {
        nodes.Item(index as u32)?.AppendChild(&document.CreateTextNode(&HSTRING::from(line))?)?;
    }
    Ok(document)
}

fn approval_body(state: &crate::state::State, chats: &BTreeSet<u64>, show_names: bool) -> String {
    let count = chats.len();
    if show_names && count == 1
        && let Some((thread, history)) = chats.first().and_then(|&id| state.thread_data(id))
    {
        let label = thread.label(history).split_whitespace().collect::<Vec<_>>().join(" ");
        let label: String = label.chars().filter(|ch| !ch.is_control()).take(100).collect();
        return format!("“{label}” is waiting for your decision. Open Needs attention in Spindle.");
    }
    if count == 1 {
        "A conversation is waiting for your decision. Open Needs attention in Spindle.".into()
    } else {
        format!("{count} conversations need a decision. Open Needs attention in Spindle.")
    }
}

fn notification_body(
    prefs: crate::state::ReplyNotifications,
    focused: bool,
    response: &str,
) -> Option<String> {
    if focused || !prefs.enabled {
        return None;
    }
    let generic = "A reply is ready in Spindle.";
    if !prefs.show_text {
        return Some(generic.into());
    }
    use pulldown_cmark::{Event, Parser, TagEnd};
    let mut plain = String::new();
    for event in Parser::new(response) {
        match event {
            Event::Text(text) | Event::Code(text) => plain.push_str(&text),
            Event::SoftBreak
            | Event::HardBreak
            | Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::CodeBlock,
            ) => plain.push(' '),
            _ => {}
        }
        if plain.len() >= 2600 {
            break;
        }
    }
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if plain.is_empty() {
        return Some(generic.into());
    }
    let mut chars = plain.chars().filter(|ch| !ch.is_control());
    let mut body = chars.by_ref().take(480).collect::<String>();
    if chars.next().is_some() {
        body.push('…');
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ReplyNotifications;

    #[test]
    fn replies_can_be_private_or_suppressed() {
        let mut prefs = ReplyNotifications::default();
        assert_eq!(
            notification_body(prefs, false, "**Done** & ready.").as_deref(),
            Some("Done & ready.")
        );
        assert!(notification_body(prefs, true, "private").is_none());
        prefs.show_text = false;
        assert_eq!(
            notification_body(prefs, false, "private").as_deref(),
            Some("A reply is ready in Spindle.")
        );
        prefs.enabled = false;
        assert!(notification_body(prefs, false, "private").is_none());
    }

    #[test]
    fn approvals_notify_once_until_resolved_and_skip_the_visible_chat() {
        use crate::attention::Decision::Tool;
        let mut alerts = ApprovalAlerts::default();
        let mut waiting = BTreeMap::from([(3, vec![Tool(11), Tool(12)]), (7, vec![Tool(14)])]);
        assert!(alerts.update(&waiting, None, false).is_empty());
        assert_eq!(alerts.update(&waiting, Some(3), true), BTreeSet::from([7]));
        assert_eq!(alerts.update(&waiting, None, true), BTreeSet::from([3]));
        assert!(alerts.update(&waiting, None, true).is_empty());
        waiting.get_mut(&3).unwrap().push(Tool(15));
        assert_eq!(alerts.update(&waiting, None, true), BTreeSet::from([3]));
        waiting.remove(&3);
        assert!(alerts.update(&waiting, None, true).is_empty());
        waiting.insert(3, vec![Tool(15)]);
        assert_eq!(alerts.update(&waiting, None, true), BTreeSet::from([3]));
    }

    #[test]
    fn approval_notifications_respect_privacy_and_escape_names_in_native_xml() -> Result<()> {
        let _runtime = Runtime::initialize()?;
        let mut state = crate::state::State::default();
        state.thread.title = "Private & <draft>".into();
        let chats = BTreeSet::from([state.chat_id]);
        assert!(!approval_body(&state, &chats, false).contains("Private"));
        let body = approval_body(&state, &chats, true);
        assert!(body.contains("Private & <draft>"));
        let document = toast_document("Spindle needs your attention", &body)?;
        let xml = document.GetXml()?.to_string_lossy();
        assert!(xml.contains("Private &amp; &lt;draft&gt;"), "{xml}");
        ToastNotification::CreateToastNotification(&document)?;
        Ok(())
    }
}
