//! Android: scan a pairing QR code with the camera, inside Spindle.
//!
//! The NDK camera (Camera2) streams small greyscale frames into an image reader; the screen
//! shows them as a preview, and a background thread looks for a QR code in the latest one
//! (`rqrr`). The camera permission is asked for the first time, through the activity.

use anyhow::{Context, Result, bail};
use ndk_sys::*;
use std::ffi::{CStr, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

#[link(name = "camera2ndk")]
unsafe extern "C" {}
#[link(name = "mediandk")]
unsafe extern "C" {}

// Sharp enough to read a QR code from arm's length; the preview is shrunk.
const WIDTH: i32 = 1280;
const HEIGHT: i32 = 720;

/// Where the scanner is.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Off,
    /// Waiting for the person to allow the camera.
    Asking,
    Running,
    Failed(String),
    /// What the code said.
    Found(String),
}

/// A greyscale frame, already turned upright for the screen.
pub struct Preview {
    pub width: usize,
    pub height: usize,
    pub grey: Vec<u8>,
}

struct Camera {
    manager: *mut ACameraManager,
    device: *mut ACameraDevice,
    reader: *mut AImageReader,
    container: *mut ACaptureSessionOutputContainer,
    output: *mut ACaptureSessionOutput,
    target: *mut ACameraOutputTarget,
    request: *mut ACaptureRequest,
    session: *mut ACameraCaptureSession,
    // The callbacks must outlive the device and session.
    _device_cb: Box<ACameraDevice_StateCallbacks>,
    _session_cb: Box<ACameraCaptureSession_stateCallbacks>,
    /// The frame the decoder looks at next, and whether to stop.
    slot: Arc<(Mutex<Option<(usize, usize, Vec<u8>)>>, Condvar)>,
    found: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
}

// The raw handles are only used from the UI thread, behind the mutex below.
unsafe impl Send for Camera {}

static STATE: Mutex<(State, Option<Camera>)> = Mutex::new((State::Off, None));

pub fn state() -> State {
    STATE.lock().unwrap().0.clone()
}

/// Start scanning: ask for the camera first if needed.
pub fn begin() {
    let mut g = STATE.lock().unwrap();
    if !matches!(g.0, State::Off | State::Failed(_) | State::Found(_)) {
        return;
    }
    if crate::android::camera_allowed() {
        g.0 = State::Running;
        match unsafe { open() } {
            Ok(c) => g.1 = Some(c),
            Err(e) => g.0 = State::Failed(format!("{e:#}")),
        }
    } else {
        crate::android::ask_camera();
        g.0 = State::Asking;
    }
}

/// Stop scanning and close the camera.
pub fn end() {
    let mut g = STATE.lock().unwrap();
    g.1 = None;
    g.0 = State::Off;
}

/// Each frame of the UI: open the camera once allowed, take the latest picture for the
/// preview and the decoder, and notice a code.
pub fn tick() -> Option<Preview> {
    let mut g = STATE.lock().unwrap();
    if g.0 == State::Asking && crate::android::camera_allowed() {
        g.0 = State::Running;
        match unsafe { open() } {
            Ok(c) => g.1 = Some(c),
            Err(e) => g.0 = State::Failed(format!("{e:#}")),
        }
    }
    let found = g.1.as_ref()?.found.lock().unwrap().take();
    if let Some(text) = found {
        g.1 = None;
        g.0 = State::Found(text);
        return None;
    }
    let cam = g.1.as_ref()?;
    let (w, h, grey) = unsafe { latest(cam.reader) }?;
    {
        let (lock, cv) = &*cam.slot;
        *lock.lock().unwrap() = Some((w, h, grey.clone()));
        cv.notify_one();
    }
    // The back camera's sensor lies sideways: turn the picture a quarter clockwise, shrunk
    // to about 400 pixels across, for the screen.
    let step = (h / 400).max(1);
    let (pw, ph) = (h / step, w / step);
    let mut out = vec![0u8; pw * ph];
    for y in 0..ph {
        for x in 0..pw {
            let sx = (y * step).min(w - 1);
            let sy = h - 1 - (x * step).min(h - 1);
            out[y * pw + x] = grey[sy * w + sx];
        }
    }
    Some(Preview {
        width: pw,
        height: ph,
        grey: out,
    })
}

