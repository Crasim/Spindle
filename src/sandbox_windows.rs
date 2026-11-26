//! The Windows launcher: an AppContainer with no capabilities (so no network and no access to
//! the user's files) inside a Job Object (memory and process limits, and every process ends
//! with the job).
//!
//! The AppContainer is given read and execute access to Spindle's Python and full access to the
//! working folder, by adding its SID to those folders' ACLs. Nothing else is granted.

use super::{Outcome, Spec, drain, launch_environment};
use anyhow::{Result, bail};
use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::FromRawHandle;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_SUCCESS, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, LocalFree,
    SetHandleInformation, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ACCESS_MODE, ConvertStringSecurityDescriptorToSecurityDescriptorW, EXPLICIT_ACCESS_W,
    GRANT_ACCESS, GetNamedSecurityInfoW, NO_MULTIPLE_TRUSTEE, REVOKE_ACCESS, SE_FILE_OBJECT,
    SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP,
    TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, FreeSid, GetSecurityDescriptorSacl, LABEL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES,
    SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
    InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
};

const PROFILE: &str = "Spindle.Sandbox";
/// HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)
const ALREADY_EXISTS: i32 = 0x800700B7_u32 as i32;

fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Closes a handle when dropped.
struct Owned(HANDLE);
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the handle is owned and closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// The AppContainer's SID, freed when dropped.
struct Sid(*mut c_void);
impl Drop for Sid {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by the AppContainer APIs, freed once.
            unsafe { FreeSid(self.0) };
        }
    }
}

fn container(name: &[u16], fresh: bool) -> Result<Sid> {
    let display = wide("Spindle sandbox");
    let mut sid: *mut c_void = std::ptr::null_mut();
    // SAFETY: valid null-terminated strings; no capabilities are requested.
    let hr = unsafe {
        CreateAppContainerProfile(
            name.as_ptr(),
            display.as_ptr(),
            display.as_ptr(),
            std::ptr::null(),
            0,
            &mut sid as *mut _ as _,
        )
    };
    if hr == ALREADY_EXISTS && !fresh {
        // SAFETY: as above.
        let hr = unsafe {
            DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid as *mut _ as _)
        };
        if hr < 0 {
            bail!(
                "Couldn't open the Spindle AppContainer (0x{:08x})",
                hr as u32
            );
        }
    } else if hr < 0 {
        bail!(
            "Couldn't create the Spindle AppContainer (0x{:08x})",
            hr as u32
        );
    }
    Ok(Sid(sid))
}

/// Add an inheritable ACE for the AppContainer to a folder.
fn acl(path: &Path, sid: &Sid, mode: ACCESS_MODE, access: u32) -> Result<()> {
    let name = wide(path);
    let mut old: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: out-pointers are valid; the descriptor is freed below.
    let err = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr() as _,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut old,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if err != ERROR_SUCCESS {
        bail!(
            "Couldn't read the permissions of {} ({err})",
            path.display()
        );
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access,
        grfAccessMode: mode,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
            ptstrName: sid.0 as _,
        },
    };
    let mut new: *mut ACL = std::ptr::null_mut();
    // SAFETY: one valid entry, the existing ACL, and an out-pointer freed below.
    let err = unsafe { SetEntriesInAclW(1, &entry, old, &mut new) };
    if err != ERROR_SUCCESS {
        // SAFETY: allocated by GetNamedSecurityInfoW.
        unsafe { LocalFree(descriptor as _) };
        bail!(
            "Couldn't prepare permissions for {} ({err})",
            path.display()
        );
    }
    // SAFETY: valid path and ACL.
    let err = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr() as _,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new,
            std::ptr::null(),
        )
    };
    // SAFETY: both were allocated by the calls above.
    unsafe {
        LocalFree(new as _);
        LocalFree(descriptor as _);
    }
    if err != ERROR_SUCCESS {
        bail!(
            "Couldn't give the sandbox access to {} ({err})",
            path.display()
        );
    }
    Ok(())
}

/// Allow writes from a low-integrity process in an already scoped work directory.
fn label_work(path: &Path) -> Result<()> {
    let label = wide("S:(ML;OICI;NW;;;LW)");
    let name = wide(path);
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: the input is a constant SDDL string and the out-pointer is valid.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            label.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        bail!(
            "Couldn't prepare the work folder's integrity label ({})",
            unsafe { GetLastError() }
        );
    }
    let mut present = 0;
    let mut defaulted = 0;
    let mut sacl = std::ptr::null_mut();
    // SAFETY: the parsed label is alive through SetNamedSecurityInfoW.
    let err = unsafe {
        let err = if GetSecurityDescriptorSacl(descriptor, &mut present, &mut sacl, &mut defaulted)
            != 0
            && present != 0
            && !sacl.is_null()
        {
            SetNamedSecurityInfoW(
                name.as_ptr() as _,
                SE_FILE_OBJECT,
                LABEL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                sacl,
            )
        } else {
            GetLastError()
        };
        LocalFree(descriptor as _);
        err
    };
    if err != ERROR_SUCCESS {
        bail!("Couldn't label the sandbox work folder ({err})");
    }
    Ok(())
}

