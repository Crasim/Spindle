//! Network access for the model's disposable Chromium tab.
//! Resolve each host once and pin the checked addresses in the actual connector.
use anyhow::{Context, Result, ensure};
use reqwest::{Client, Method, Url};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

pub(crate) fn url(value: &str) -> Result<Url> {
    ensure!(value.len() <= 4096, "The web address is too long");
    let url = Url::parse(value).context("Enter a complete public HTTP or HTTPS URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "Use a public HTTP or HTTPS URL"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none() && url.port().is_none(),
        "Public pages use standard web ports and no credentials in the address"
    );
    let host = url.host_str().context("The URL has no host")?;
    let name = host.trim_end_matches('.').trim_matches(['[', ']']);
    if let Ok(ip) = name.parse::<IpAddr>() {
        ensure!(public_ip(ip), "The address is not on the public web");
    } else {
        ensure!(
            name.contains('.')
                && ![
                    "local",
                    "localhost",
                    "internal",
                    "lan",
                    "home.arpa",
                    "onion"
                ]
                .iter()
                .any(|suffix| name == *suffix || name.ends_with(&format!(".{suffix}"))),
            "Use a public hostname"
        );
    }
    Ok(url)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_ipv4(ip),
        IpAddr::V6(ip) => {
            let [first, second, third, ..] = ip.segments();
            // Only globally allocated unicast. Exclude protocol assignments and
            // transition addresses that can embed an internal IPv4 address.
            (0x2000..0x4000).contains(&first)
                && first != 0x2002
                && !(first == 0x2001 && (second <= 0x1ff || second == 0xdb8))
                && !(first == 0x3fff && second < 0x1000)
                && !(first == 0x2620 && second == 0x004f && third == 0x8000)
        }
    }
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    !matches!(
        ip.octets(),
        [0 | 10 | 127 | 224..=255, ..]
            | [100, 64..=127, ..]
            | [169, 254, ..]
            | [172, 16..=31, ..]
            | [192, 0, 0 | 2, _]
            | [192, 88, 99, _]
            | [192, 168, ..]
            | [198, 18 | 19, ..]
            | [198, 51, 100, _]
            | [203, 0, 113, _]
    )
}

pub(crate) struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct Network {
    clients: HashMap<String, Client>,
}

impl Network {
    pub async fn read(
        &mut self,
        address: &str,
        method: &str,
        accept: Option<&str>,
        max_bytes: usize,
    ) -> Result<Response> {
        let url = url(address)?;
        ensure!(
            matches!(method, "GET" | "HEAD"),
            "Public browsing uses GET and HEAD"
        );
        let host = url.host_str().context("The URL has no host")?;
        let client = if let Some(client) = self.clients.get(host) {
            client
        } else {
            ensure!(self.clients.len() < 32, "The page reaches too many hosts");
            let mut builder = Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(5))
                .user_agent("Mozilla/5.0 (compatible; SpindleBrowser/1.0)");
            if host.trim_matches(['[', ']']).parse::<IpAddr>().is_err() {
                let port = url
                    .port_or_known_default()
                    .context("The URL has no web port")?;
                let addresses = tokio::net::lookup_host((host, port))
                    .await?
                    .collect::<Vec<_>>();
                ensure!(
                    !addresses.is_empty()
                        && addresses.iter().all(|address| public_ip(address.ip())),
                    "The hostname does not resolve exclusively to public addresses"
                );
                // A zero port lets reqwest select 80 or 443 when reusing this host.
                let mut addresses = addresses;
                for address in &mut addresses {
                    address.set_port(0);
                }
                builder = builder.resolve_to_addrs(host, &addresses);
            }
            self.clients
                .entry(host.to_owned())
                .or_insert(builder.build()?)
        };
        let mut request = client
            .request(Method::from_bytes(method.as_bytes())?, url.clone())
            .header("accept-encoding", "identity");
        if let Some(accept) = accept {
            request = request.header("accept", accept);
        }
        let mut response = request.send().await?;
        let status = response.status();
        let headers = response
            .headers()
            .iter()
            .filter(|(name, _)| {
                matches!(
                    name.as_str(),
                    "content-type"
                        | "content-encoding"
                        | "content-language"
                        | "location"
                        | "access-control-allow-origin"
                        | "access-control-expose-headers"
                        | "cross-origin-resource-policy"
                        | "content-security-policy"
                        | "content-security-policy-report-only"
                        | "x-content-type-options"
                )
            })
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.to_string(), value.to_owned()))
            })
            .collect();
        let mut bytes = Vec::new();
        if !status.is_redirection() && method == "GET" {
            ensure!(
                response
                    .content_length()
                    .is_none_or(|length| length <= max_bytes as u64),
                "The web response is too large"
            );
            while let Some(chunk) = response.chunk().await? {
                ensure!(
                    bytes.len() + chunk.len() <= max_bytes,
                    "The web response is too large"
                );
                bytes.extend_from_slice(&chunk);
            }
        }
        Ok(Response {
            status: status.as_u16(),
            headers,
            bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_public_links_and_rejects_internal_and_privileged_urls() {
        for value in [
            "file:///C:/data.txt",
            "http://localhost/",
            "http://test.local/",
            "http://2130706433/",
            "http://192.168.0.1/",
            "http://169.254.169.254/",
            "http://100.100.100.200/",
            "https://[::ffff:127.0.0.1]/",
            "https://[2002:7f00:1::]/",
            "https://user:secret@example.com/",
            "https://example.com:444/",
        ] {
            assert!(url(value).is_err(), "{value}");
        }
        assert!(url("https://example.com/links?q=test#section").is_ok());
        assert!(public_ip("2606:4700::1111".parse().unwrap()));
    }
}
