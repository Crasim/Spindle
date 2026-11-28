//! Browsing public pages and checking workspace HTML with a local Chromium executable.
//! Spindle uses the DevTools protocol on a local port and creates a fresh profile for each run.

use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const SUPPORTED: bool = cfg!(any(windows, target_os = "linux"));
pub(crate) const POINTER_PNG: &[u8] = include_bytes!("../browser-extension/cursor.png");
const SETTINGS: &str = "Settings → Sandbox → Model browser";

pub(crate) fn activity_appearance() -> Value {
    let [r, g, b, _] = crate::widgets::accent().to_array();
    json!({"accent": format!("#{r:02x}{g:02x}{b:02x}"), "light": crate::widgets::is_light()})
}

pub(crate) fn public_page(address: &str) -> Result<reqwest::Url> {
    #[cfg(any(windows, target_os = "linux", target_os = "android"))]
    { crate::public_web::url(address) }
    #[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
    {
        let _ = address;
        bail!("Public Chromium browsing is available on Windows and Linux")
    }
}

pub(crate) fn country_code(value: &str) -> Option<String> {
    let mut value = value.trim().to_ascii_uppercase();
    if value == "UK" { value = "GB".into(); }
    (value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_uppercase())
        && !matches!(value.as_str(), "XX" | "ZZ")).then_some(value)
}

#[derive(Debug)]
pub(crate) enum PageAccess {
    Http(u16),
    Challenge,
}

impl PageAccess {
    pub(crate) fn offers_regular_browser(&self) -> bool {
        matches!(self, Self::Http(401 | 403 | 429) | Self::Challenge)
    }
}

impl std::fmt::Display for PageAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(401) => f.write_str("The site requires authentication (HTTP 401) in the built-in browser’s isolated session"),
            Self::Http(403) => f.write_str("The site refused the built-in browser’s request (HTTP 403)"),
            Self::Http(429) => f.write_str("The site rate-limited the built-in browser (HTTP 429)"),
            Self::Http(status) => write!(f, "The site returned HTTP {status}"),
            Self::Challenge => f.write_str("The site showed a verification page instead of the requested content"),
        }
    }
}

impl std::error::Error for PageAccess {}

#[cfg(windows)]
pub(crate) fn panel_cursor_script() -> String {
    let css = include_str!("../browser-extension/activity.css");
    let pointer = base64::engine::general_purpose::STANDARD.encode(POINTER_PNG);
    let css = serde_json::to_string(&format!("{css}\n:host {{ --spindle-pointer: url(data:image/png;base64,{pointer}); }}")).unwrap();
    let appearance = activity_appearance();
    format!(r#"if (window === top && !document.querySelector('spindle-browser-activity')) {{
            const host = document.createElement('spindle-browser-activity');
            host.style.cssText = 'all:initial!important;position:fixed!important;inset:0!important;pointer-events:none!important;z-index:2147483647!important';
            const appearance = {appearance};
            host.toggleAttribute('data-light', appearance.light);
            host.style.setProperty('--spindle-accent', appearance.accent);
            host.setAttribute('data-working','');
            const shadow = host.attachShadow({{mode:'closed'}});
            const style = document.createElement('style');
            style.textContent = {css};
            const badge = document.createElement('div');
            badge.setAttribute('role', 'status');
            badge.textContent = 'Spindle is reading this page · Press Esc to stop';
            const pointer = document.createElement('span');
            const first = document.querySelector('main h1, [role=main] h1, h1, main, article');
            if (first) {{
                const bounds = first.getBoundingClientRect();
                const x = Math.max(0, Math.min(innerWidth - 35, bounds.left));
                const y = Math.max(0, Math.min(innerHeight - 44, bounds.top));
                pointer.style.transform = 'translate(' + x + 'px,' + y + 'px)';
            }}
            shadow.append(style, badge, pointer);
            document.documentElement.append(host);
            const stop = event => {{
                if (event.isTrusted && event.key === 'Escape') {{
                    document.documentElement.dataset.spindleReadStopped = '1';
                    host.remove();
                }}
            }};
            addEventListener('keydown',stop,true);
            addEventListener('spindle-read-finished', () => {{host.remove();removeEventListener('keydown',stop,true);}}, {{once:true}});
    }}"#)
}

#[cfg(windows)]
pub(crate) const VISIBLE_PAGE_SCRIPT: &str = r#"(() => {
    if (document.readyState !== 'complete') return null;
    window.dispatchEvent(new Event('spindle-read-finished'));
    if (document.documentElement.dataset.spindleReadStopped === '1') return {stopped:true};
    const page = document.querySelector('main, [role=main], article') || document.body;
    const links = [];
    for (const link of page?.querySelectorAll('a[href]') || []) {
        const label = (link.innerText || link.getAttribute('aria-label') || '').trim().slice(0,180);
        if (!label || !link.getClientRects().length || !/^https?:/.test(link.href)) continue;
        links.push([label, link.href.slice(0,2048)]);
        if (links.length === 100) break;
    }
    return {address: location.href, status: performance.getEntriesByType('navigation')[0]?.responseStatus ?? 0,
        title: document.title.slice(0,300), text: (page?.innerText || '').slice(0,18000), links};
})()"#;

#[cfg(windows)]
#[derive(serde::Deserialize)]
pub(crate) struct VisiblePage {
    address: String,
    status: u16,
    title: String,
    text: String,
    links: Vec<(String, String)>,
}

#[cfg(windows)]
impl VisiblePage {
    pub fn read(self, expected: &str) -> Result<String> {
        let mut address = public_page(&self.address)?;
        let mut expected = public_page(expected)?;
        address.set_fragment(None);
        expected.set_fragment(None);
        ensure!(address == expected, "The browser left the approved page. Nothing was shared.");
        if self.status >= 400 { return Err(PageAccess::Http(self.status).into()); }
        if verification_page(&self.title, &self.text) { return Err(PageAccess::Challenge.into()); }
        ensure!(!self.text.trim().is_empty(), "The page has no readable text");
        let body: String = self.text.chars().take(14000).collect();
        let mut result = format!("Public page: {address}\n{}\n\n{body}\n\nVisible links:\n", self.title.chars().take(300).collect::<String>());
        for (label, link) in self.links.into_iter().take(100) {
            if let Ok(link) = public_page(&link) {
                let label: String = label.chars().take(180).collect();
                result.push_str(&format!("- {label} · {link}\n"));
            }
        }
        Ok(result)
    }
}

