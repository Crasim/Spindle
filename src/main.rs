#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod artifacts;
mod android_apk;
#[cfg(target_os = "android")]
mod android_background;
mod automation;
mod browser;
mod location;
mod browser_link;
#[cfg(any(windows, target_os = "linux"))]
mod browser_channel;
mod browser_native;
#[cfg(any(windows, target_os = "linux", target_os = "android"))]
mod updates;
mod local_servers;
#[cfg(any(windows, target_os = "linux", target_os = "android", test))]
mod public_web;
mod cli;
#[cfg(any(windows, target_os = "linux"))]
mod claude_console;
mod comfy;
mod connectors;
mod conversation_import;
mod compaction;
mod control;
mod desktop;
mod diff;
mod documents;
mod editing;
mod engine;
mod eval;
mod execution;
mod generation;
mod html_preview;
mod git;
mod mcp;
mod memory;
mod meter;
#[cfg(any(windows, target_os = "android"))]
mod oauth;
mod ollama;
mod packages;
mod pastes;
mod plan;
mod policy;
mod project_history;
mod projects;
mod providers;
mod runtime;
mod sandbox;
mod native;
mod rust_build;
#[cfg(target_os = "android")]
mod scan;
mod service;
mod speech;
mod state;
mod sync;
mod threads;
mod tool_catalog;
mod tools;
mod tray;
mod usage;
mod vault;
mod widgets;
mod workcopy;

use anyhow::{Context, Result, bail};
use engine::Engine;
use ollama::{Message, Ollama};
use std::{
    io::{self, BufRead},
    sync::mpsc,
    thread,
};
use tokio::sync::mpsc as channel;

/// The Android app: NativeActivity loads this library and calls `android_main`.
#[cfg(target_os = "android")]
mod android {
    use std::sync::OnceLock;
    pub use winit::platform::android::activity::AndroidApp;

    pub static APP: OnceLock<AndroidApp> = OnceLock::new();

    /// A `spindle://pair` link Spindle was opened with (scanning the QR code with the phone's
    /// camera), handed out once.
    static LINK: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

    pub fn take_link() -> Option<String> {
        LINK.lock().unwrap().take()
    }

    pub fn choose_conversation_file() -> jni::errors::Result<()> {
        java(|env, activity| env.call_method(activity, "chooseConversationFile", "()V", &[]).map(|_| ()))
    }

    pub fn take_conversation_file() -> Option<Result<String, String>> {
        let value = java(|env, activity| {
            let value = env.call_method(activity, "takeConversationFile", "()Ljava/lang/String;", &[])?.l()?;
            if value.is_null() {
                return Ok(String::new());
            }
            Ok(String::from(env.get_string(&value.into())?))
        }).ok()?;
        if let Some(name) = value.strip_prefix("file:") {
            Some(Ok(name.to_owned()))
        } else {
            value.strip_prefix("error:").map(|message| Err(message.to_owned()))
        }
    }

