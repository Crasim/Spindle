//! Native Messaging carries tab requests and results. Only the host can see the private IPC endpoint.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub const HOST: &str = "org.spindleapp.browser_link";
pub const ORIGIN: &str = "chrome-extension://hlnagoiglillplcmaefcddjemiefpnoa/";
const LIFETIME: i64 = 8;

#[derive(Clone, Serialize, Deserialize)]
pub struct BrowserClient {
    pub id: String,
    pub browser: crate::browser::LinkBrowser,
    #[serde(default)]
    pub version: Option<String>,
    pub ready: bool,
    pub has_tab: bool,
    #[serde(default)]
    pub guided: bool,
    #[serde(default)]
    pub managed: bool,
    #[serde(default)]
    pub located: Option<Located>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Located {
    pub key: String,
    /// Most recent visit to the requested page, or zero if this profile has no match.
    pub last_visit: u64,
}

#[derive(Serialize, Deserialize)]
struct Lookup {
    key: String,
    page: String,
    updated: i64,
}

impl BrowserClient {
    pub fn regular_profile(&self) -> bool {
        // Older extensions did not identify Spindle's separate browser profile.
        self.guided && !self.managed
    }

    pub fn can_connect(&self, destination: Option<&reqwest::Url>) -> bool {
        self.regular_profile() && self.ready && self.up_to_date()
            && (destination.is_some() || self.has_tab)
    }

    pub fn up_to_date(&self) -> bool {
        let required = extension_version_parts(crate::browser_link::extension_version())
            .expect("Bundled Browser Link version is valid");
        self.version.as_deref().and_then(extension_version_parts)
            .is_some_and(|version| version >= required)
    }
}

fn extension_version_parts(value: &str) -> Option<[u16; 4]> {
    let mut parts = [0; 4];
    for (index, value) in value.split('.').enumerate() {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *parts.get_mut(index)? = value.parse().ok()?;
    }
    Some(parts)
}

#[derive(Serialize, Deserialize)]
struct Presence {
    client: BrowserClient,
    updated: i64,
}

#[derive(Serialize, Deserialize)]
struct Offer {
    id: String,
    client: String,
    updated: i64,
    #[serde(default)]
    page: Option<String>,
}

fn bridge() -> PathBuf {
    crate::runtime::data_dir()
        .join("browser-link")
        .join("native-v2")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SetupMarker { Prepared, Connected }

fn setup_marker(browser: crate::browser::LinkBrowser, marker: SetupMarker) -> PathBuf {
    let status = match marker {
        SetupMarker::Prepared => "prepared",
        SetupMarker::Connected => "connected",
    };
    bridge().join("setup").join(format!("{}-{status}.json", browser.key()))
}

pub fn setup_started(browser: crate::browser::LinkBrowser) -> bool {
    setup_marker(browser, SetupMarker::Prepared).is_file()
}

pub fn connected_before(browser: crate::browser::LinkBrowser) -> bool {
    setup_marker(browser, SetupMarker::Connected).is_file()
}

fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn write_file(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("Missing native bridge directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(parent)?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&temp, serde_json::to_vec(value)?)?;
    fs::rename(temp, path)?;
    Ok(())
}

fn read_file<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(8192)
        .read_to_end(&mut bytes)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn fresh(updated: i64) -> bool {
    let now = crate::automation::now();
    (now - LIFETIME..=now + 1).contains(&updated)
}

fn presence_path(id: &str) -> PathBuf {
    bridge().join("clients").join(format!("{id}.json"))
}

fn offer_path(id: &str) -> PathBuf {
    bridge().join("offers").join(format!("{id}.json"))
}

fn lookup_path(id: &str) -> PathBuf {
    bridge().join("lookup").join(format!("{id}.json"))
}

/// Ask connected profiles about this approved page only. No tab inventory leaves the browser.
pub fn find_profile<'a>(ready: &[&'a BrowserClient], page: &reqwest::Url, request: u64, elapsed: std::time::Duration) -> Result<Option<&'a BrowserClient>> {
    let key = crate::tools::hash(format!("{}:{request}:{page}", std::process::id()).as_bytes());
    let mut missing = false;
    for client in ready {
        ensure!(client.up_to_date(), "Update Browser Link before looking up tabs");
        if client.located.as_ref().is_some_and(|result| result.key == key) { continue; }
        missing = true;
        let path = lookup_path(&client.id);
        if !read_file::<Lookup>(&path).is_some_and(|q| q.key == key && fresh(q.updated)) {
            write_file(&path, &Lookup { key: key.clone(), page: page.to_string(), updated: crate::automation::now() })?;
        }
    }
    if missing && elapsed < std::time::Duration::from_secs(5) { return Ok(None); }
    Ok(ready.iter().copied().max_by_key(|client| {
        let visit = client.located.as_ref().filter(|result| result.key == key).map_or(0, |result| result.last_visit);
        visit
    }))
}