/// Marks a folder already opened to the sandbox. Windows copies an inheritable permission onto
/// every file below the folder, which is slow for the runtime, so it is done once per folder.
pub const ACCESS_MARKER: &str = ".spindle-access";

fn grant_once(path: &Path, sid: &Sid, access: u32) -> Result<()> {
    let marker = path.join(ACCESS_MARKER);
    if marker.is_file() {
        return Ok(());
    }
    acl(path, sid, GRANT_ACCESS, access)?;
    std::fs::write(marker, "The Spindle sandbox can use this folder.\n")?;
    Ok(())
}

fn pipe(inherit_read: bool) -> Result<(Owned, Owned)> {
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let (mut read, mut write): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: valid out-pointers.
    if unsafe { CreatePipe(&mut read, &mut write, &attrs, 0) } == 0 {
        bail!("Couldn't create a pipe ({})", unsafe { GetLastError() });
    }
    let (read, write) = (Owned(read), Owned(write));
    let parent_end = if inherit_read { write.0 } else { read.0 };
    // SAFETY: the chosen end is owned by the launcher and must not be inherited.
    if unsafe { SetHandleInformation(parent_end, HANDLE_FLAG_INHERIT, 0) } == 0 {
        bail!("Couldn't prepare a sandbox pipe ({})", unsafe {
            GetLastError()
        });
    }
    Ok((read, write))
}

pub(crate) fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

struct PrivateProfile<'a> {
    name: Vec<u16>,
    sid: Sid,
    readonly: Vec<&'a Path>,
}

impl Drop for PrivateProfile<'_> {
    fn drop(&mut self) {
        for path in &self.readonly {
            let _ = acl(path, &self.sid, REVOKE_ACCESS, 0);
        }
        // The job and its handles are closed before the profile goes away.
        unsafe { DeleteAppContainerProfile(self.name.as_ptr()) };
    }
}

pub fn run(spec: &Spec, cancel: &Arc<AtomicBool>, private: bool) -> Result<Outcome> {
    if private {
        let package = format!(
            "spindle.native.{}",
            crate::sync::core::hex(&crate::vault::random::<12>()?)
        );
        // Windows rewrites TEMP and TMP below LOCALAPPDATA for a lowbox process.
        // Its private LOCALAPPDATA is in this job, so provision the redirected directory.
        std::fs::create_dir_all(
            spec.work
                .join(".tmp")
                .join("Packages")
                .join(&package)
                .join("AC")
                .join("Temp"),
        )?;
        let name = wide(package);
        let sid = container(&name, true)?;
        let mut profile = PrivateProfile {
            name,
            sid,
            readonly: vec![],
        };
        for dir in &spec.readonly {
            profile.readonly.push(dir);
            acl(
                dir,
                &profile.sid,
                GRANT_ACCESS,
                FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
            )?;
        }
        profile.readonly.push(&spec.work);
        acl(&spec.work, &profile.sid, GRANT_ACCESS, FILE_ALL_ACCESS)?;
        label_work(&spec.work)?;
        return run_in(spec, cancel, &profile.sid, true);
    }
    let sid = container(&wide(PROFILE), false)?;
    for dir in &spec.readonly {
        grant_once(dir, &sid, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE)?;
    }
    grant_once(&spec.work, &sid, FILE_ALL_ACCESS)?;
    label_work(&spec.work)?;
    run_in(spec, cancel, &sid, false)
}