    static BACK_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_org_spindleapp_spindle_SpindleActivity_nativeBack(
        _env: jni::JNIEnv,
        _activity: jni::objects::JObject,
    ) {
        BACK_REQUESTED.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn take_back() -> bool {
        BACK_REQUESTED.swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    pub fn set_back_navigation(enabled: bool) {
        let _ = java(|env, activity| {
            env.call_method(activity, "setBackNavigation", "(Z)V", &[enabled.into()])?;
            Ok(())
        });
    }

    /// Whether the camera may be used (asked for the first time the scanner opens).
    pub fn camera_allowed() -> bool {
        java(|env, activity| {
            let name = env.new_string("android.permission.CAMERA")?;
            let r = env
                .call_method(
                    activity,
                    "checkSelfPermission",
                    "(Ljava/lang/String;)I",
                    &[jni::objects::JValue::Object(&name)],
                )?
                .i()?;
            Ok(r == 0)
        })
        .unwrap_or(false)
    }

    /// Show Android's "allow the camera?" prompt.
    pub fn ask_camera() {
        let _ = java(|env, activity| {
            let name = env.new_string("android.permission.CAMERA")?;
            let arr = env.new_object_array(1, "java/lang/String", &name)?;
            env.call_method(
                activity,
                "requestPermissions",
                "([Ljava/lang/String;I)V",
                &[
                    jni::objects::JValue::Object(&arr),
                    jni::objects::JValue::Int(7),
                ],
            )?;
            Ok(())
        });
    }

    /// The phone's model name ("Pixel 8"), for naming this device.
    pub fn device_name() -> Option<String> {
        static NAME: OnceLock<Option<String>> = OnceLock::new();
        NAME.get_or_init(|| {
            java(|env, _| {
                let model = env
                    .get_static_field("android/os/Build", "MODEL", "Ljava/lang/String;")?
                    .l()?;
                let s: String = env.get_string(&model.into())?.into();
                Ok(s)
            })
            .ok()
            .filter(|s| !s.trim().is_empty())
        })
        .clone()
    }

    /// Let the Wi-Fi hand Spindle the other devices' announcements: many phones drop
    /// broadcast packets for apps that don't hold a multicast lock.
    fn multicast_lock() {
        let r = java(|env, activity| {
            let name = env.new_string("wifi")?;
            let wifi = env
                .call_method(
                    activity,
                    "getSystemService",
                    "(Ljava/lang/String;)Ljava/lang/Object;",
                    &[(&name).into()],
                )?
                .l()?;
            if wifi.is_null() {
                return Ok(());
            }
            let tag = env.new_string("spindle")?;
            let lock = env
                .call_method(
                    &wifi,
                    "createMulticastLock",
                    "(Ljava/lang/String;)Landroid/net/wifi/WifiManager$MulticastLock;",
                    &[(&tag).into()],
                )?
                .l()?;
            env.call_method(&lock, "setReferenceCounted", "(Z)V", &[false.into()])?;
            env.call_method(&lock, "acquire", "()V", &[])?;
            // Held for as long as the app runs.
            std::mem::forget(env.new_global_ref(&lock)?);
            Ok(())
        });
        if let Err(e) = r {
            log(&format!("No multicast lock: {e}"));
        }
    }

    /// The link in the intent that opened the app, if any.
    fn intent_link() -> Option<String> {
        java(|env, activity| {
            let intent = env
                .call_method(activity, "getIntent", "()Landroid/content/Intent;", &[])?
                .l()?;
            if intent.is_null() {
                return Ok(None);
            }
            let data = env
                .call_method(&intent, "getDataString", "()Ljava/lang/String;", &[])?
                .l()?;
            if data.is_null() {
                return Ok(None);
            }
            let s: String = env.get_string(&data.into())?.into();
            Ok(Some(s))
        })
        .ok()
        .flatten()
        .filter(|s| s.starts_with("spindle://pair"))
    }

    /// Run some JNI with the activity, clearing any Java exception it leaves.
    pub(super) fn java<T>(
        f: impl FnOnce(&mut jni::JNIEnv, &jni::objects::JObject) -> jni::errors::Result<T>,
    ) -> jni::errors::Result<T> {
        let app = APP.get().ok_or(jni::errors::Error::NullPtr("app"))?;
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as *mut _)? };
        let mut env = vm.attach_current_thread()?;
        let activity =
            unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
        let r = f(&mut env, &activity);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        r
    }

    pub fn request_web_location(id: u64) -> bool {
        java(|env, activity| {
            env.call_method(activity, "requestWebLocation", "(J)V", &[jni::objects::JValue::Long(id as i64)])?;
            Ok(())
        }).is_ok()
    }

    pub fn copy_text(text: &str) {
        let _ = java(|env, activity| {
            let value = env.new_string(text)?;
            env.call_method(activity, "copyChatSelection", "(Ljava/lang/String;)V", &[(&value).into()])?;
            Ok(())
        });
    }

    pub fn show_html_preview(url: &str, scope: &str, bounds: [i32; 4]) -> jni::errors::Result<()> {
        java(|env, activity| {
            let url = env.new_string(url)?;
            let scope = env.new_string(scope)?;
            let [x, y, width, height] = bounds;
            env.call_method(activity, "showHtmlPreview", "(Ljava/lang/String;Ljava/lang/String;IIII)V", &[
                (&url).into(), (&scope).into(), x.into(), y.into(), width.into(), height.into(),
            ])?;
            Ok(())
        })
    }

    pub fn hide_html_preview() {
        let _ = java(|env, activity| env.call_method(activity, "hideHtmlPreview", "()V", &[]).map(|_| ()));
    }