/// The newest picture as greyscale (the Y plane), if one has arrived.
unsafe fn latest(reader: *mut AImageReader) -> Option<(usize, usize, Vec<u8>)> {
    unsafe {
        let mut image: *mut AImage = null_mut();
        if AImageReader_acquireLatestImage(reader, &mut image).0 != 0 || image.is_null() {
            return None;
        }
        let (mut w, mut h, mut stride) = (0i32, 0i32, 0i32);
        let (mut data, mut len) = (null_mut::<u8>(), 0i32);
        AImage_getWidth(image, &mut w);
        AImage_getHeight(image, &mut h);
        AImage_getPlaneRowStride(image, 0, &mut stride);
        AImage_getPlaneData(image, 0, &mut data, &mut len);
        let ok = !data.is_null() && w > 0 && h > 0 && stride >= w && len >= stride * (h - 1) + w;
        let out = ok.then(|| {
            let src = std::slice::from_raw_parts(data, len as usize);
            let (w, h, stride) = (w as usize, h as usize, stride as usize);
            let mut grey = Vec::with_capacity(w * h);
            for row in 0..h {
                grey.extend_from_slice(&src[row * stride..row * stride + w]);
            }
            (w, h, grey)
        });
        AImage_delete(image);
        out
    }
}

unsafe extern "C" fn device_event(_: *mut c_void, _: *mut ACameraDevice) {}
unsafe extern "C" fn device_error(_: *mut c_void, _: *mut ACameraDevice, _: i32) {}
unsafe extern "C" fn session_event(_: *mut c_void, _: *mut ACameraCaptureSession) {}

fn check(status: camera_status_t, what: &str) -> Result<()> {
    if status.0 != 0 {
        bail!("Couldn't {what} (camera error {})", status.0);
    }
    Ok(())
}

/// The back camera's ID, or the first camera.
unsafe fn pick(manager: *mut ACameraManager) -> Result<std::ffi::CString> {
    unsafe {
        let mut list: *mut ACameraIdList = null_mut();
        check(
            ACameraManager_getCameraIdList(manager, &mut list),
            "list the cameras",
        )?;
        let ids: Vec<std::ffi::CString> = (0..(*list).numCameras as usize)
            .map(|i| CStr::from_ptr(*(*list).cameraIds.add(i)).to_owned())
            .collect();
        ACameraManager_deleteCameraIdList(list);
        let mut chosen = None;
        for id in &ids {
            let mut meta: *mut ACameraMetadata = null_mut();
            if ACameraManager_getCameraCharacteristics(manager, id.as_ptr(), &mut meta).0 != 0 {
                continue;
            }
            let mut entry: ACameraMetadata_const_entry = std::mem::zeroed();
            let back = ACameraMetadata_getConstEntry(
                meta,
                acamera_metadata_tag::ACAMERA_LENS_FACING.0,
                &mut entry,
            )
            .0 == 0
                && entry.count > 0
                && *entry.data.u8_ as u32
                    == acamera_metadata_enum_acamera_lens_facing::ACAMERA_LENS_FACING_BACK.0;
            ACameraMetadata_free(meta);
            if back {
                chosen = Some(id.clone());
                break;
            }
        }
        chosen
            .or_else(|| ids.first().cloned())
            .context("This phone has no camera")
    }
}