fn run_in(spec: &Spec, cancel: &Arc<AtomicBool>, sid: &Sid, private: bool) -> Result<Outcome> {
    // SAFETY: no name, default security.
    let job = Owned(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
    if job.0.is_null() {
        bail!("Couldn't create a job object ({})", unsafe {
            GetLastError()
        });
    }
    // SAFETY: a zeroed limit structure is valid; fields are set below.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_JOB_MEMORY
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    limits.BasicLimitInformation.ActiveProcessLimit = spec.max_processes;
    limits.JobMemoryLimit = (spec.memory_mb * 1024 * 1024) as usize;
    // SAFETY: the structure matches the information class.
    let ok = unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        bail!("Couldn't set the job limits ({})", unsafe {
            GetLastError()
        });
    }

    let (out_read, out_write) = pipe(false)?;
    let (err_read, err_write) = pipe(false)?;
    // Supply an already closed input stream. Some tools open the Windows NUL device when
    // stdin is missing, and AppContainer does not have access to that device.
    let (in_read, in_write) = pipe(true)?;
    drop(in_write);

    let mut capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: sid.0 as _,
        Capabilities: std::ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let inherit: [HANDLE; 3] = [in_read.0, out_write.0, err_write.0];
    let mut size = 0usize;
    // SAFETY: the first call only reports the size needed.
    unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 2, 0, &mut size) };
    let mut list = vec![0u8; size];
    let attrs = list.as_mut_ptr() as *mut c_void;
    // SAFETY: the buffer has the reported size and lives until after CreateProcessW.
    if unsafe { InitializeProcThreadAttributeList(attrs as _, 2, 0, &mut size) } == 0 {
        bail!("Couldn't prepare the process attributes ({})", unsafe {
            GetLastError()
        });
    }
    // SAFETY: values outlive the attribute list's use.
    let ok = unsafe {
        UpdateProcThreadAttribute(
            attrs as _,
            0,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
            &mut capabilities as *mut _ as *const c_void,
            std::mem::size_of::<SECURITY_CAPABILITIES>(),
            std::ptr::null_mut(),
            std::ptr::null(),
        ) != 0
            && UpdateProcThreadAttribute(
                attrs as _,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                inherit.as_ptr() as *const c_void,
                std::mem::size_of_val(&inherit),
                std::ptr::null_mut(),
                std::ptr::null(),
            ) != 0
    };
    if !ok {
        unsafe { DeleteProcThreadAttributeList(attrs as _) };
        bail!("Couldn't set the sandbox attributes ({})", unsafe {
            GetLastError()
        });
    }

    // SAFETY: zeroed structures are valid starting points for these Win32 types.
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = in_read.0;
    startup.StartupInfo.hStdOutput = out_write.0;
    startup.StartupInfo.hStdError = err_write.0;
    startup.lpAttributeList = attrs as _;

    let mut line = quote(&crate::projects::display_path(&spec.program));
    for a in &spec.args {
        line.push(' ');
        line.push_str(&quote(a));
    }
    let mut line = wide(line);
    let mut env: Vec<u16> = vec![];
    for (k, v) in launch_environment(spec, private) {
        env.extend(OsStr::new(&format!("{k}={v}")).encode_wide());
        env.push(0);
    }
    env.push(0);
    let program = wide(&spec.program);
    let dir = wide(&spec.work);
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let started = Instant::now();
    // SAFETY: all pointers are valid for the call; the process starts suspended so it can be
    // placed in the job before it runs any code.
    let created = unsafe {
        CreateProcessW(
            program.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_SUSPENDED
                | CREATE_NO_WINDOW
                | CREATE_UNICODE_ENVIRONMENT,
            env.as_ptr() as *const c_void,
            dir.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    };
    unsafe { DeleteProcThreadAttributeList(attrs as _) };
    drop(list);
    drop(in_read);
    if created == 0 {
        bail!("Couldn't start the sandboxed program ({})", unsafe {
            GetLastError()
        });
    }
    let process = Owned(info.hProcess);
    let thread = Owned(info.hThread);
    // SAFETY: valid job and process handles.
    if unsafe { AssignProcessToJobObject(job.0, process.0) } == 0 {
        unsafe { TerminateJobObject(job.0, 1) };
        // The process is suspended and not in the job: end it before reporting.
        unsafe { windows_sys::Win32::System::Threading::TerminateProcess(process.0, 1) };
        bail!("Couldn't place the program in its job ({})", unsafe {
            GetLastError()
        });
    }
    // SAFETY: the main thread of the process we created.
    unsafe { ResumeThread(thread.0) };
    drop(thread);
    // The child holds its own copies of the write ends; close ours so reads end with the child.
    drop(out_write);
    drop(err_write);
    let read_out = out_read.0 as isize;
    let read_err = err_read.0 as isize;
    std::mem::forget(out_read);
    std::mem::forget(err_read);
    // SAFETY: ownership of the read ends moves into the File, which closes them.
    let out =
        std::thread::spawn(move || drain(unsafe { std::fs::File::from_raw_handle(read_out as _) }));
    let err =
        std::thread::spawn(move || drain(unsafe { std::fs::File::from_raw_handle(read_err as _) }));

    let mut outcome = Outcome::default();
    loop {
        // SAFETY: a valid process handle.
        if unsafe { WaitForSingleObject(process.0, 50) } == WAIT_OBJECT_0 {
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(process.0, &mut code) };
            outcome.code = Some(code as i32);
            break;
        }
        let over = started.elapsed() > spec.timeout;
        if over || cancel.load(Ordering::Relaxed) {
            outcome.timed_out = over;
            outcome.cancelled = !over;
            break;
        }
    }
    // Ending the job ends every process the program started, even ones still running.
    // SAFETY: a valid job handle.
    unsafe { TerminateJobObject(job.0, 1) };
    drop(job);
    drop(process);
    outcome.millis = started.elapsed().as_millis() as u64;
    outcome.stdout = out.join().unwrap_or_default();
    outcome.stderr = err.join().unwrap_or_default();
    Ok(outcome)
}