    pub fn build_apk(request: &str) -> anyhow::Result<String> {
        use anyhow::Context;
        java(|env, activity| {
            let request = env.new_string(request)?;
            let result = env.call_method(activity, "buildApk",
                "(Ljava/lang/String;)Ljava/lang/String;", &[(&request).into()])?.l()?;
            let result = jni::objects::JString::from(result);
            Ok(env.get_string(&result)?.into())
        }).context("Android APK build service did not respond")
    }

    pub fn download_apk(name: &str, bytes: &[u8]) -> anyhow::Result<String> {
        use anyhow::Context;
        java(|env, activity| {
            let name = env.new_string(name)?;
            let input = env.byte_array_from_slice(bytes)?;
            let result = env.call_method(activity, "downloadApk",
                "(Ljava/lang/String;[B)Ljava/lang/String;", &[(&name).into(), (&input).into()])?.l()?;
            let result = jni::objects::JString::from(result);
            Ok(env.get_string(&result)?.into())
        }).context("Android could not save the APK to Downloads")
    }

    pub fn open_update_apk(version: &str) -> anyhow::Result<()> {
        use anyhow::Context;
        let error: String = java(|env, activity| {
            let version = env.new_string(version)?;
            let result = env.call_method(activity, "openUpdateApk",
                "(Ljava/lang/String;)Ljava/lang/String;", &[(&version).into()])?.l()?;
            let result = jni::objects::JString::from(result);
            Ok(env.get_string(&result)?.into())
        }).context("Android could not open the APK")?;
        anyhow::ensure!(error.is_empty(), "{error}");
        Ok(())
    }

    pub fn protect_provider_credential(input: &[u8], encrypt: bool) -> anyhow::Result<Vec<u8>> {
        use anyhow::Context;
        java(|env, activity| {
            let input = env.byte_array_from_slice(input)?;
            let sealed = env.call_method(activity, "protectProviderCredential", "([BZ)[B", &[
                (&input).into(), encrypt.into(),
            ])?.l()?;
            env.convert_byte_array(jni::objects::JByteArray::from(sealed))
        }).context("Android Keystore could not open or protect the provider credential")
    }

