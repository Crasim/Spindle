//! Native Windows notifications for finished replies.

use anyhow::{Context, Result};
use std::{os::windows::ffi::OsStrExt, path::Path};
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
}

impl Notifications {
    pub(crate) fn reply(
        &mut self,
        workspace: &Path,
        prefs: crate::state::ReplyNotifications,
        response: &str,
    ) {
        let mut foreground = 0;
        unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut foreground) };
        if let Some(body) = notification_body(prefs, foreground == std::process::id(), response)
            && let Err(error) = self.show(workspace, &body)
        {
            eprintln!("Spindle notification: {error:#}");
        }
    }

    fn show(&mut self, workspace: &Path, body: &str) -> Result<()> {
        let _runtime = Runtime::initialize()?;
        if !self.registered {
            register(workspace).context("Register Spindle for Windows notifications")?;
            self.registered = true;
        }
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?;
        notifier
            .Show(&ToastNotification::CreateToastNotification(
                &toast_document(body)?,
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

fn toast_document(body: &str) -> windows::core::Result<XmlDocument> {
    let document = XmlDocument::new()?;
    document.LoadXml(&HSTRING::from(concat!(
        r#"<toast><visual><binding template="ToastGeneric">"#,
        r#"<text>Spindle reply ready</text><text hint-maxLines="4"/>"#,
        "</binding></visual></toast>",
    )))?;
    document
        .GetElementsByTagName(&HSTRING::from("text"))?
        .Item(1)?
        .AppendChild(&document.CreateTextNode(&HSTRING::from(body))?)?;
    Ok(document)
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
}