pub fn clients() -> Vec<BrowserClient> {
    let Ok(entries) = fs::read_dir(bridge().join("clients")) else {
        return vec![];
    };
    let mut result: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                return None;
            }
            let record: Presence = read_file(&path)?;
            if !fresh(record.updated) {
                let _ = fs::remove_file(&path);
                return None;
            }
            if !valid_id(&record.client.id) || !record.client.regular_profile() {
                return None;
            }
            Some(record.client)
        })
        .collect();
    result.sort_by(|a, b| {
        a.browser
            .label()
            .cmp(b.browser.label())
            .then(a.id.cmp(&b.id))
    });
    result
}

pub fn offer(browser: &BrowserClient, link: &crate::browser_link::Link) -> Result<()> {
    ensure!(browser.can_connect(link.destination.as_ref()), "Browser Link is unavailable or needs an update");
    let client = &browser.id;
    ensure!(valid_id(client), "Invalid browser client");
    let path = offer_path(client);
    if let Some(current) = read_file::<Offer>(&path)
        && fresh(current.updated)
    {
        ensure!(
            current.id == link.id,
            "This browser is connecting to another Spindle chat"
        );
        if crate::automation::now() - current.updated < 2 {
            return Ok(());
        }
    }
    write_file(
        &path,
        &Offer {
            id: link.id.clone(),
            client: client.into(),
            updated: crate::automation::now(),
            page: link.destination.as_ref().map(ToString::to_string),
        },
    )
}

pub fn withdraw(client: &str, link: &crate::browser_link::Link) {
    if valid_id(client) {
        let path = offer_path(client);
        if read_file::<Offer>(&path).is_some_and(|offer| offer.id == link.id) {
            let _ = fs::remove_file(path);
        }
    }
}

fn exchange(client: BrowserClient) -> Result<Option<Offer>> {
    ensure!(valid_id(&client.id), "Invalid browser client");
    if client.regular_profile() {
        let receipt = setup_marker(client.browser, SetupMarker::Connected);
        if !receipt.is_file() { let _ = write_file(&receipt, &true); }
    }
    let offer = (client.regular_profile() && client.ready && client.up_to_date())
        .then(|| read_file::<Offer>(&offer_path(&client.id)))
        .flatten()
        .filter(|offer| offer.client == client.id && fresh(offer.updated)
            && if offer.page.is_some() { client.guided } else { client.has_tab });
    let path = presence_path(&client.id);
    write_file(
        &path,
        &Presence {
            client,
            updated: crate::automation::now(),
        },
    )?;
    Ok(offer.filter(|offer| valid_id(&offer.id)))
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn host(_: &str) -> Result<()> {
    anyhow::bail!("Browser Link is available on Windows and Linux")
}

#[cfg(any(windows, target_os = "linux"))]
pub fn host(origin: &str) -> Result<()> {
    ensure!(
        origin.trim_end_matches('/') == ORIGIN.trim_end_matches('/'),
        "Browser Link received an unexpected extension origin"
    );
    let (mut_input, input) = tokio::sync::mpsc::channel(8);
    std::thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        while let Ok(frame) = native_read(&mut stdin) {
            if mut_input.blocking_send(frame).is_err() {
                break;
            }
        }
    });
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(relay(input, &mut io::stdout().lock()))
}

#[cfg(any(windows, target_os = "linux"))]
fn native_read(input: &mut impl Read) -> Result<serde_json::Value> {
    let mut header = [0; 4];
    input.read_exact(&mut header)?;
    let length = u32::from_ne_bytes(header) as usize;
    ensure!(
        (1..=64 * 1024).contains(&length),
        "Invalid native browser message size"
    );
    let mut data = vec![0; length];
    input.read_exact(&mut data)?;
    Ok(serde_json::from_slice(&data)?)
}

#[cfg(any(windows, target_os = "linux"))]
fn native_write(output: &mut impl Write, value: &serde_json::Value) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    output.write_all(&(data.len() as u32).to_ne_bytes())?;
    output.write_all(&data)?;
    output.flush()?;
    Ok(())
}

#[cfg(any(windows, target_os = "linux"))]
struct Relay {
    id: String,
    writer: tokio::io::WriteHalf<crate::browser_channel::Client>,
    reader: tokio::task::JoinHandle<()>,
}