    /// Show or hide the on-screen keyboard. The request winit makes (implicit, on the decor
    /// view) is ignored for a NativeActivity, so ask the input method directly, forced, for
    /// the view NativeActivity gives focus to (its content view); only when the wish changes.
    pub fn keyboard(want: bool, native_editor: bool) {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};
        static SHOWN: AtomicBool = AtomicBool::new(false);
        static WANTED: Mutex<Option<Instant>> = Mutex::new(None);
        if native_editor {
            SHOWN.store(false, Ordering::Relaxed);
            *WANTED.lock().unwrap() = None;
            return;
        }
        // Opening the keyboard resizes the window, and a field can lose focus for a frame
        // while the layout settles; closing at once would reopen it on the next frame, and
        // the keyboard flickers. Close only once nothing has wanted it for a moment.
        let show = {
            let mut last = WANTED.lock().unwrap();
            if want {
                *last = Some(Instant::now());
                true
            } else {
                last.is_some_and(|t| t.elapsed() < Duration::from_millis(500))
            }
        };
        if SHOWN.swap(show, Ordering::Relaxed) == show {
            return;
        }
        if let Err(e) = soft_input(show) {
            log(&format!("Keyboard: {e}"));
        }
    }

    pub fn set_composer_ime(revision: i32, text: &str, start: i32, end: i32, show: bool, mode: i32) -> bool {
        java(|env, activity| {
            let text = env.new_string(text)?;
            env.call_method(
                activity, "setComposerIme", "(ILjava/lang/String;IIZI)V",
                &[revision.into(), (&text).into(), start.into(), end.into(), show.into(), mode.into()],
            )?;
            Ok(())
        }).is_ok()
    }

    pub fn take_composer_ime() -> Option<(i32, String, usize, usize)> {
        java(|env, activity| {
            use jni::objects::{JObjectArray, JString};
            let result = env.call_method(activity, "takeComposerIme", "()[Ljava/lang/String;", &[])?.l()?;
            if result.is_null() { return Ok(None); }
            let array = JObjectArray::from(result);
            let mut string = |index| {
                let item = JString::from(env.get_object_array_element(&array, index)?);
                env.get_string(&item).map(Into::<String>::into)
            };
            let revision = string(0)?.parse::<i32>().unwrap_or(0);
            let text = string(1)?;
            let start = string(2)?.parse().unwrap_or_else(|_| text.encode_utf16().count());
            let end = string(3)?.parse().unwrap_or(start);
            Ok(Some((revision, text, start, end)))
        }).ok().flatten()
    }

    pub fn close_composer_ime() {
        let _ = java(|env, activity| {
            env.call_method(activity, "closeComposerIme", "()V", &[])?;
            Ok(())
        });
    }

    fn soft_input(show: bool) -> jni::errors::Result<()> {
        let Some(app) = APP.get() else {
            return Ok(());
        };
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr() as *mut _)? };
        let mut env = vm.attach_current_thread()?;
        let result = soft_input_with(&mut env, app, show);
        // A Java exception left pending would break every later call on this thread.
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
        }
        result
    }

    fn soft_input_with(
        env: &mut jni::JNIEnv,
        app: &AndroidApp,
        show: bool,
    ) -> jni::errors::Result<()> {
        use jni::objects::{JObject, JValue};
        let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
        let service = env
            .get_static_field(
                "android/content/Context",
                "INPUT_METHOD_SERVICE",
                "Ljava/lang/String;",
            )?
            .l()?;
        let imm = env
            .call_method(
                &activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&service)],
            )?
            .l()?;
        let window = env
            .call_method(&activity, "getWindow", "()Landroid/view/Window;", &[])?
            .l()?;
        let decor = env
            .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])?
            .l()?;
        // android.R.id.content holds NativeActivity's own view.
        let content = env
            .call_method(
                &decor,
                "findViewById",
                "(I)Landroid/view/View;",
                &[JValue::Int(0x01020002)],
            )?
            .l()?;
        let view = if content.is_null() {
            decor
        } else {
            let n = env
                .call_method(&content, "getChildCount", "()I", &[])?
                .i()?;
            if n > 0 {
                env.call_method(
                    &content,
                    "getChildAt",
                    "(I)Landroid/view/View;",
                    &[JValue::Int(0)],
                )?
                .l()?
            } else {
                content
            }
        };
        if show {
            // InputMethodManager.SHOW_FORCED = 2
            env.call_method(
                &imm,
                "showSoftInput",
                "(Landroid/view/View;I)Z",
                &[JValue::Object(&view), JValue::Int(2)],
            )?;
        } else {
            let token = env
                .call_method(&view, "getWindowToken", "()Landroid/os/IBinder;", &[])?
                .l()?;
            env.call_method(
                &imm,
                "hideSoftInputFromWindow",
                "(Landroid/os/IBinder;I)Z",
                &[JValue::Object(&token), JValue::Int(0)],
            )?;
        }
        Ok(())
    }

    /// The part of the window covered by the status bar (top) and by the navigation bar or
    /// the on-screen keyboard (bottom), in pixels.
    pub fn insets(window_height_px: f32) -> (f32, f32) {
        let Some(app) = APP.get() else {
            return (0., 0.);
        };
        let r = app.content_rect();
        if r.bottom <= r.top {
            return (0., 0.);
        }
        let top = r.top.max(0) as f32;
        let bottom = (window_height_px - r.bottom as f32).max(0.);
        (top, bottom)
    }

    /// Write a line to Android's log (`adb logcat -s Spindle`).
    pub fn log(msg: &str) {
        unsafe extern "C" {
            fn __android_log_write(
                prio: i32,
                tag: *const std::ffi::c_char,
                text: *const std::ffi::c_char,
            ) -> i32;
        }
        let text = std::ffi::CString::new(msg.replace('\0', " ")).unwrap_or_default();
        unsafe { __android_log_write(4, c"Spindle".as_ptr(), text.as_ptr()) };
    }

    /// The app's private storage holds everything: the workspace, Spindle's state, and the
    /// key that encrypts the session (other apps can't read the app's private storage).
    fn prepare(app: &AndroidApp) -> anyhow::Result<()> {
        use anyhow::Context;
        let base = app.internal_data_path().context("No app storage")?;
        let workspace = base.join("workspace");
        for d in ["state", "data", "tmp", "workspace"] {
            std::fs::create_dir_all(base.join(d))?;
        }
        let key_file = base.join("session.key");
        let key = match std::fs::read_to_string(&key_file) {
            Ok(k) if k.trim().len() >= 32 => k.trim().to_owned(),
            _ => {
                let k = crate::sync::core::hex(&crate::vault::random::<32>()?);
                std::fs::write(&key_file, &k)?;
                k
            }
        };
        // Set before any other thread starts.
        unsafe {
            std::env::set_var("HOME", &base);
            std::env::set_var("XDG_STATE_HOME", base.join("state"));
            std::env::set_var("XDG_DATA_HOME", base.join("data"));
            std::env::set_var("SPINDLE_DATA_DIR", base.join("data").join("spindle"));
            std::env::set_var("TMPDIR", base.join("tmp"));
            std::env::set_var("SPINDLE_SESSION_PASSPHRASE", key);
        }
        std::env::set_current_dir(&workspace)?;
        Ok(())
    }

    #[unsafe(no_mangle)]
    fn android_main(app: AndroidApp) {
        std::panic::set_hook(Box::new(|p| log(&format!("panic: {p}"))));
        if let Err(e) = prepare(&app) {
            log(&format!("Couldn't prepare storage: {e:#}"));
            return;
        }
        let _ = APP.set(app);
        *LINK.lock().unwrap() = intent_link();
        multicast_lock();
        if let Err(e) = super::main() {
            log(&format!("Spindle stopped: {e:#}"));
        }
    }
}