unsafe fn open() -> Result<Camera> {
    unsafe {
        let manager = ACameraManager_create();
        anyhow::ensure!(!manager.is_null(), "No camera service");
        let mut cam = Camera {
            manager,
            device: null_mut(),
            reader: null_mut(),
            container: null_mut(),
            output: null_mut(),
            target: null_mut(),
            request: null_mut(),
            session: null_mut(),
            _device_cb: Box::new(ACameraDevice_StateCallbacks {
                context: null_mut(),
                onDisconnected: Some(device_event),
                onError: Some(device_error),
            }),
            _session_cb: Box::new(ACameraCaptureSession_stateCallbacks {
                context: null_mut(),
                onClosed: Some(session_event),
                onReady: Some(session_event),
                onActive: Some(session_event),
            }),
            slot: Default::default(),
            found: Default::default(),
            stop: Default::default(),
        };
        // From here on, dropping `cam` closes whatever was opened.
        let id = pick(manager)?;
        check(
            ACameraManager_openCamera(
                manager,
                id.as_ptr(),
                &mut *cam._device_cb as *mut _,
                &mut cam.device,
            ),
            "open the camera",
        )?;
        let status = AImageReader_new(
            WIDTH,
            HEIGHT,
            AIMAGE_FORMATS::AIMAGE_FORMAT_YUV_420_888.0 as i32,
            4,
            &mut cam.reader,
        );
        anyhow::ensure!(
            status.0 == 0,
            "Couldn't read from the camera ({})",
            status.0
        );
        let mut window: *mut ANativeWindow = null_mut();
        anyhow::ensure!(
            AImageReader_getWindow(cam.reader, &mut window).0 == 0,
            "Couldn't read from the camera"
        );
        check(
            ACaptureSessionOutputContainer_create(&mut cam.container),
            "set up the camera",
        )?;
        check(
            ACaptureSessionOutput_create(window, &mut cam.output),
            "set up the camera",
        )?;
        check(
            ACaptureSessionOutputContainer_add(cam.container, cam.output),
            "set up the camera",
        )?;
        check(
            ACameraOutputTarget_create(window, &mut cam.target),
            "set up the camera",
        )?;
        check(
            ACameraDevice_createCaptureRequest(
                cam.device,
                ACameraDevice_request_template::TEMPLATE_PREVIEW,
                &mut cam.request,
            ),
            "set up the camera",
        )?;
        check(
            ACaptureRequest_addTarget(cam.request, cam.target),
            "set up the camera",
        )?;
        // Keep focusing as the phone moves (the preview template may leave it fixed). Not
        // every camera has autofocus: a refusal here isn't an error.
        let af = acamera_metadata_enum_acamera_control_af_mode::ACAMERA_CONTROL_AF_MODE_CONTINUOUS_PICTURE.0 as u8;
        let _ = ACaptureRequest_setEntry_u8(
            cam.request,
            acamera_metadata_tag::ACAMERA_CONTROL_AF_MODE.0,
            1,
            &af,
        );
        check(
            ACameraDevice_createCaptureSession(
                cam.device,
                cam.container,
                &*cam._session_cb as *const _,
                &mut cam.session,
            ),
            "start the camera",
        )?;
        let mut requests = [cam.request];
        check(
            ACameraCaptureSession_setRepeatingRequest(
                cam.session,
                null_mut(),
                1,
                requests.as_mut_ptr(),
                null_mut(),
            ),
            "start the camera",
        )?;
        // Look for a code in the latest frame, as fast as the phone manages.
        let (slot, found, stop) = (cam.slot.clone(), cam.found.clone(), cam.stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let frame = {
                    let (lock, cv) = &*slot;
                    let g = lock.lock().unwrap();
                    let (mut g, _) = cv
                        .wait_timeout(g, std::time::Duration::from_millis(500))
                        .unwrap();
                    g.take()
                };
                let Some((w, h, grey)) = frame else {
                    continue;
                };
                if let Some(text) = decode(w, h, &grey) {
                    *found.lock().unwrap() = Some(text);
                    return;
                }
            }
        });
        Ok(cam)
    }
}

/// The text of a QR code in a greyscale picture, if there is one.
pub fn decode(w: usize, h: usize, grey: &[u8]) -> Option<String> {
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| grey[y * w + x]);
    img.detect_grids()
        .into_iter()
        .find_map(|g| g.decode().ok().map(|(_, text)| text))
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.slot.1.notify_one();
        unsafe {
            if !self.session.is_null() {
                ACameraCaptureSession_close(self.session);
            }
            if !self.request.is_null() {
                ACaptureRequest_free(self.request);
            }
            if !self.target.is_null() {
                ACameraOutputTarget_free(self.target);
            }
            if !self.output.is_null() {
                ACaptureSessionOutput_free(self.output);
            }
            if !self.container.is_null() {
                ACaptureSessionOutputContainer_free(self.container);
            }
            if !self.device.is_null() {
                ACameraDevice_close(self.device);
            }
            if !self.reader.is_null() {
                AImageReader_delete(self.reader);
            }
            if !self.manager.is_null() {
                ACameraManager_delete(self.manager);
            }
        }
    }
}