#[cfg(any(windows, target_os = "linux"))]
impl Drop for Relay {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

#[cfg(any(windows, target_os = "linux"))]
async fn relay(
    mut input: tokio::sync::mpsc::Receiver<serde_json::Value>,
    output: &mut impl Write,
) -> Result<()> {
    use crate::browser_channel::{connect, read, write};
    use serde_json::{Value, json};
    let (send, mut received) = tokio::sync::mpsc::channel::<(String, Option<Value>)>(8);
    let mut requested = None::<Offer>;
    let mut bound = None::<Relay>;
    let mut last = String::new();
    loop {
        tokio::select! {
            event = input.recv() => {
                let Some(event) = event else { return Ok(()) };
                if let Some(client) = event.get("browser") {
                    let client: BrowserClient = serde_json::from_value(client.clone())?;
                    ensure!(valid_id(&client.id), "Invalid browser client");
                    let current = client.regular_profile() && client.up_to_date();
                    if !current && (requested.take().is_some() || bound.is_some()) {
                        bound = None;
                        native_write(output, &json!({"disconnected":true}))?;
                    }
                    if current && let Some(lookup) = read_file::<Lookup>(&lookup_path(&client.id)) {
                        if fresh(lookup.updated) && !client.located.as_ref().is_some_and(|result| result.key == lookup.key) {
                            native_write(output, &json!({"locate": lookup.page, "key": lookup.key}))?;
                        } else {
                            let _ = fs::remove_file(lookup_path(&client.id));
                        }
                    }
                    let next = exchange(client)?;
                    if requested.is_none() && let Some(offer) = next.filter(|offer| offer.id != last) {
                        bound = None;
                        native_write(output, &json!({"connect":true,"page":offer.page}))?;
                        requested = Some(offer);
                    }
                } else if let Some(tab) = event.get("tab") {
                    let Some(offer) = requested.take() else { continue };
                    if read_file::<Offer>(&offer_path(&offer.client)).is_some_and(|o| o.id == offer.id) {
                        let _ = fs::remove_file(offer_path(&offer.client));
                    }
                    last.clone_from(&offer.id);
                    let stream = async {
                        let mut stream = connect(&offer.id).await?;
                        write(&mut stream, &json!({"url":tab,"error":event.get("error")})).await?;
                        Ok::<_, anyhow::Error>(stream)
                    }.await;
                    match stream {
                        Ok(stream) => {
                            let (mut stream, writer) = tokio::io::split(stream);
                            let id = offer.id;
                            let source = id.clone();
                            let send = send.clone();
                            let reader = tokio::spawn(async move {
                                while let Ok(frame) = read(&mut stream).await {
                                    if send.send((source.clone(), Some(frame))).await.is_err() { return; }
                                }
                                let _ = send.send((source, None)).await;
                            });
                            bound = Some(Relay { id, writer, reader });
                        }
                        Err(_) => native_write(output, &json!({"disconnected":true}))?,
                    }
                } else if event["disconnect"] == true {
                    bound = None;
                    requested = None;
                } else if let Some(packet) = event.get("packet")
                    && let Some(relay) = &mut bound
                    && write(&mut relay.writer, packet).await.is_err()
                {
                    bound = None;
                    native_write(output, &json!({"disconnected":true}))?;
                }
            }
            message = received.recv() => {
                let Some((source, message)) = message else { return Ok(()) };
                if bound.as_ref().is_none_or(|relay| relay.id != source) { continue; }
                match message {
                    Some(packet) => native_write(output, &json!({"packet":packet}))?,
                    None => {
                        bound = None;
                        native_write(output, &json!({"disconnected":true}))?;
                    }
                }
            }
        }
    }
}

pub fn install(browser: crate::browser::LinkBrowser) -> Result<()> {
    use crate::browser::LinkBrowser::{Chrome, Edge};
    let executable = std::env::current_exe()?;
    let manifest = serde_json::json!({
        "name": HOST,
        "description": "Spindle Browser Link",
        "path": executable,
        "type": "stdio",
        "allowed_origins": [ORIGIN],
    });
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let path = bridge().join(format!("{HOST}.json"));
        write_file(&path, &manifest)?;
        let vendor = match browser {
            Edge => r"Microsoft\Edge",
            Chrome => r"Google\Chrome",
        };
        let key = format!(r"HKCU\Software\{vendor}\NativeMessagingHosts\{HOST}");
        let system =
            std::env::var_os("SystemRoot").context("Cannot locate the Windows system folder")?;
        let status =
            std::process::Command::new(PathBuf::from(system).join("System32").join("reg.exe"))
                .args(["add", &key, "/ve", "/t", "REG_SZ", "/d"])
                .arg(&path)
                .args(["/f"])
                .creation_flags(0x0800_0000)
                .stdout(std::process::Stdio::null())
                .status()?;
        ensure!(
            status.success(),
            "Could not register Browser Link for {}",
            browser.label()
        );
        let _ = write_file(&setup_marker(browser, SetupMarker::Prepared), &true);
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .context("Cannot find the browser configuration directory")?;
        let browser_dir = match browser {
            Edge => "microsoft-edge",
            Chrome => "google-chrome",
        };
        write_file(
            &config.join(browser_dir).join("NativeMessagingHosts").join(format!("{HOST}.json")),
            &manifest,
        )?;
        let _ = write_file(&setup_marker(browser, SetupMarker::Prepared), &true);
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (browser, manifest, Chrome, Edge);
        anyhow::bail!("Browser Link is available on Windows and Linux")
    }
}