/// Spindle's version: the same major and minor on every platform, plus the Android app's
/// own build number (counted by android/build-apk.sh).
pub fn version() -> String {
    let built = option_env!("SPINDLE_BUILT")
        .and_then(|t| t.parse::<i64>().ok())
        .map(|t| format!(" · built {}", automation::local_time(t)))
        .unwrap_or_default();
    match option_env!("SPINDLE_ANDROID_BUILD") {
        Some(n) => format!("{} · Android build {n}{built}", env!("CARGO_PKG_VERSION")),
        None => format!("{}{built}", env!("CARGO_PKG_VERSION")),
    }
}

fn main() -> Result<()> {
    #[cfg(target_os = "android")]
    let args: Vec<String> = vec![];
    #[cfg(not(target_os = "android"))]
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(origin) = args.first().filter(|arg| arg.starts_with("chrome-extension://")) {
        return browser_native::host(origin);
    }
    #[cfg(all(windows, not(debug_assertions)))]
    if std::env::var_os("SPINDLE_GUI_LAUNCH").is_none() && args.first().is_some_and(|first|
        !matches!(first.as_str(), "serve" | "--run-due")
            && (first != "service" || matches!(args.get(1).map(String::as_str), Some("install" | "uninstall" | "status" | "stop")))) {
        // Command-line use from an existing console still has stdin, stdout and stderr.
        // A direct desktop launch never allocates a console of its own.
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
    #[cfg(any(windows, target_os = "linux"))]
    if args.is_empty() {
        match updates::launch_staged() {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => eprintln!("Spindle update: {error:#}"),
        }
    }
    let runtime = tokio::runtime::Runtime::new()?;
    let mut api = Ollama::new()?;
    match args.first().map(String::as_str) {
        Some("--read") if args.len() > 1 => {
            println!(
                "{}",
                documents::read_section(
                    &std::env::current_dir()?.canonicalize()?,
                    &args[1..].join(" ")
                )?
            );
            return Ok(());
        }
        Some("--help" | "-h") => {
            println!("{}", cli::USAGE);
            println!(
                "\nOther: spindle [--cli | --eval --model M [--ollama URL | --provider-url URL] [--thinking default|off|low|medium|high] [--dir DIR] [--minutes N] [--goal] [--allow-setup] [--seed DIR] PROMPT | --discover [HOST] | --models | --ask MODEL PROMPT | --read [--section N] FILE | --run-due PROJECT | --setup-python | --probe]\nDefault: open desktop workspace in the current directory."
            );
            return Ok(());
        }
        Some("--eval") => return eval::main(&args[1..]),
        Some("--discover") if args.len() <= 2 => {
            let host = args.get(1).map_or("localhost", String::as_str);
            let servers = runtime.block_on(local_servers::discover(host))?;
            if servers.is_empty() {
                println!("No model or image servers answered on the standard ports at {host}");
            }
            for server in servers {
                println!("{}  {}  · {} models", server.name, server.url, server.models);
            }
            return Ok(());
        }
        // Talking to the Spindle that owns a workspace (control.rs): the CLI and MCP.
        Some(
            "ask" | "send" | "status" | "chats" | "show" | "approve" | "deny" | "file" | "projects"
            | "models" | "sync",
        ) => return cli::main(&args),
        Some("mcp") => return mcp::main(&args[1..]),
        // Running in the background (service.rs): manage it, or be it.
        Some("service")
            if matches!(
                args.get(1).map(String::as_str),
                Some("install" | "uninstall" | "status" | "stop")
            ) =>
        {
            let mut root = std::env::current_dir()?;
            if let Some(i) = args.iter().position(|a| a == "--dir") {
                root = std::path::PathBuf::from(args.get(i + 1).context("--dir needs a folder")?);
            }
            let root = root.canonicalize()?;
            let out = match args[1].as_str() {
                "install" => service::install(&root)?,
                "uninstall" => {
                    service::stop(&root)?;
                    service::uninstall(&root)?
                }
                "stop" => service::stop(&root)?,
                _ => service::status(&root),
            };
            println!("{out}");
            return Ok(());
        }
        Some("service") => {
            if let Some(i) = args.iter().position(|a| a == "--dir") {
                std::env::set_current_dir(args.get(i + 1).context("--dir needs a folder")?)?;
            }
            // Started at sign-in: no console window.
            #[cfg(windows)]
            unsafe {
                windows_sys::Win32::System::Console::FreeConsole();
            }
        }
        Some("sync-server") => {
            let mut data = None;
            let mut listen = "0.0.0.0:8443".to_owned();
            let mut mode = "store".to_owned();
            let mut mailbox = 24u64;
            let mut admin = std::env::var("SPINDLE_SYNC_ADMIN").unwrap_or_default();
            let mut it = args[1..].iter();
            while let Some(a) = it.next() {
                let mut next = || {
                    it.next()
                        .cloned()
                        .with_context(|| format!("{a} needs a value"))
                };
                match a.as_str() {
                    "--data" => data = Some(std::path::PathBuf::from(next()?)),
                    "--listen" => listen = next()?,
                    "--mode" => mode = next()?,
                    "--mailbox-hours" => {
                        mailbox = next()?.parse().context("--mailbox-hours is a number")?
                    }
                    "--admin-token" => admin = next()?,
                    _ => bail!(
                        "Use: spindle sync-server --data DIR [--listen ADDR] [--mode store|relay|both] [--mailbox-hours N] [--admin-token T]"
                    ),
                }
            }
            let cfg = sync::hosted::Config {
                data: data.context("Give --data DIR (where the server keeps encrypted records)")?,
                mode: sync::hosted::Mode::parse(&mode)?,
                mailbox: std::time::Duration::from_secs(mailbox * 3600),
                admin,
            };
            if cfg.admin.is_empty() {
                eprintln!("No --admin-token (or SPINDLE_SYNC_ADMIN): no new spaces can be added.");
            }
            return runtime.block_on(sync::hosted::serve(cfg, &listen));
        }
        Some("serve") => {}
        Some("--setup-python") => {
            println!(
                "Downloading Python {} (python-build-standalone {}) and checking it against the pinned SHA-256…",
                runtime::PIN.version,
                runtime::PIN.release
            );
            let rt = runtime.block_on(runtime::install())?;
            println!("Installed in {}", rt.home.display());
            return Ok(());
        }
        Some("--probe") => {
            let rt = runtime::installed()
                .context("Spindle's Python isn't set up. Run `spindle --setup-python` first, or use Settings › Sandbox.")?;
            let report = sandbox::probe(&rt, &runtime::data_dir().join("probe"));
            println!("Sandbox check · {} · {}", report.backend, report.runtime);
            for c in &report.checks {
                println!(
                    "  {} {} · {}",
                    if c.passed { "PASS" } else { "FAIL" },
                    c.name,
                    c.detail
                );
            }
            if !report.problem.is_empty() {
                println!("{}", report.problem);
            }
            let file = runtime::data_dir().join("probe").join("report.json");
            std::fs::write(&file, serde_json::to_vec_pretty(&report)?)?;
            println!(
                "{} · report saved to {}",
                if report.ready { "READY" } else { "NOT READY" },
                file.display()
            );
            std::process::exit(if report.ready { 0 } else { 1 });
        }
        Some("--models") if args.len() == 1 => {
            for m in runtime.block_on(api.models())? {
                println!("{m}");
            }
            return Ok(());
        }
        Some("--ask") if args.len() >= 3 => {
            let prompt = args[2..].join(" ");
            anyhow::ensure!(prompt.len() <= 12000, "Prompt exceeds input capacity");
            println!(
                "{}",
                runtime.block_on(api.chat(&args[1], &[Message::new("user", prompt)]))?
            );
            return Ok(());
        }
        Some("--run-due") if args.len() == 2 => {}
        Some("--cli") if args.len() == 1 => {}
        None => {}
        _ => bail!("Use --help for usage"),
    }
    let background = args.first().is_some_and(|s| s == "--run-due");
    let root = if background {
        std::path::PathBuf::from(&args[1])
    } else {
        std::env::current_dir()?
    }
    .canonicalize()?;
    let service = args.first().is_some_and(|s| s == "service");
    let serving = args.first().is_some_and(|s| s == "serve") || service;
    // A windowless Spindle (started by the CLI or MCP) may hold the workspace: the window
    // and the terminal ask it to hand over first.
    if !background && !serving && state::Store::open(&root).is_err() {
        let _ = control::hand_over(&root);
    }
    let (store, state) = match state::Store::open(&root) {
        Err(e)
            if background
                && matches!(
                    e.downcast_ref::<std::fs::TryLockError>(),
                    Some(std::fs::TryLockError::WouldBlock)
                ) =>
        {
            return Ok(());
        }
        other => other?,
    };
    let (partial, preview) = tokio::sync::watch::channel(String::new());
    api.1 = Some(partial);
    let initial_accent = state.preferences.accent;
    let engine = Engine {
        root: root.clone(),
        state,
        models: vec![],
        store: Some(store),
        api,
        jobs: execution::Jobs { web_region_ui: args.is_empty(), ..Default::default() },
    };
    let (tx, input) = channel::unbounded_channel();
    let (output, rx) = mpsc::channel();
    if background {
        return runtime.block_on(engine.run(input, output, true));
    }
    // The owner of the workspace also answers the CLI and MCP (control.rs). Every view the
    // engine makes goes to the control channel and on to the window or terminal.
    let control = match control::serve(&root, tx.clone(), serving) {
        Ok(c) => Some(c),
        Err(e) if serving => return Err(e),
        Err(_) => None,
    };
    let (engine_out, engine_views) = mpsc::channel::<engine::View>();
    let forward_control = control.clone();
    let desktop_context = std::sync::Arc::new(std::sync::OnceLock::<eframe::egui::Context>::new());
    let repaint = desktop_context.clone();
    thread::spawn(move || {
        #[cfg(target_os = "android")]
        let mut delivered_notice = String::new();
        for view in engine_views {
            if let Some(c) = &forward_control {
                c.publish(&view);
            }
            // A long Android session can run while the renderer is suspended. Keep
            // one-shot UI receipts and send a fresh snapshot when the activity returns.
            #[cfg(target_os = "android")]
            if !android_background::activity().visible
                && view.composer_receipt.is_none()
                && (delivered_notice == view.notice
                    || (view.notice != "Plan revision queued" && !view.notice.starts_with("Scheduled #")))
            {
                continue;
            }
            #[cfg(target_os = "android")]
            delivered_notice.clone_from(&view.notice);
            if !serving {
                if output.send(view).is_err() { break; }
                if let Some(ctx) = repaint.get() { ctx.request_repaint(); }
            }
        }
        if let Some(ctx) = repaint.get() { ctx.request_repaint(); }
    });
    let desktop_shutdown = std::sync::Arc::new(desktop::DesktopShutdown::default());
    let shutdown = desktop_shutdown.clone();
    let ui_runtime = runtime.handle().clone();
    let runner = thread::spawn(move || {
        let result = runtime.block_on(engine.run(input, engine_out, false));
        drop(runtime);
        if result.is_ok() {
            shutdown.finish();
        }
        #[cfg(any(windows, target_os = "linux"))]
        if let Err(error) = &result {
            shutdown.cancel_update_restart(error);
        }
        result
    });
    if service {
        // The background service: never stops on its own; the tray's Exit ends it.
        let c = control.clone().unwrap();
        let exe = std::env::current_exe().ok();
        let open_root = root.clone();
        let tray_tx = tx.clone();
        let tray = tray::start(move |a| match a {
            tray::Action::Open => {
                // The window takes over the workspace from the service when it's idle.
                if let Some(exe) = &exe {
                    let _ = std::process::Command::new(exe)
                        .current_dir(&open_root)
                        .spawn();
                }
            }
            tray::Action::Pause => {
                let _ = tray_tx.send("/sync pause".into());
            }
            tray::Action::Resume => {
                let _ = tray_tx.send("/sync resume".into());
            }
            tray::Action::Exit => {
                let _ = tray_tx.send("/exit".into());
            }
        });
        loop {
            thread::sleep(std::time::Duration::from_secs(2));
            if runner.is_finished() {
                break;
            }
            if let (Some(t), Ok(v)) = (&tray, c.latest()) {
                t.set(tray::status_of(&v));
            }
        }
        drop(tray);
        let result = runner
            .join()
            .map_err(|_| anyhow::anyhow!("Session engine stopped unexpectedly"));
        c.close();
        return result?.context("Session engine");
    }
    if serving {
        // Windowless: stop after a few minutes with no requests and no work.
        let c = control.clone().unwrap();
        let idle = std::env::var("SPINDLE_SERVE_IDLE")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(300);
        loop {
            thread::sleep(std::time::Duration::from_secs(5));
            if runner.is_finished() {
                break;
            }
            let quiet = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
                .saturating_sub(c.last_used.load(std::sync::atomic::Ordering::Relaxed));
            if quiet >= idle && control::idle(&c) {
                let _ = tx.send("/exit".into());
                break;
            }
        }
        let result = runner
            .join()
            .map_err(|_| anyhow::anyhow!("Session engine stopped unexpectedly"));
        c.close();
        return result?.context("Session engine");
    }
    let desktop_result = if !args.is_empty() {
        println!("Spindle · /help for commands · /exit to close");
        thread::spawn(move || {
            for msg in io::stdin().lock().lines().map_while(Result::ok) {
                let exit = msg.trim() == "/exit";
                if tx.send(msg).is_err() || exit {
                    break;
                }
            }
        });
        for view in rx {
            println!("{}", view.notice);
        }
        Ok(())
    } else {
        let close = tx.clone();
        let workspace = root.clone();
        let shutdown = desktop_shutdown.clone();
        let options = eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_icon(desktop::spindle_icon(initial_accent))
                .with_inner_size([1180., 800.])
                .with_min_inner_size([820., 550.]),
            #[cfg(target_os = "android")]
            android_app: android::APP.get().cloned(),
            ..Default::default()
        };
        let ui = eframe::run_native(
            "Spindle",
            options,
            Box::new(move |cc| {
                let ctx = cc.egui_ctx.clone();
                let _ = desktop_context.set(ctx.clone());
                let mut stream = preview.clone();
                ui_runtime.spawn(async move {
                    // Wake the UI for new text, coalescing providers that send many small chunks.
                    while stream.changed().await.is_ok() {
                        tokio::time::sleep(std::time::Duration::from_millis(33)).await;
                        drop(stream.borrow_and_update());
                        #[cfg(target_os = "android")]
                        if !android_background::activity().visible { continue; }
                        ctx.request_repaint();
                    }
                });
                let mut app = desktop::Desktop::new(
                    cc,
                    workspace,
                    tx,
                    rx,
                    preview,
                    initial_accent,
                );
                app.attach_shutdown(cc, shutdown);
                Ok(Box::new(app))
            }),
        );
        let _ = close.send("/exit".into());
        ui.map_err(|e| anyhow::anyhow!("{e}"))
    };
    let result = runner
        .join()
        .map_err(|_| anyhow::anyhow!("Session engine stopped unexpectedly"));
    if let Some(c) = &control {
        c.close();
    }
    desktop_result?;
    result?.context("Session engine")?;
    #[cfg(any(windows, target_os = "linux"))]
    if desktop_shutdown.restarting() {
        updates::restart(&root)?;
    }
    Ok(())
}