pub(crate) fn verification_page(title: &str, body: &str) -> bool {
    let title = title.trim().to_ascii_lowercase();
    let body: String = body.chars().take(1800).collect::<String>().to_ascii_lowercase();
    ["just a moment", "verify you are human", "attention required", "access denied",
        "robot check", "security check"]
        .iter().any(|phrase| title.starts_with(phrase))
        || (body.lines().count() < 25
            && ["verify that you are human", "complete the security check", "confirm you're not a robot",
                "unfortunately, bots use duckduckgo too", "our systems have detected unusual traffic"]
                .iter().any(|phrase| body.contains(phrase)))
}

/// The saved executable overrides the environment and system discovery.
pub fn resolve(configured: &str) -> Result<PathBuf> {
    ensure!(SUPPORTED, "The model browser is available on Windows and Linux");
    if !configured.trim().is_empty() {
        return executable(Path::new(configured.trim())).with_context(|| {
            format!("The configured browser is unavailable. Choose an executable in {SETTINGS}")
        });
    }
    if let Some(path) = std::env::var_os("SPINDLE_BROWSER").filter(|p| !p.is_empty()) {
        return executable(Path::new(&path)).with_context(|| {
            format!("SPINDLE_BROWSER is unavailable. Change it or choose an executable in {SETTINGS}")
        });
    }
    discover().with_context(|| {
        format!("No Chromium browser found. Install Edge, Chrome or Chromium, or choose an executable in {SETTINGS}")
    })
}

fn executable(path: &Path) -> Result<PathBuf> {
    ensure!(path.is_absolute(), "Enter an absolute executable path");
    let file = path.metadata().context("The executable cannot be opened")?;
    ensure!(file.is_file(), "Select a browser executable file");
    #[cfg(windows)]
    ensure!(
        path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("exe")),
        "Select a browser .exe file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(file.permissions().mode() & 0o111 != 0, "The file is not executable");
    }
    Ok(path.to_owned())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkBrowser {
    #[default]
    Edge,
    Chrome,
}

impl LinkBrowser {
    pub const ALL: [Self; 2] = [Self::Edge, Self::Chrome];

    pub fn available() -> impl Iterator<Item = Self> {
        Self::ALL.into_iter().filter(|browser| browser.installed())
    }
    pub fn installed(self) -> bool { self.resolve().is_ok() }

    pub fn key(self) -> &'static str {
        match self { Self::Edge => "edge", Self::Chrome => "chrome" }
    }

    pub fn label(self) -> &'static str {
        match self { Self::Edge => "Microsoft Edge", Self::Chrome => "Google Chrome" }
    }

    fn resolve(self) -> Result<PathBuf> {
        ensure!(SUPPORTED, "Browser Link is available on desktop");
        #[cfg(windows)]
        let candidate = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"].into_iter()
            .filter_map(std::env::var_os)
            .map(|root| PathBuf::from(root).join(match self {
                Self::Edge => r"Microsoft\Edge\Application\msedge.exe",
                Self::Chrome => r"Google\Chrome\Application\chrome.exe",
            }))
            .find_map(|path| executable(&path).ok());
        #[cfg(not(windows))]
        let candidate = std::env::var_os("PATH").into_iter().flat_map(|paths| {
            std::env::split_paths(&paths).collect::<Vec<_>>()
        }).find_map(|dir| match self {
            Self::Edge => ["microsoft-edge", "microsoft-edge-stable"],
            Self::Chrome => ["google-chrome", "google-chrome-stable"],
        }.into_iter().find_map(|name| executable(&dir.join(name)).ok()));
        candidate.with_context(|| format!("Install {} first", self.label()))
    }

    pub fn prepare_existing_profile(self) -> Result<PathBuf> {
        self.resolve()?;
        crate::browser_native::install(self)?;
        crate::browser_link::extension_folder()
    }

    fn extensions_address(self) -> &'static str {
        match self {
            Self::Edge => "edge://extensions/",
            Self::Chrome => "chrome://extensions/",
        }
    }

    fn write_install_guide(self, folder: &Path, staging: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(staging)?;
        let guide = staging.join(match self {
            Self::Edge => "setup-browser-link-edge.html",
            Self::Chrome => "setup-browser-link-chrome.html",
        });
        let folder_text = folder.to_str().context("The extension path must be valid Unicode")?
            .replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let download_url = crate::browser_link::extension_download_url();
        let html = include_str!("../assets/browser-link-setup.html")
            .replace("{{browser}}", self.label())
            .replace("{{address}}", self.extensions_address())
            .replace("{{download}}", download_url.as_deref().unwrap_or(""))
            .replace("{{download_visibility}}", if download_url.is_some() { "" } else { "hidden" })
            .replace("{{staging}}", if cfg!(windows) {
                "Spindle unpacked Browser Link in <strong>Downloads</strong>."
            } else { "Spindle unpacked Browser Link in the folder shown below." })
            .replace("{{folder}}", &folder_text);
        let mut temp = tempfile::Builder::new().prefix(".browser-link-guide-").tempfile_in(staging)?;
        temp.write_all(html.as_bytes())?;
        temp.persist(&guide)?;
        Ok(guide)
    }

    pub fn open_install_guide(self, folder: &Path) -> Result<()> {
        // Retail Chrome and Edge discard internal extensions URLs supplied at startup.
        // Open an ordinary local page in the user's profile. No debugging flags required.
        let guide = self.write_install_guide(folder, &crate::runtime::data_dir().join("plugins"))?;
        let url = file_url(&guide).context("Cannot open the Browser Link install guide")?;
        Command::new(self.resolve()?).arg(url).stdin(Stdio::null())
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
        Ok(())
    }
}

