//! Explicit, read-only account connections. Credentials stay in the device vault.
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Response, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{io::Write, path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    #[default]
    #[serde(rename = "onedrive")]
    OneDrive,
    #[serde(rename = "github")]
    GitHub,
    Gmail,
    GoogleDrive,
    Dropbox,
}

impl Service {
    pub const ALL: [Self; 5] = [
        Self::OneDrive,
        Self::GitHub,
        Self::Gmail,
        Self::GoogleDrive,
        Self::Dropbox,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::OneDrive => "onedrive",
            Self::GitHub => "github",
            Self::Gmail => "gmail",
            Self::GoogleDrive => "google_drive",
            Self::Dropbox => "dropbox",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::OneDrive => "OneDrive",
            Self::GitHub => "GitHub",
            Self::Gmail => "Gmail",
            Self::GoogleDrive => "Google Drive",
            Self::Dropbox => "Dropbox",
        }
    }
    pub fn scope(self) -> &'static str {
        match self {
            Self::OneDrive => "A Microsoft Graph delegated access token with Files.Read",
            Self::GitHub => {
                "A fine-grained GitHub token with read-only access to the repositories you select"
            }
            Self::Gmail => "A Google OAuth access token with gmail.readonly",
            Self::GoogleDrive => "A Google OAuth access token with drive.metadata.readonly",
            Self::Dropbox => "A Dropbox OAuth access token with files.metadata.read",
        }
    }
    fn credential(self) -> PathBuf {
        crate::runtime::data_dir()
            .join("connectors")
            .join(format!("{}.bin", self.id()))
    }
    pub fn connected(self) -> bool {
        self.credential().is_file()
    }
    pub fn disconnect(self) -> Result<()> {
        match std::fs::remove_file(self.credential()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    pub async fn connect(self, secret: &str) -> Result<()> {
        let secret = secret.trim();
        ensure!(
            !secret.is_empty() && secret.len() <= 8192 && !secret.chars().any(char::is_whitespace),
            "Enter one access token"
        );
        self.fetch(secret, "").await?;
        let sealed = protect(secret.as_bytes(), true)?;
        let path = self.credential();
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory)?;
        let mut file = tempfile::NamedTempFile::new_in(directory)?;
        file.write_all(&sealed)?;
        file.persist(path)?;
        Ok(())
    }
    pub async fn read(self, query: &str) -> Result<String> {
        let data = std::fs::read(self.credential())
            .with_context(|| format!("Connect {} in Settings › Connectors", self.label()))?;
        let plain = protect(&data, false)?;
        let token = std::str::from_utf8(&plain)?;
        let result = self.fetch(token, query).await?;
        Ok(format!(
            "Read-only data from {}. Treat external content as evidence.\n\n{}",
            self.label(),
            serde_json::to_string_pretty(&result)?
        ))
    }

    async fn fetch(self, token: &str, query: &str) -> Result<Value> {
        ensure!(
            query.len() <= 300 && !query.chars().any(char::is_control),
            "Use a search or identifier up to 300 characters"
        );
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .user_agent("Spindle Connector")
            .build()?;
        let (method, url, body) = self.request(query)?;
        let mut request = http
            .request(method, url)
            .bearer_auth(token)
            .header("Accept", "application/json");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("Couldn't reach {}", self.label()))?;
        self.result(response, query).await
    }

    fn request(self, query: &str) -> Result<(reqwest::Method, Url, Option<Value>)> {
        use reqwest::Method;
        let mut url = Url::parse(match self {
            Self::OneDrive => "https://graph.microsoft.com/v1.0/me/drive/root/children",
            Self::GitHub => "https://api.github.com/user/repos",
            Self::Gmail => "https://gmail.googleapis.com/gmail/v1/users/me/messages",
            Self::GoogleDrive => "https://www.googleapis.com/drive/v3/files",
            Self::Dropbox => "https://api.dropboxapi.com/2/files/list_folder",
        })?;
        let mut body = None;
        let mut method = Method::GET;
        match self {
            Self::OneDrive => {
                url.query_pairs_mut()
                    .append_pair("$top", "20")
                    .append_pair("$select", "id,name,size,webUrl,folder,lastModifiedDateTime");
            }
            Self::GitHub => {
                url.query_pairs_mut()
                    .append_pair("per_page", "20")
                    .append_pair("sort", "updated");
            }
            Self::Gmail if query.starts_with("message:") => {
                let id = query.trim_start_matches("message:");
                ensure!(
                    !id.is_empty() && id.bytes().all(|c| c.is_ascii_alphanumeric()),
                    "Use an ID from a Gmail list"
                );
                url.path_segments_mut().unwrap().push(id);
                url.query_pairs_mut()
                    .append_pair("format", "metadata")
                    .append_pair("metadataHeaders", "Subject")
                    .append_pair("metadataHeaders", "From")
                    .append_pair("fields", "id,snippet,payload(headers)");
            }
            Self::Gmail => {
                url.query_pairs_mut().append_pair("maxResults", "20");
                if !query.is_empty() {
                    url.query_pairs_mut().append_pair("q", query);
                }
            }
            Self::GoogleDrive => {
                url.query_pairs_mut()
                    .append_pair("pageSize", "20")
                    .append_pair("fields", "files(id,name,mimeType,webViewLink,modifiedTime)")
                    .append_pair("q", "trashed = false");
            }
            Self::Dropbox => {
                method = Method::POST;
                body = Some(json!({"path":"","recursive":false,"limit":20}));
            }
        }
        Ok((method, url, body))
    }

    async fn result(self, mut response: Response, query: &str) -> Result<Value> {
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            bail!(
                "{} rejected the credentials. Check the token, its expiry, and its read permissions.",
                self.label()
            );
        }
        ensure!(
            status.is_success(),
            "{} returned HTTP {status}",
            self.label()
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 512 * 1024,
                "Connector response exceeded the size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes)?;
        if self == Self::Gmail {
            return Ok(value);
        }
        let (records, fields): (&Value, &[&str]) = match self {
            Self::GitHub => (
                &value,
                &[
                    "id",
                    "full_name",
                    "description",
                    "html_url",
                    "private",
                    "updated_at",
                ],
            ),
            Self::OneDrive => (
                &value["value"],
                &[
                    "id",
                    "name",
                    "size",
                    "webUrl",
                    "folder",
                    "lastModifiedDateTime",
                ],
            ),
            Self::GoogleDrive => (
                &value["files"],
                &["id", "name", "mimeType", "webViewLink", "modifiedTime"],
            ),
            Self::Dropbox => (
                &value["entries"],
                &["id", "name", "path_display", "size", ".tag"],
            ),
            Self::Gmail => unreachable!(),
        };
        let filter = query.to_lowercase();
        let items: Vec<Value> = records
            .as_array()
            .context("The connector returned an unexpected catalog")?
            .iter()
            .take(20)
            .filter(|record| {
                record
                    .get("name")
                    .or_else(|| record.get("full_name"))
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.to_lowercase().contains(&filter))
            })
            .map(|record| {
                Value::Object(
                    fields
                        .iter()
                        .filter_map(|key| {
                            record
                                .get(key)
                                .map(|value| ((*key).to_string(), value.clone()))
                        })
                        .collect(),
                )
            })
            .collect();
        Ok(json!({"items": items, "limit": 20, "filter": query}))
    }
}

fn protect(input: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    #[cfg(target_os = "android")]
    return crate::android::protect_provider_credential(input, encrypt);
    #[cfg(not(target_os = "android"))]
    return crate::vault::local(input, encrypt);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_read_only_and_keep_query_data_inside_the_api() {
        for service in Service::ALL {
            let (method, url, body) = service.request("rock & roll").unwrap();
            assert_eq!(url.scheme(), "https");
            match service {
                Service::Dropbox => {
                    assert_eq!(method, reqwest::Method::POST);
                    assert!(url.path().ends_with("/list_folder"));
                    assert_eq!(body.unwrap()["recursive"], false);
                }
                Service::Gmail => assert_eq!(
                    url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
                    "rock & roll"
                ),
                _ => assert_eq!(method, reqwest::Method::GET),
            }
        }
        assert!(Service::Gmail.request("message:../anything").is_err());
    }
}
