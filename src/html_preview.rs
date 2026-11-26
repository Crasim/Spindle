//! A local, read-only origin for a preview and its relative assets.

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
};
use std::{
    io::Read,
    net::TcpListener,
    path::{Path, PathBuf},
    sync::Arc,
};

const HOST: &str = "spindle-preview.invalid";
const MAX_ASSET: u64 = 16 * 1024 * 1024;
const CSP: &str = "sandbox allow-scripts; default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' data: blob:; font-src 'self' data:; connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'";

pub fn is_html(path: &Path) -> bool {
    matches!(crate::documents::extension(path).as_str(), "html" | "htm")
}

struct Page {
    token: String,
    entry: String,
    source: String,
    directory: Option<PathBuf>,
    address: String,
}

pub struct Server {
    pub address: String,
    pub scope: String,
    pub url: String,
    pub app_url: String,
    _stop: tokio::sync::oneshot::Sender<()>,
}

impl Server {
    pub fn start(title: &str, source: String, disk: Option<&Path>) -> Result<Self> {
        use ring::rand::SecureRandom;
        let mut random = [0; 24];
        ring::rand::SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| anyhow::anyhow!("Could not create a private preview URL"))?;
        let token = crate::sync::core::hex(&random);
        let entry = disk
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            .or_else(|| Path::new(title).file_name().and_then(|n| n.to_str()))
            .filter(|n| !n.starts_with('.') && is_html(Path::new(n)))
            .unwrap_or("index.html")
            .to_owned();
        let directory = disk
            .and_then(Path::parent)
            .map(Path::canonicalize)
            .transpose()?;
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).context("Open the local preview server")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?.to_string();
        let scope = format!("http://{address}/{token}/");
        let url = entry_url(&address, &token, &entry)?;
        let app_url = entry_url(HOST, &token, &entry)?;
        let (stop, closed) = tokio::sync::oneshot::channel();
        let page = Arc::new(Page {
            token,
            entry,
            source,
            directory,
            address: address.clone(),
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        std::thread::Builder::new()
            .name("html-preview".into())
            .spawn(move || {
                runtime.block_on(async move {
                    if let Ok(listener) = tokio::net::TcpListener::from_std(listener) {
                        let router = Router::new().fallback(serve).with_state(page);
                        let _ = axum::serve(listener, router)
                            .with_graceful_shutdown(async {
                                let _ = closed.await;
                            })
                            .await;
                    }
                });
            })?;
        Ok(Self {
            address,
            scope,
            url,
            app_url,
            _stop: stop,
        })
    }
}

fn entry_url(host: &str, token: &str, entry: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(&format!("http://{host}/{token}/"))?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("Invalid preview URL"))?
        .pop_if_empty()
        .push(entry);
    Ok(url.into())
}

async fn serve(State(page): State<Arc<Page>>, req: Request<Body>) -> Response<Body> {
    let host = req
        .uri()
        .authority()
        .map(|a| a.as_str())
        .or_else(|| req.headers().get("host").and_then(|v| v.to_str().ok()));
    let is_proxy = req.uri().scheme_str() == Some("http") && host == Some(HOST);
    let is_local = req.uri().scheme().is_none() && host == Some(page.address.as_str());
    let asset = if req.method() == axum::http::Method::GET && (is_local || is_proxy) {
        let prefix = format!("/{}/", page.token);
        req.uri()
            .path()
            .strip_prefix(&prefix)
            .and_then(|path| page.asset(path))
    } else {
        None
    };
    let (status, kind, bytes) = match asset {
        Some((kind, bytes)) => (StatusCode::OK, kind, bytes),
        None => (
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            b"This resource is outside the preview or unavailable.".to_vec(),
        ),
    };
    Response::builder()
        .status(status)
        .header("Content-Type", kind)
        .header("Content-Security-Policy", CSP)
        .header("Access-Control-Allow-Origin", "*")
        .header("Cache-Control", "no-store")
        .header("Referrer-Policy", "no-referrer")
        .header("X-DNS-Prefetch-Control", "off")
        .header("X-Content-Type-Options", "nosniff")
        .header("Connection", "close")
        .body(Body::from(bytes))
        .expect("Static preview response headers")
}

