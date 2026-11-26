//! Private, per-user IPC between Spindle and its native-messaging host.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_FRAME: usize = 64 * 1024;

pub async fn read(stream: &mut (impl AsyncRead + Unpin)) -> Result<Value> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).await?;
    let length = u32::from_ne_bytes(header) as usize;
    ensure!(
        (1..=MAX_FRAME).contains(&length),
        "Invalid browser message size"
    );
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub async fn write(stream: &mut (impl AsyncWrite + Unpin), value: &Value) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    ensure!(data.len() <= MAX_FRAME, "Browser message is too large");
    stream.write_all(&(data.len() as u32).to_ne_bytes()).await?;
    stream.write_all(&data).await?;
    Ok(())
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{mem::size_of, ptr};
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            },
            GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER, TokenUser,
        },
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    fn pipe(id: &str) -> String {
        format!(r"\\.\pipe\Spindle.Browser.{id}")
    }

    fn account_sid() -> Result<String> {
        let mut token: HANDLE = ptr::null_mut();
        // The token handle is used only to read the current process's account SID.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut length = 0;
        unsafe {
            GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut length);
        }
        let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        };
        let error = std::io::Error::last_os_error();
        unsafe {
            CloseHandle(token);
        }
        if ok == 0 {
            return Err(error.into());
        }
        let mut wide: *mut u16 = ptr::null_mut();
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut wide) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // ConvertSidToStringSidW owns a NUL-terminated allocation until LocalFree.
        let length = (0..).find(|&i| unsafe { *wide.add(i) == 0 }).unwrap();
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(wide, length) });
        unsafe {
            LocalFree(wide.cast());
        }
        Ok(sid?)
    }

    pub struct Listener(NamedPipeServer);

    impl Listener {
        pub fn bind(id: &str) -> Result<Self> {
            let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})", account_sid()?)
                .encode_utf16()
                .chain([0])
                .collect();
            let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
            if unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut descriptor,
                    ptr::null_mut(),
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            // CreateNamedPipe copies this descriptor. The one instance rejects remote clients.
            let result = unsafe {
                ServerOptions::new()
                    .first_pipe_instance(true)
                    .max_instances(1)
                    .reject_remote_clients(true)
                    .create_with_security_attributes_raw(
                        pipe(id),
                        (&mut attributes as *mut SECURITY_ATTRIBUTES).cast(),
                    )
            };
            unsafe {
                LocalFree(descriptor);
            }
            Ok(Self(
                result.context("Could not open the private browser pipe")?,
            ))
        }

        pub async fn accept(self) -> Result<NamedPipeServer> {
            self.0.connect().await?;
            Ok(self.0)
        }
    }

    pub async fn connect(id: &str) -> Result<NamedPipeClient> {
        Ok(ClientOptions::new().open(pipe(id))?)
    }
    pub type Client = NamedPipeClient;
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, MetadataExt},
        path::PathBuf,
    };
    use tokio::net::{UnixListener, UnixStream};

    fn endpoint(id: &str) -> Result<PathBuf> {
        let uid = unsafe { libc::geteuid() };
        let folder = std::env::temp_dir().join(format!("spindle-browser-{uid}"));
        match fs::DirBuilder::new().mode(0o700).create(&folder) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        let meta = fs::symlink_metadata(&folder)?;
        ensure!(
            meta.is_dir() && meta.uid() == uid && meta.mode() & 0o077 == 0,
            "Spindle's browser socket directory must belong to this account and be private"
        );
        Ok(folder.join(format!("{id}.sock")))
    }

    pub struct Listener {
        listener: UnixListener,
        path: PathBuf,
    }

    impl Listener {
        pub fn bind(id: &str) -> Result<Self> {
            let path = endpoint(id)?;
            let listener =
                UnixListener::bind(&path).context("Could not open the private browser socket")?;
            Ok(Self { listener, path })
        }

        pub async fn accept(self) -> Result<UnixStream> {
            Ok(self.listener.accept().await?.0)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    pub async fn connect(id: &str) -> Result<UnixStream> {
        Ok(UnixStream::connect(endpoint(id)?).await?)
    }
    pub type Client = UnixStream;
}

#[cfg(any(windows, target_os = "linux"))]
pub use platform::{Client, Listener, connect};
