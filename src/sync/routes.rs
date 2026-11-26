//! Addresses travel in signed, encrypted sync records. Discovery broadcasts carry only
//! the peer's listener port. A route is a hint and never replaces the device handshake.

use super::core::Kind;
use super::space::Space;
use anyhow::Result;
use serde_json::{Value, json};
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_NAMES: usize = 4;
const MAX_IPS: usize = 12;
const REFRESH: Duration = Duration::from_secs(60);

pub(super) fn endpoint(text: &str, default_port: u16) -> Option<String> {
    let text = text.trim();
    if let Ok(addr) = text.parse::<SocketAddr>() {
        return (addr.port() != 0).then(|| addr.to_string());
    }
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, default_port).to_string());
    }
    let (host, port) = match text.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().ok().filter(|p| *p != 0)?),
        None => (text, default_port),
    };
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    (super::provider_sync::valid_hostname(&host) || host == "localhost")
        .then(|| format!("{host}:{port}"))
}

/// Reserve space for DNS so a succession of IP changes can't eject stable names.
pub(super) fn merge(first: &str, previous: &str) -> String {
    let mut names = 0;
    let mut ips = 0;
    let mut kept = Vec::new();
    for item in first.split(',').chain(previous.split(',')) {
        let Some(addr) = endpoint(item, super::peer::PORT) else {
            continue;
        };
        if kept.contains(&addr) {
            continue;
        }
        let (used, cap) = if addr.parse::<SocketAddr>().is_ok() {
            (&mut ips, MAX_IPS)
        } else {
            (&mut names, MAX_NAMES)
        };
        if *used < cap {
            *used += 1;
            kept.push(addr);
        }
    }
    kept.join(",")
}

#[derive(Default)]
struct DnsCache {
    last: Option<Instant>,
    interfaces: Vec<IpAddr>,
    name: Option<String>,
    vpn_ips: Vec<IpAddr>,
}

static DNS: Mutex<DnsCache> = Mutex::new(DnsCache {
    last: None,
    interfaces: Vec::new(),
    name: None,
    vpn_ips: Vec::new(),
});

/// Refresh discovery off the UI thread. The system/Tailscale name is never inferred from
/// a Spindle display label, which is free-form and may be different from the DNS name.
pub(super) async fn refresh_dns() {
    let interfaces = super::pair::local_ips();
    {
        let mut cache = DNS.lock().unwrap();
        if cache.interfaces == interfaces && cache.last.is_some_and(|t| t.elapsed() < REFRESH) {
            return;
        }
        cache.last = Some(Instant::now());
        cache.interfaces.clone_from(&interfaces);
    }
    let result = tailscale(&interfaces).await;
    let mut cache = DNS.lock().unwrap();
    // Another caller may have started discovery for a newer network.
    if cache.interfaces == interfaces {
        (cache.name, cache.vpn_ips) = result;
    }
}

fn magic_name(name: &str) -> Option<String> {
    let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
    (name.ends_with(".ts.net") && super::provider_sync::valid_hostname(&name)).then_some(name)
}

fn tailscale_ipv4(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
}