fn discover() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let mut roots = vec![];
        for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Some(v) = std::env::var_os(var) {
                roots.push(PathBuf::from(v));
            }
        }
        for root in &roots {
            for rel in [
                r"Microsoft\Edge\Application\msedge.exe",
                r"Google\Chrome\Application\chrome.exe",
                r"Chromium\Application\chrome.exe",
            ] {
                let p = root.join(rel);
                if let Ok(p) = executable(&p) {
                    return Some(p);
                }
            }
        }
        None
    }
    #[cfg(not(windows))]
    {
        let path = std::env::var_os("PATH")?;
        for name in [
            "microsoft-edge",
            "microsoft-edge-stable",
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
        ] {
            for dir in std::env::split_paths(&path) {
                let p = dir.join(name);
                if let Ok(p) = executable(&p) {
                    return Some(p);
                }
            }
        }
        None
    }
}

/// What the page did.
#[derive(Debug, Default)]
pub struct Report {
    pub title: String,
    pub console: Vec<String>,
    pub errors: Vec<String>,
    pub requests: Vec<String>,
    pub failed: Vec<String>,
    pub result: Option<String>,
    /// Internet requests were blocked for this check.
    pub blocked_web: bool,
    /// PNG bytes of the page after the checks.
    pub screenshot: Option<Vec<u8>>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.errors.is_empty() && self.failed.is_empty()
    }

    /// The report for the model.
    pub fn text(&self, root: &Path) -> String {
        let clean = |s: &str| {
            let mut s = s.replace(&file_url(root).unwrap_or_default(), "");
            s = s.replace(&root.display().to_string(), "");
            s
        };
        let mut out = format!(
            "{} · title: {}\n",
            if self.ok() {
                "No errors"
            } else {
                "Problems found"
            },
            if self.title.is_empty() {
                "(none)"
            } else {
                &self.title
            }
        );
        let section = |out: &mut String, head: &str, items: &[String], limit: usize| {
            if items.is_empty() {
                out.push_str(&format!("{head}: none\n"));
                return;
            }
            out.push_str(&format!("{head} ({}):\n", items.len()));
            for i in items.iter().take(limit) {
                out.push_str(&format!("  {}\n", clean(i)));
            }
            if items.len() > limit {
                out.push_str(&format!("  … {} more\n", items.len() - limit));
            }
        };
        section(&mut out, "Errors", &self.errors, 30);
        section(&mut out, "Console", &self.console, 40);
        if self.blocked_web {
            out.push_str("Web traffic is blocked for page checks (a setting), so internet requests fail with ERR_BLOCKED_BY_CLIENT; local files load normally.\n");
        }
        section(&mut out, "Failed requests", &self.failed, 20);
        section(&mut out, "Requests", &self.requests, 30);
        if let Some(r) = &self.result {
            out.push_str(&format!("Script result: {}\n", clean(r)));
        }
        if self.screenshot.is_some() {
            out.push_str("A screenshot was taken.\n");
        }
        out
    }
}

fn file_url(path: &Path) -> Option<String> {
    reqwest::Url::from_file_path(path)
        .ok()
        .map(|u| u.to_string())
}

struct Browser {
    child: Child,
    _profile: tempfile::TempDir,
    _blocked_proxy: Option<std::net::TcpListener>,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn launch(exe: &Path, public_web: bool) -> Result<(Browser, u16)> {
    launch_configured(exe, public_web, |_| {})
}

fn launch_configured(
    exe: &Path,
    public_web: bool,
    configure: impl FnOnce(&mut Command),
) -> Result<(Browser, u16)> {
    let profile = tempfile::tempdir()?;
    let mut cmd = Command::new(exe);
    cmd.args([
        "--headless=new",
        "--remote-debugging-port=0",
        "--remote-allow-origins=*",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--disable-sync",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-default-apps",
        "--mute-audio",
        "--window-size=1280,800",
    ])
    .arg(format!("--user-data-dir={}", profile.path().display()))
    .arg("about:blank")
    .stdin(Stdio::null())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    let blocked_proxy = if public_web {
        // Browser traffic fails closed. Only intercepted reads fulfilled by Spindle
        // can reach the network, including when a page opens other targets.
        let proxy = std::net::TcpListener::bind("127.0.0.1:0")?;
        cmd.arg(format!("--proxy-server=http://{}", proxy.local_addr()?));
        cmd.args([
            "--proxy-bypass-list=<-loopback>",
            "--host-resolver-rules=MAP * ~NOTFOUND",
            "--disable-quic",
            "--disable-features=MediaRouter,OptimizationHints,SpeculationRulesPrefetchProxy",
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
        ]);
        Some(proxy)
    } else {
        None
    };
    // Chrome refuses to start as root on Linux without this.
    #[cfg(unix)]
    if unsafe { libc::geteuid() } == 0 {
        ensure!(
            !public_web,
            "Run Spindle as a regular user to open public pages in Chromium"
        );
        cmd.arg("--no-sandbox");
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    configure(&mut cmd);
    let child = cmd
        .spawn()
        .with_context(|| format!("Couldn't start {}", exe.display()))?;
    let browser = Browser {
        child,
        _profile: profile,
        _blocked_proxy: blocked_proxy,
    };
    let marker = browser._profile.path().join("DevToolsActivePort");
    let start = Instant::now();
    loop {
        if let Ok(text) = std::fs::read_to_string(&marker)
            && let Some(port) = text.lines().next().and_then(|l| l.trim().parse().ok())
        {
            return Ok((browser, port));
        }
        ensure!(
            start.elapsed() < Duration::from_secs(20),
            "The browser didn't start within 20 seconds"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn http_get(port: u16, path: &str) -> Result<String> {
    let mut s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )?;
    // The DevTools server may keep the connection open, so read by Content-Length.
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&data);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("content-length")
                    .then(|| v.trim().parse::<usize>().ok())?
            });
            if length.is_some_and(|n| body.len() >= n) {
                return Ok(body.to_owned());
            }
        }
    }
    let text = String::from_utf8_lossy(&data);
    Ok(text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default())
}

