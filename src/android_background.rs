use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use jni::{
    JNIEnv,
    objects::JObject,
    sys::{jboolean, jlong},
};
use tokio::sync::watch;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Activity {
    pub visible: bool,
    service: bool,
}

impl Activity {
    pub fn can_work(self) -> bool {
        self.visible || self.service
    }

    pub fn notify_reply(self) -> bool {
        self.service && !self.visible
    }
}

static ACTIVITY: LazyLock<watch::Sender<Activity>> =
    LazyLock::new(|| watch::channel(Activity::default()).0);
static OPEN_REPLY: AtomicU64 = AtomicU64::new(u64::MAX);

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_spindleapp_spindle_SpindleActivity_nativeBackgroundState(
    _env: JNIEnv,
    _activity: JObject,
    visible: jboolean,
    service: jboolean,
) {
    ACTIVITY.send_if_modified(|state| {
        let next = Activity {
            visible: visible != 0,
            service: service != 0,
        };
        if *state == next {
            return false;
        }
        *state = next;
        true
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_spindleapp_spindle_SpindleActivity_nativeOpenReply(
    _env: JNIEnv,
    _activity: JObject,
    chat: jlong,
) {
    if chat >= 0 {
        OPEN_REPLY.store(chat as u64, Ordering::Release);
    }
}

pub(crate) fn take_reply_chat() -> Option<u64> {
    let chat = OPEN_REPLY.swap(u64::MAX, Ordering::AcqRel);
    (chat != u64::MAX).then_some(chat)
}

pub(crate) fn activity() -> Activity {
    *ACTIVITY.borrow()
}

pub(crate) fn subscribe() -> watch::Receiver<Activity> {
    ACTIVITY.subscribe()
}

pub(crate) fn reply_ready(chat: u64) {
    if activity().notify_reply() {
        let _ = super::android::java(|env, app| {
            env.call_method(
                app,
                "notifyBackgroundReply",
                "(J)V",
                &[(chat as jlong).into()],
            )?;
            Ok(())
        });
    }
}

/// Keeps the CPU awake while there is active work and the screen is hidden.
/// Android releases the lock on idle, return to the app, or service shutdown.
#[derive(Default)]
pub(crate) struct Work(bool);

impl Work {
    pub fn set(&mut self, busy: bool) {
        if self.0 != busy {
            let _ = super::android::java(|env, app| {
                env.call_method(app, "setBackgroundBusy", "(Z)V", &[busy.into()])?;
                Ok(())
            });
            self.0 = busy;
        }
    }
}

impl Drop for Work {
    fn drop(&mut self) {
        self.set(false);
    }
}

pub(crate) fn settings(ui: &mut eframe::egui::Ui) {
    let flags =
        super::android::java(|env, app| env.call_method(app, "backgroundStatus", "()I", &[])?.i())
            .unwrap_or_default();
    let mut enabled = flags & 1 != 0;
    let active = flags & 2 != 0;
    ui.heading("Background work");
    if ui
        .checkbox(&mut enabled, "Keep Spindle running in the background")
        .changed()
    {
        let _ = super::android::java(|env, app| {
            env.call_method(app, "setBackgroundEnabled", "(Z)V", &[enabled.into()])?;
            Ok(())
        });
    }
    ui.weak("Keep sync and tasks running when you switch apps or lock your screen. Completed replies can notify you while Spindle is out of view.");
    if enabled {
        ui.weak(if active {
            "Background mode is active. Removing Spindle from Recent apps ends the session."
        } else {
            "Android hasn’t started background mode. Turn it off and back on while Spindle is open to retry."
        });
        if flags & 4 == 0 && ui.button("Notification settings").clicked() {
            open_system_settings(true);
        }
        if flags & 8 == 0 {
            ui.weak("Android battery settings can delay sync and replies while the phone sleeps.");
            if ui.button("Android app settings").clicked() {
                open_system_settings(false);
            }
        }
    }
}

fn open_system_settings(notifications: bool) {
    let _ = super::android::java(|env, app| {
        env.call_method(
            app,
            "openBackgroundSettings",
            "(Z)V",
            &[notifications.into()],
        )?;
        Ok(())
    });
}