#[cfg(any(windows, target_os = "linux", test))]
fn tailscale_status(status: &Value) -> (Option<String>, Vec<IpAddr>) {
    if status["BackendState"] != "Running" {
        return (None, Vec::new());
    }
    let me = &status["Self"];
    let name = me["DNSName"].as_str().and_then(magic_name);
    let ips = me["TailscaleIPs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|ip| ip.as_str()?.parse::<IpAddr>().ok())
        .filter(|ip| ip.is_ipv4())
        .collect();
    (name, ips)
}

#[cfg(any(windows, target_os = "linux"))]
async fn tailscale(_interfaces: &[IpAddr]) -> (Option<String>, Vec<IpAddr>) {
    #[cfg(windows)]
    let mut cmd = {
        let Some(programs) = std::env::var_os("ProgramFiles") else {
            return Default::default();
        };
        let mut cmd = tokio::process::Command::new(
            std::path::PathBuf::from(programs)
                .join("Tailscale")
                .join("tailscale.exe"),
        );
        cmd.creation_flags(0x0800_0000);
        cmd
    };
    #[cfg(target_os = "linux")]
    let mut cmd = tokio::process::Command::new("tailscale");
    cmd.args(["status", "--json", "--peers=false"])
        .kill_on_drop(true);
    let Ok(Ok(result)) = tokio::time::timeout(Duration::from_secs(2), cmd.output()).await else {
        return Default::default();
    };
    if result.status.success() {
        if let Ok(status) = serde_json::from_slice(&result.stdout) {
            return tailscale_status(&status);
        }
    }
    Default::default()
}

#[cfg(target_os = "android")]
async fn tailscale(interfaces: &[IpAddr]) -> (Option<String>, Vec<IpAddr>) {
    let Some(ip) = interfaces.iter().find(|ip| tailscale_ipv4(ip)).copied() else {
        return Default::default();
    };
    let task = tokio::task::spawn_blocking(move || {
        crate::android::java(|env, _| {
            let ip = env.new_string(ip.to_string())?;
            let addr = env
                .call_static_method(
                    "java/net/InetAddress",
                    "getByName",
                    "(Ljava/lang/String;)Ljava/net/InetAddress;",
                    &[(&ip).into()],
                )?
                .l()?;
            let name = env
                .call_method(addr, "getCanonicalHostName", "()Ljava/lang/String;", &[])?
                .l()?;
            let name: String = env.get_string(&name.into())?.into();
            Ok(magic_name(&name))
        })
        .ok()
        .flatten()
    });
    let name = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    (name, Vec::new())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android")))]
async fn tailscale(_interfaces: &[IpAddr]) -> (Option<String>, Vec<IpAddr>) {
    Default::default()
}

pub(super) fn local_endpoints(port: u16, override_name: &str) -> String {
    let ips = super::pair::local_ips();
    let cache = DNS.lock().unwrap();
    let system = if cfg!(target_os = "android") {
        String::new()
    } else {
        super::live::hostname()
    };
    let names = [
        override_name,
        cache.name.as_deref().unwrap_or(""),
        system.as_str(),
    ];
    let names = names
        .into_iter()
        .filter(|name| super::provider_sync::valid_hostname(name))
        .map(|name| format!("{name}:{port}"))
        .collect::<Vec<_>>()
        .join(",");
    let prioritized = ips
        .first()
        .into_iter()
        .chain(&cache.vpn_ips)
        .chain(ips.iter().filter(|ip| tailscale_ipv4(ip)))
        .map(|ip| SocketAddr::new(*ip, port).to_string())
        .collect::<Vec<_>>()
        .join(",");
    let remaining = ips
        .iter()
        .map(|ip| SocketAddr::new(*ip, port).to_string())
        .collect::<Vec<_>>()
        .join(",");
    merge(&merge(&prioritized, &names), &remaining)
}

fn key(id: &str) -> String {
    format!("peer-routes:{id}")
}

fn signed_routes(space: &Space, device: &str) -> Option<Value> {
    space
        .item(Kind::Settings, &key(device))
        .iter()
        .rev()
        .find(|(_, envelope, _)| envelope.device == device)
        .filter(|(_, _, body)| !body.deleted)
        .map(|(_, _, body)| body.value.clone())
}

pub(super) fn publish(space: &mut Space, addresses: &str) -> Result<()> {
    let device = &space.id.device.id;
    let value = json!(addresses);
    let previous = signed_routes(space, device);
    if previous.as_ref() != Some(&value) && (!addresses.is_empty() || previous.is_some()) {
        space.put_changes(Kind::Settings, &key(device), value, vec![])?;
    }
    Ok(())
}

pub(super) fn advertised(space: &Space, device: &str) -> String {
    let value = signed_routes(space, device);
    let list = value.as_ref().and_then(Value::as_str).unwrap_or("");
    // Only routable candidate hosts from the signed record. Loopback can still be
    // explicitly added by a user, but isn't accepted as someone else's address.
    let list = list
        .split(',')
        .take(MAX_NAMES + MAX_IPS)
        .filter_map(|text| endpoint(text, super::peer::PORT))
        .filter(|addr| {
            if let Ok(socket) = addr.parse::<SocketAddr>() {
                let ip = socket.ip();
                !(ip.is_loopback() || ip.is_unspecified() || ip.is_multicast())
            } else {
                !addr.starts_with("localhost:")
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    merge(&list, "")
}

#[cfg(test)]
mod tests {
    use super::super::space::tests::join;
    use super::super::transport::{Hub, Progress, Transport, sync};
    use super::*;

    #[test]
    fn keeps_dns_names_when_ips_change() {
        assert_eq!(
            endpoint(" Mobile.tail-demo.TS.NET.:47613 ", 1).as_deref(),
            Some("mobile.tail-demo.ts.net:47613")
        );
        assert!(endpoint("https://mobile.example:47613/", 1).is_none());
        assert!(endpoint("mobile.example:0", 1).is_none());
        let ips = (1..=25)
            .map(|i| format!("192.0.2.{i}:47613"))
            .collect::<Vec<_>>()
            .join(",");
        let merged = merge(&ips, "mobile.tail-demo.ts.net:47613");
        assert_eq!(merged.split(',').count(), MAX_IPS + 1);
        assert!(merged.ends_with("mobile.tail-demo.ts.net:47613"));
    }

    #[test]
    fn tailscale_discovery_uses_only_self() {
        let (name, ips) = tailscale_status(&json!({
            "BackendState": "Running",
            "Self": {"DNSName": "device.tail-demo.ts.net.", "TailscaleIPs": ["100.80.10.3", "fd7a:115c:a1e0::1"]},
            "Peer": {"other": {"DNSName": "other.tail-demo.ts.net."}}
        }));
        assert_eq!(name.as_deref(), Some("device.tail-demo.ts.net"));
        assert_eq!(ips, vec!["100.80.10.3".parse::<IpAddr>().unwrap()]);
        assert_eq!(
            tailscale_status(&json!({"BackendState": "Stopped"})),
            (None, vec![])
        );
    }

    #[tokio::test]
    async fn route_changes_cross_sync_and_only_the_device_can_publish_its_routes() -> Result<()> {
        let mut desktop = Space::create("Host", None)?;
        let mut phone = join(&desktop, "Phone", true);
        desktop.add_member(phone.member_entry()?)?;
        let id = desktop.id.device.id.clone();
        let transport = Transport::Memory(Hub::default());
        let mut sent = Progress::default();
        let mut received = Progress::default();
        let home = "192.0.2.1:47613,desktop.tail-demo.ts.net:47613";
        publish(&mut desktop, home)?;
        sync(&mut desktop, &transport, &mut sent).await?;
        sync(&mut phone, &transport, &mut received).await?;
        assert_eq!(advertised(&phone, &id), home);

        let vpn = "100.80.10.3:47613,desktop.tail-demo.ts.net:47613";
        publish(&mut desktop, vpn)?;
        let records = desktop.store.records.len();
        publish(&mut desktop, vpn)?;
        assert_eq!(records, desktop.store.records.len());
        sync(&mut desktop, &transport, &mut sent).await?;
        sync(&mut phone, &transport, &mut received).await?;
        assert_eq!(advertised(&phone, &id), vpn);

        phone.put_changes(
            Kind::Settings,
            &key(&id),
            json!("wrong.example:47613"),
            vec![],
        )?;
        assert_eq!(advertised(&phone, &id), vpn);
        Ok(())
    }
}