/// A minimal WebSocket client for the DevTools connection (local, text frames).
struct Socket {
    stream: TcpStream,
    next: u64,
}

impl Socket {
    fn connect(port: u16, path: &str) -> Result<Self> {
        let mut stream = TcpStream::connect(("127.0.0.1", port))?;
        let key = base64::engine::general_purpose::STANDARD.encode(rand_bytes());
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )?;
        // Read the handshake response up to the blank line.
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte)?;
            head.push(byte[0]);
            ensure!(head.len() < 8192, "Bad handshake from the browser");
        }
        let head = String::from_utf8_lossy(&head);
        ensure!(
            head.starts_with("HTTP/1.1 101"),
            "The browser refused the connection: {}",
            head.lines().next().unwrap_or_default()
        );
        Ok(Self { stream, next: 0 })
    }

    fn send(&mut self, method: &str, params: Value) -> Result<u64> {
        self.next += 1;
        let text = json!({"id": self.next, "method": method, "params": params}).to_string();
        let data = text.as_bytes();
        let mut frame = vec![0x81u8];
        match data.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n if n < 65536 => {
                frame.push(0x80 | 126);
                frame.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                frame.push(0x80 | 127);
                frame.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        let mask = rand_bytes();
        let mask = [mask[0], mask[1], mask[2], mask[3]];
        frame.extend_from_slice(&mask);
        frame.extend(data.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        self.stream.write_all(&frame)?;
        Ok(self.next)
    }

    /// The next message, or None when nothing arrives before `until`.
    fn receive(&mut self, until: Instant) -> Result<Option<Value>> {
        let mut message = Vec::new();
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            self.stream.set_read_timeout(Some(left))?;
            let mut head = [0u8; 2];
            match self.stream.read_exact(&mut head) {
                Ok(()) => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            }
            // Once a frame has started, read the rest of it whatever the deadline.
            self.stream
                .set_read_timeout(Some(Duration::from_secs(10)))?;
            let fin = head[0] & 0x80 != 0;
            let op = head[0] & 0x0f;
            let mut len = u64::from(head[1] & 0x7f);
            if len == 126 {
                let mut b = [0u8; 2];
                self.stream.read_exact(&mut b)?;
                len = u64::from(u16::from_be_bytes(b));
            } else if len == 127 {
                let mut b = [0u8; 8];
                self.stream.read_exact(&mut b)?;
                len = u64::from_be_bytes(b);
            }
            ensure!(
                len <= 64 * 1024 * 1024,
                "Message from the browser too large"
            );
            let mut payload = vec![0u8; len as usize];
            self.stream.read_exact(&mut payload)?;
            match op {
                0x8 => bail!("The browser closed the connection"),
                0x9 => {
                    // Ping: answer with a masked pong.
                    let mut frame = vec![0x8a, 0x80 | payload.len().min(125) as u8, 0, 0, 0, 0];
                    frame.extend_from_slice(&payload[..payload.len().min(125)]);
                    self.stream.write_all(&frame)?;
                }
                0xa => {}
                _ => {
                    message.extend_from_slice(&payload);
                    if fin {
                        return Ok(Some(serde_json::from_slice(&message)?));
                    }
                }
            }
        }
    }

    /// Send a command and wait for its reply, handing events that arrive meanwhile to `event`.
    fn call(
        &mut self,
        method: &str,
        params: Value,
        event: &mut dyn FnMut(&Value),
    ) -> Result<Value> {
        let id = self.send(method, params)?;
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let Some(msg) = self.receive(until)? else {
                bail!("The browser didn't answer {method}");
            };
            if msg.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(e) = msg.get("error") {
                    bail!("{method}: {}", e["message"].as_str().unwrap_or("failed"));
                }
                return Ok(msg["result"].clone());
            }
            event(&msg);
        }
    }
}

fn rand_bytes() -> [u8; 16] {
    let mut b = [0u8; 16];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).ok();
    b
}

/// Collects what the page reports.
#[derive(Default)]
struct Collector {
    report: Report,
    requests: BTreeMap<String, (String, String)>,
    loaded: bool,
}