impl Page {
    fn asset(&self, path: &str) -> Option<(&'static str, Vec<u8>)> {
        let mut relative = PathBuf::new();
        for part in path.split('/') {
            let name = decode(part)?;
            if name.is_empty()
                || name.starts_with('.')
                || name.contains(['/', '\\', ':'])
                || name.chars().any(char::is_control)
            {
                return None;
            }
            relative.push(name);
        }
        if relative == Path::new(&self.entry) {
            return Some(("text/html; charset=utf-8", self.source.as_bytes().to_vec()));
        }
        let kind = match crate::documents::extension(&relative).as_str() {
            "html" | "htm" => "text/html; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            "js" | "mjs" => "text/javascript; charset=utf-8",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "webp" => "image/webp",
            "avif" => "image/avif",
            "woff" => "font/woff",
            "woff2" => "font/woff2",
            "mp3" => "audio/mpeg",
            "wav" => "audio/wav",
            "mp4" => "video/mp4",
            "webm" => "video/webm",
            _ => return None,
        };
        let candidate = self.directory.as_ref()?.join(relative);
        let actual = candidate.canonicalize().ok()?;
        if actual != candidate || !actual.is_file() {
            return None;
        }
        let file = std::fs::File::open(actual).ok()?;
        let mut bytes = Vec::new();
        file.take(MAX_ASSET + 1).read_to_end(&mut bytes).ok()?;
        (bytes.len() as u64 <= MAX_ASSET).then_some((kind, bytes))
    }
}

pub fn decode(part: &str) -> Option<String> {
    let mut out = Vec::with_capacity(part.len());
    let mut bytes = part.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = char::from(bytes.next()?).to_digit(16)?;
            let lo = char::from(bytes.next()?).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(target_os = "linux")]
pub struct AppWindow {
    pub child: std::process::Child,
    _profile: tempfile::TempDir,
}

#[cfg(target_os = "linux")]
pub(crate) fn configure_proxy(command: &mut std::process::Command, server: &Server) {
    // The proxy itself serves this preview and has no upstream connection. Subtract
    // Chromium's implicit loopback exceptions so previews cannot reach other local services.
    command.arg(format!("--proxy-server=http://{}", server.address)).args([
        "--proxy-bypass-list=<-loopback>",
        "--dns-prefetch-disable",
        "--disable-quic",
        "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
    ]);
}

#[cfg(target_os = "linux")]
impl AppWindow {
    pub fn open(configured: &str, server: &Server) -> Result<Self> {
        use std::process::{Command, Stdio};
        anyhow::ensure!(
            unsafe { libc::geteuid() } != 0,
            "Run Spindle as a regular user to launch the isolated preview browser"
        );
        let exe = crate::browser::resolve(configured)?;
        let profile = tempfile::tempdir()?;
        let mut command = Command::new(exe);
        configure_proxy(&mut command, server);
        let child = command
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-sync",
                "--disable-background-networking",
                "--disable-background-mode",
                "--disable-component-update",
                "--disable-default-apps",
            ])
            .arg(format!("--user-data-dir={}", profile.path().display()))
            .arg(format!("--app={}", server.app_url))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("Open the Chromium preview window")?;
        Ok(Self {
            child,
            _profile: profile,
        })
    }
}

#[cfg(target_os = "linux")]
impl Drop for AppWindow {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn preview_serves_linked_assets_and_denies_other_resources() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("page");
        std::fs::create_dir(&folder).unwrap();
        let entry = folder.join("Snake Game.HTML");
        let html = r#"<link rel="stylesheet" href="theme.css"><script src="game.js"></script><a href="next.html">Next</a>"#;
        std::fs::write(&entry, html).unwrap();
        std::fs::write(folder.join("game.js"), "window.ready = true;").unwrap();
        std::fs::write(folder.join("next.html"), "<title>Next</title>").unwrap();
        std::fs::write(folder.join(".env"), "PRIVATE").unwrap();
        std::fs::write(dir.path().join("outside.html"), "PRIVATE").unwrap();
        let server = Server::start("Snake Game.HTML", html.to_owned(), Some(&entry)).unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client.get(&server.url).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-security-policy"], CSP);
        assert_eq!(response.text().await.unwrap(), html);
        for file in ["game.js", "next.html"] {
            let response = client
                .get(format!("{}{file}", server.scope))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        for file in [".env", "%2e%2e%2foutside.html", "missing.html"] {
            let response = client
                .get(format!("{}{file}", server.scope))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        let proxy = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(format!("http://{}", server.address)).unwrap())
            .build()
            .unwrap();
        assert_eq!(
            proxy.get(&server.app_url).send().await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            proxy
                .get("http://example.com/")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}