impl Collector {
    fn event(&mut self, msg: &Value) {
        let p = &msg["params"];
        let at = |url: &Value, line: &Value| {
            let url = url.as_str().unwrap_or_default();
            let short = url.rsplit('/').next().unwrap_or(url);
            match line.as_i64() {
                Some(l) if !short.is_empty() => format!(" ({short}:{})", l + 1),
                _ => String::new(),
            }
        };
        match msg["method"].as_str().unwrap_or_default() {
            "Page.loadEventFired" => self.loaded = true,
            "Runtime.consoleAPICalled" => {
                let text: Vec<String> = p["args"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|v| match &v["value"] {
                                Value::String(s) => s.clone(),
                                Value::Null => v["description"]
                                    .as_str()
                                    .unwrap_or(v["type"].as_str().unwrap_or(""))
                                    .to_owned(),
                                other => other.to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let frame = &p["stackTrace"]["callFrames"][0];
                let kind = p["type"].as_str().unwrap_or("log");
                let line = format!(
                    "[{kind}] {}{}",
                    text.join(" "),
                    at(&frame["url"], &frame["lineNumber"])
                );
                if kind == "error" || kind == "assert" {
                    self.report.errors.push(line);
                } else if self.report.console.len() < 200 {
                    self.report.console.push(line);
                }
            }
            "Runtime.exceptionThrown" => {
                let d = &p["exceptionDetails"];
                let text = d["exception"]["description"]
                    .as_str()
                    .or(d["text"].as_str())
                    .unwrap_or("Uncaught exception");
                let first = text.lines().next().unwrap_or(text);
                self.report.errors.push(format!(
                    "[exception] {first}{}",
                    at(&d["url"], &d["lineNumber"])
                ));
            }
            "Log.entryAdded" => {
                let e = &p["entry"];
                let level = e["level"].as_str().unwrap_or("info");
                let line = format!(
                    "[{level}] {}{}",
                    e["text"].as_str().unwrap_or_default(),
                    at(&e["url"], &e["lineNumber"])
                );
                if level == "error" {
                    self.report.errors.push(line);
                } else if self.report.console.len() < 200 {
                    self.report.console.push(line);
                }
            }
            "Network.requestWillBeSent" => {
                let id = p["requestId"].as_str().unwrap_or_default().to_owned();
                let method = p["request"]["method"].as_str().unwrap_or("GET").to_owned();
                let url = p["request"]["url"].as_str().unwrap_or_default();
                let url: String = url.chars().take(200).collect();
                self.requests.insert(id, (method, url));
            }
            "Network.responseReceived" => {
                let id = p["requestId"].as_str().unwrap_or_default();
                if let Some((method, url)) = self.requests.get(id) {
                    let status = p["response"]["status"].as_i64().unwrap_or(0);
                    let line = format!("{method} {url} → {status}");
                    if status >= 400 {
                        self.report.failed.push(line);
                    } else if self.report.requests.len() < 200 && !url.starts_with("data:") {
                        self.report.requests.push(line);
                    }
                }
            }
            "Network.loadingFailed" => {
                let id = p["requestId"].as_str().unwrap_or_default();
                if let Some((method, url)) = self.requests.get(id) {
                    self.report.failed.push(format!(
                        "{method} {url} → {}",
                        p["errorText"].as_str().unwrap_or("failed")
                    ));
                }
            }
            _ => {}
        }
    }
}

fn connect_tab(port: u16) -> Result<Socket> {
    let list: Value = serde_json::from_str(&http_get(port, "/json/list")?)
        .context("The browser's page list wasn't readable")?;
    let target = list
        .as_array()
        .and_then(|a| a.iter().find(|t| t["type"] == "page"))
        .and_then(|t| t["webSocketDebuggerUrl"].as_str())
        .context("The browser has no page to use")?;
    let path = target
        .split_once(&format!(":{port}"))
        .map(|(_, p)| p)
        .context("Unexpected DevTools address")?;
    Socket::connect(port, path)
}

/// Verify that the configured Chromium can start and answer DevTools commands.
pub fn check_installation(exe: &Path) -> Result<String> {
    let (_browser, port) = launch(exe, true)?;
    let mut ws = connect_tab(port)?;
    let version = ws.call("Browser.getVersion", json!({}), &mut |_| {})?;
    version["product"]
        .as_str()
        .map(str::to_owned)
        .context("The browser did not report its version through DevTools")
}

/// Open `page` headless, run `script` if given, then take a screenshot.
pub fn check(
    exe: &Path,
    page: &Path,
    script: Option<&str>,
    wait: Duration,
    block_web: bool,
) -> Result<Report> {
    let url = file_url(page).context("Couldn't make a file URL for the page")?;
    let (_browser, port) = launch(exe, false)?;
    let mut ws = connect_tab(port)?;
    let mut c = Collector::default();
    for m in [
        "Runtime.enable",
        "Log.enable",
        "Network.enable",
        "Page.enable",
    ] {
        ws.call(m, json!({}), &mut |e| c.event(e))?;
    }
    if block_web {
        ws.call(
            "Network.setBlockedURLs",
            json!({"urls": ["http://*", "https://*", "ws://*", "wss://*"]}),
            &mut |e| c.event(e),
        )?;
        c.report.blocked_web = true;
    }
    ws.call("Page.navigate", json!({"url": url}), &mut |e| c.event(e))?;
    // Until the load event (at most 15 s), then the settle time.
    let load_by = Instant::now() + Duration::from_secs(15);
    while !c.loaded {
        match ws.receive(load_by)? {
            Some(msg) => c.event(&msg),
            None => {
                c.report
                    .errors
                    .push("[timeout] The page didn't finish loading within 15 seconds".into());
                break;
            }
        }
    }
    let settle = |ws: &mut Socket, c: &mut Collector, d: Duration| -> Result<()> {
        let until = Instant::now() + d;
        while let Some(msg) = ws.receive(until)? {
            c.event(&msg);
        }
        Ok(())
    };
    settle(&mut ws, &mut c, wait)?;
    if let Some(script) = script.filter(|s| !s.trim().is_empty()) {
        let r = ws.call(
            "Runtime.evaluate",
            json!({"expression": script, "awaitPromise": true, "returnByValue": true, "userGesture": true}),
            &mut |e| c.event(e),
        )?;
        c.report.result = Some(if let Some(d) = r.get("exceptionDetails") {
            let text = d["exception"]["description"]
                .as_str()
                .or(d["text"].as_str())
                .unwrap_or("threw an exception");
            let error = text.lines().next().unwrap_or(text);
            c.report.errors.push(format!("Test script: {error}"));
            format!("threw: {error}")
        } else {
            let v = &r["result"]["value"];
            let s = match v {
                Value::Null => r["result"]["description"]
                    .as_str()
                    .unwrap_or("undefined")
                    .to_owned(),
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            s.chars().take(2000).collect()
        });
        settle(&mut ws, &mut c, wait.min(Duration::from_secs(5)))?;
    }
    let title = ws.call(
        "Runtime.evaluate",
        json!({"expression": "document.title", "returnByValue": true}),
        &mut |e| c.event(e),
    )?;
    c.report.title = title["result"]["value"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if let Ok(shot) = ws.call(
        "Page.captureScreenshot",
        json!({"format": "png"}),
        &mut |e| c.event(e),
    ) && let Some(data) = shot["data"].as_str()
    {
        c.report.screenshot = base64::engine::general_purpose::STANDARD.decode(data).ok();
    }
    Ok(c.report)
}

/// Open one public page in an isolated Chromium profile. Only Spindle issues network
/// requests. The model supplies a URL and a read offset, and gets text and link URLs.
#[cfg(any(windows, target_os = "linux"))]
pub(crate) fn open_public(
    exe: &Path,
    address: &str,
    start: usize,
    diagnostics: bool,
    runtime: &tokio::runtime::Handle,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<String> {
    let mut network = crate::public_web::Network::default();
    open_public_tab(
        exe,
        address,
        start,
        diagnostics,
        cancelled,
        |url, method, accept, max, budget| {
            runtime.block_on(async {
                tokio::time::timeout(
                    budget.min(Duration::from_secs(4)),
                    network.read(url, method, accept, max),
                )
                .await
                .context("The public site did not respond in time")?
            })
        },
    )
}

#[cfg(any(windows, target_os = "linux"))]
fn open_public_tab(
    exe: &Path,
    address: &str,
    start: usize,
    diagnostics: bool,
    cancelled: &std::sync::atomic::AtomicBool,
    mut read: impl FnMut(
        &str,
        &str,
        Option<&str>,
        usize,
        Duration,
    ) -> Result<crate::public_web::Response>,
) -> Result<String> {
    use crate::public_web;
    use std::sync::atomic::Ordering;
    let address = public_web::url(address)?;
    let (_browser, port) = launch(exe, true)?;
    let mut ws = connect_tab(port)?;
    for method in ["Page.enable", "Network.enable"] {
        ws.call(method, json!({}), &mut |_| {})?;
    }
    if diagnostics {
        for method in ["Runtime.enable", "Log.enable"] {
            ws.call(method, json!({}), &mut |_| {})?;
        }
    }
    let tree = ws.call("Page.getFrameTree", json!({}), &mut |_| {})?;
    let root = tree["frameTree"]["frame"]["id"]
        .as_str()
        .context("Chromium has no main frame")?;
    ws.call(
        "Network.setBypassServiceWorker",
        json!({"bypass": true}),
        &mut |_| {},
    )?;
    ws.call(
        "Network.setBlockedURLs",
        json!({"urls": ["file://*", "ftp://*", "ws://*", "wss://*"]}),
        &mut |_| {},
    )?;
    ws.call(
        "Fetch.enable",
        json!({"patterns": [{"urlPattern":"*", "requestStage":"Request"}]}),
        &mut |_| {},
    )?;
    let navigation = ws.send("Page.navigate", json!({"url": address.as_str()}))?;
    let deadline = Instant::now() + Duration::from_secs(22);
    let mut settle_by = None;
    let mut document = None;
    let mut status = None;
    let mut count = 0;
    let mut bytes_left = 16 * 1024 * 1024;
    let mut traffic = Vec::new();
    let mut console = Collector::default();

    while Instant::now() < settle_by.unwrap_or(deadline) {
        ensure!(!cancelled.load(Ordering::Relaxed), "Browsing stopped");
        let until =
            (Instant::now() + Duration::from_millis(150)).min(settle_by.unwrap_or(deadline));
        let Some(message) = ws.receive(until)? else {
            continue;
        };
        if message["id"].as_u64() == Some(navigation) {
            if let Some(error) = message["error"]["message"]
                .as_str()
                .or(message["result"]["errorText"].as_str())
            {
                bail!("Chromium could not open the page: {error}");
            }
        }
        let p = &message["params"];
        if diagnostics
            && console.report.errors.len() + console.report.console.len() < 100
            && matches!(
                message["method"].as_str(),
                Some("Runtime.consoleAPICalled" | "Runtime.exceptionThrown" | "Log.entryAdded")
            )
        {
            console.event(&message);
        }
        match message["method"].as_str() {
            Some("Page.frameNavigated") if p["frame"]["id"].as_str() == Some(root) => {
                document = p["frame"]["url"].as_str().map(str::to_owned);
                settle_by = None;
            }
            Some("Page.loadEventFired") if document.is_some() => {
                let wait = if diagnostics { 1900 } else { 1100 };
                settle_by = Some((Instant::now() + Duration::from_millis(wait)).min(deadline));
            }
            Some("Fetch.requestPaused") => {
                let id = p["requestId"]
                    .as_str()
                    .context("Could not identify the Chromium request")?;
                let address = p["request"]["url"].as_str().unwrap_or_default();
                let method = p["request"]["method"].as_str().unwrap_or_default();
                let accept = p["request"]["headers"].as_object().and_then(|headers| {
                    headers
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case("accept"))
                        .and_then(|(_, value)| value.as_str())
                });
                let main = p["resourceType"] == "Document" && p["frameId"].as_str() == Some(root);
                let render_budget = settle_by
                    .unwrap_or(deadline)
                    .saturating_duration_since(Instant::now());
                let result = if !matches!(method, "GET" | "HEAD") {
                    Err(anyhow::anyhow!("Public browsing uses GET and HEAD"))
                } else if count < 80 && bytes_left > 0 && !render_budget.is_zero() {
                    count += 1;
                    read(
                        address,
                        method,
                        accept,
                        bytes_left.min(if main { 4 << 20 } else { 2 << 20 }),
                        render_budget,
                    )
                } else {
                    Err(anyhow::anyhow!("Page exceeded its browsing budget"))
                };
                if diagnostics && traffic.len() < 80 {
                    let route: String = address.chars().take(250).collect();
                    let resource_type = p["resourceType"].as_str().unwrap_or("Other");
                    let outcome = match &result {
                        Ok(response) => {
                            format!("HTTP {} · {} bytes", response.status, response.bytes.len())
                        }
                        Err(error) => format!(
                            "blocked or failed · {}",
                            error.to_string().chars().take(140).collect::<String>()
                        ),
                    };
                    traffic.push(format!("{method} {route} [{resource_type}] → {outcome}"));
                }
                match result {
                    Ok(response) => {
                        if main {
                            status = Some(response.status);
                            if !diagnostics && response.status >= 400 {
                                return Err(PageAccess::Http(response.status).into());
                            }
                        }
                        bytes_left -= response.bytes.len();
                        let headers: Vec<_> = response
                            .headers
                            .into_iter()
                            .map(|(name, value)| json!({"name":name,"value":value}))
                            .collect();
                        ws.send("Fetch.fulfillRequest", json!({
                            "requestId": id, "responseCode": response.status, "responseHeaders": headers,
                            "body": base64::engine::general_purpose::STANDARD.encode(response.bytes),
                        }))?;
                    }
                    Err(error) if main => return Err(error.context("The built-in browser could not load the main document")),
                    Err(_) => {
                        ws.send(
                            "Fetch.failRequest",
                            json!({"requestId": id, "errorReason":"BlockedByClient"}),
                        )?;
                    }
                }
            }
            _ => {}
        }
    }
    let final_url = public_web::url(
        document
            .as_deref()
            .context("The page did not finish opening")?,
    )?;
    ensure!(!cancelled.load(Ordering::Relaxed), "Browsing stopped");
    // A fixed expression reads the content. The page cannot supply or trigger a
    // model-authored script, form submission, or authenticated browser session.
    let expression = r#"(() => {
        const content = document.querySelector('main, [role="main"], article') || document.body;
        return {
            title: document.title.slice(0, 300),
            text: (content?.innerText || document.body?.innerText || '').slice(0, 300000),
            links: Array.from(document.querySelectorAll('a[href]')).slice(0, 1200)
                .filter(a => a.getClientRects().length > 0)
                .slice(0, 220)
                .map(a => [a.href.slice(0,4096), (a.innerText || a.getAttribute('aria-label') || '').trim().slice(0,140)])
        };
    })()"#;
    let result = ws.call(
        "Runtime.evaluate",
        json!({"expression": expression, "returnByValue": true}),
        &mut |_| {},
    )?;
    let page = &result["result"]["value"];
    let body = page["text"]
        .as_str()
        .context("Chromium could not read the page")?;
    let body = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let challenge = verification_page(page["title"].as_str().unwrap_or_default(), &body);
    let (visible, more) = {
        let mut chars = body.chars().skip(start);
        let content: String = chars.by_ref().take(6000).collect();
        (content, chars.next().is_some())
    };
    let mut output = format!(
        "Public page: {final_url}\nHTTP {} · {}\n\n",
        status.unwrap_or(200),
        page["title"].as_str().unwrap_or_default()
    );
    if visible.is_empty() {
        output.push_str(if start == 0 {
            "The page exposed no readable text."
        } else {
            "No text at this offset."
        });
    } else {
        output.push_str(&visible);
    }
    if more {
        output.push_str(&format!(
            "\n\nContinue with web_open and start: {}.",
            start + 6000
        ));
    }
    if diagnostics {
        add_diagnostics(&mut output, "Console errors", &console.report.errors, 8);
        add_diagnostics(
            &mut output,
            "Other console messages",
            &console.report.console,
            6,
        );
        add_diagnostics(&mut output, "Network requests", &traffic, 16);
    }
    let problem = status.filter(|status| *status >= 400).map(PageAccess::Http)
        .or_else(|| challenge.then_some(PageAccess::Challenge));
    if let Some(problem) = problem {
        let error = anyhow::Error::new(problem);
        return Err(if diagnostics { error.context(output) } else { error });
    }
    if let Some(links) = page["links"].as_array() {
        let mut seen = std::collections::HashSet::new();
        output.push_str("\n\nPublic links (use web_open with the URL):\n");
        for link in links
            .iter()
            .filter_map(|link| {
                let url = public_web::url(link[0].as_str()?).ok()?;
                let label = link[1].as_str()?.trim();
                (!label.is_empty() && seen.insert(url.clone())).then_some((label, url))
            })
            .take(24)
        {
            if output.len() > 14_000 {
                break;
            }
            output.push_str(&format!(
                "- {} · {}\n",
                link.0.split_whitespace().collect::<Vec<_>>().join(" "),
                link.1
            ));
        }
    }
    Ok(output)
}

#[cfg(any(windows, target_os = "linux"))]
fn add_diagnostics(output: &mut String, heading: &str, items: &[String], limit: usize) {
    output.push_str(&format!("\n\n{heading} ({}):\n", items.len()));
    for line in items.iter().take(limit) {
        output.push_str(&line.chars().take(260).collect::<String>());
        output.push('\n');
    }
    if items.len() > limit {
        output.push_str("… more entries omitted\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_verification_pages() {
        assert!(verification_page("Just a moment…", "Hold on"));
        assert!(verification_page("Search", "Unfortunately, bots use DuckDuckGo too."));
        assert!(!verification_page("Radeon RX 6800 XT search results", "Used graphics cards with shipping."));
    }

    #[test]
    fn install_guide_is_local_browser_specific_and_outside_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("example & tools").join("spindle-browser-link");
        std::fs::create_dir_all(&folder).unwrap();
        for browser in [LinkBrowser::Edge, LinkBrowser::Chrome] {
            let guide = browser.write_install_guide(&folder, dir.path()).unwrap();
            assert_eq!(guide.parent(), Some(dir.path()));
            let html = std::fs::read_to_string(&guide).unwrap();
            assert!(html.contains(browser.extensions_address()));
            assert!(html.contains(browser.label()));
            if let Some(url) = crate::browser_link::extension_download_url() {
                assert!(html.contains(&url));
            }
            assert!(html.contains("example &amp; tools"));
            assert!(!html.contains("{{"));
            assert!(file_url(&guide).unwrap().starts_with("file:"));
        }
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    #[ignore = "launches the installed Chromium and plays a local HTML preview"]
    fn local_html_preview_executes_relative_scripts_and_follows_links() {
        let exe = resolve("").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snake.html");
        let source = r#"<!doctype html><link rel="stylesheet" href="theme.css"><button id="play">Start</button><a id="next" href="next.html">Next</a><script src="game.js"></script>"#;
        std::fs::write(dir.path().join("theme.css"), "#play { color: rgb(0, 128, 0); }").unwrap();
        std::fs::write(dir.path().join("game.js"), "document.querySelector('#play').onclick=()=>document.title='Playing';").unwrap();
        std::fs::write(dir.path().join("next.html"), "<!doctype html><title>Next level</title>").unwrap();
        let server = crate::html_preview::Server::start("snake.html", source.into(), Some(&path)).unwrap();
        #[cfg(target_os = "linux")]
        let ((_browser, port), url) = (
            launch_configured(&exe, false, |cmd| crate::html_preview::configure_proxy(cmd, &server)).unwrap(),
            &server.app_url,
        );
        #[cfg(windows)]
        let ((_browser, port), url) = (launch(&exe, false).unwrap(), &server.url);
        let mut page = connect_tab(port).unwrap();
        page.call("Page.navigate", json!({"url": url}), &mut |_| {}).unwrap();
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            let ready = page.call("Runtime.evaluate", json!({"expression":"Boolean(document.querySelector('#play')?.onclick)","returnByValue":true}), &mut |_| {}).unwrap();
            if ready["result"]["value"] == true { break; }
            std::thread::sleep(Duration::from_millis(100));
        }
        let played = page.call("Runtime.evaluate", json!({"expression":"document.querySelector('#play').click(); [document.title, getComputedStyle(document.querySelector('#play')).color]", "returnByValue": true}), &mut |_| {}).unwrap();
        assert_eq!(played["result"]["value"], json!(["Playing", "rgb(0, 128, 0)"]));
        let _ = page.call("Runtime.evaluate", json!({"expression":"document.querySelector('#next').click()"}), &mut |_| {});
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(next) = page.call("Runtime.evaluate", json!({"expression":"document.title", "returnByValue": true}), &mut |_| {})
                && next["result"]["value"] == "Next level" { break; }
            assert!(Instant::now() < until, "Relative page link did not navigate");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[cfg(any(windows, target_os = "linux"))]
    #[test]
    fn public_browser_follows_redirects_and_reads_rendered_content() {
        let Ok(exe) = resolve("") else { return };
        #[cfg(unix)]
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let mut requests = Vec::new();
        let result = open_public_tab(
            &exe,
            "https://www.example.com/start",
            0,
            true,
            &Default::default(),
            |url, method, _, _, _| {
                let parsed = crate::public_web::url(url)?;
                requests.push((url.to_owned(), method.to_owned()));
                ensure!(parsed.host_str() == Some("www.example.com") && method == "GET");
                let (status, header, body) = match parsed.path() {
                    "/start" => (302, ("location", "/article"), ""),
                    "/article" => (200, ("content-type", "text/html; charset=utf-8"), r#"
                        <title>Browser fixture</title><main><p id="text">Before script</p>
                        <a href="/next">Continue reading</a><a href="http://127.0.0.1/">Local admin</a>
                        <script src="/render.js"></script>
                        <script>console.error('Fixture console failure'); fetch('/post', {method:'POST',body:'x'}).catch(()=>{})</script>
                        </main>"#),
                    "/render.js" => (200, ("content-type", "text/javascript"), "document.querySelector('#text').innerText = 'Rendered briefing'"),
                    _ => (404, ("content-type", "text/plain"), ""),
                };
                Ok(crate::public_web::Response {
                    status, headers: vec![(header.0.into(), header.1.into())], bytes: body.as_bytes().to_vec(),
                })
            },
        ).unwrap();
        assert!(
            result.contains("Public page: https://www.example.com/article"),
            "{result}"
        );
        assert!(
            result.contains("Rendered briefing") && !result.contains("Before script"),
            "{result}"
        );
        assert!(
            result.contains("https://www.example.com/next")
                && !result.contains("http://127.0.0.1/"),
            "{result}"
        );
        assert!(requests.iter().any(|(url, _)| url.ends_with("/render.js")));
        assert!(requests.iter().all(|(_, method)| method == "GET"));
        assert!(result.contains("Fixture console failure"), "{result}");
        assert!(
            result.contains("GET https://www.example.com/render.js [Script] → HTTP 200"),
            "{result}"
        );
        assert!(
            result.lines().any(
                |line| line.starts_with("POST https://www.example.com/post ")
                    && line.contains(" → blocked")
            ),
            "{result}"
        );
    }

    #[test]
    fn reports_console_errors_network_and_script_results() {
        let Ok(exe) = resolve("") else {
            eprintln!("no browser here; skipping");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let page = dir.path().join("game.html");
        std::fs::write(
            &page,
            "<!doctype html><title>Snake</title><script src=\"missing.js\"></script><script>\nconsole.log('ready', 3);\nwindow.score = 0;\nfunction step(){ undefinedThing(); }\nsetTimeout(step, 10);\n</script>",
        )
        .unwrap();
        let r = check(
            &exe,
            &page,
            Some("window.score + 41"),
            Duration::from_millis(500),
            false,
        )
        .unwrap();
        let text = r.text(dir.path());
        assert_eq!(r.title, "Snake");
        assert!(text.contains("[log] ready 3"), "{text}");
        assert!(text.contains("undefinedThing is not defined"), "{text}");
        assert!(text.contains("(game.html:4)"), "{text}");
        assert!(text.contains("missing.js"), "{text}");
        assert!(text.contains("Script result: 41"), "{text}");
        assert!(!text.contains(&dir.path().display().to_string()), "{text}");
        assert!(
            r.screenshot
                .as_ref()
                .is_some_and(|png| png.starts_with(b"\x89PNG"))
        );
        let failed_script = check(&exe, &page, Some("throw new Error('interaction test failed')"),
            Duration::from_millis(100), false).unwrap();
        assert!(!failed_script.ok() && failed_script.errors.iter().any(|error| error.contains("Test script: Error: interaction test failed")));
        // With web traffic blocked, an internet request fails while the page still runs.
        std::fs::write(&page, "<!doctype html><title>Net</title><script>fetch('https://example.com/x').catch(e => console.log('fetch failed'));</script>").unwrap();
        let r = check(&exe, &page, None, Duration::from_millis(800), true).unwrap();
        let text = r.text(dir.path());
        assert!(
            text.contains("example.com/x") && text.contains("BLOCKED"),
            "{text}"
        );
    }
}
