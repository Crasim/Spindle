//! Opt-in discovery of model and image servers on one computer.
use anyhow::{Context, Result, ensure};
use reqwest::{Client, Url};
use serde_json::Value;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ollama,
    Compatible,
    ComfyUi,
    StableDiffusion,
}

#[derive(Clone)]
pub struct Server {
    pub name: &'static str,
    pub url: String,
    pub kind: Kind,
    pub models: usize,
}

pub fn address(value: &str) -> Result<String> {
    let url = Url::parse(value.trim()).context("Enter a complete HTTP or HTTPS server address")?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Enter an HTTP or HTTPS base address without credentials or query parameters"
    );
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub fn client(timeout: Duration) -> Result<Client> {
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(timeout)
        .timeout(timeout)
        .build()?)
}

pub async fn discover(host: &str) -> Result<Vec<Server>> {
    use Kind::*;
    let host = if host.trim().is_empty() {
        "localhost"
    } else {
        host.trim()
    };
    let base = Url::parse(&format!("http://{host}")).context("Enter a hostname or IP address")?;
    ensure!(
        base.host().is_some()
            && base.port().is_none()
            && base.path() == "/"
            && base.username().is_empty()
            && base.password().is_none()
            && base.query().is_none()
            && base.fragment().is_none(),
        "Enter only the hostname or IP address, such as localhost or your desktop's name"
    );
    let http = client(Duration::from_secs(3))?;
    let mut tasks = tokio::task::JoinSet::new();
    for (order, (name, kind, port, catalog)) in [
        ("Ollama", Ollama, 11434, "/api/tags"),
        ("LM Studio", Compatible, 1234, "/models"),
        ("Maya", Compatible, 18081, "/models"),
        ("OpenAI-compatible · port 8080", Compatible, 8080, "/models"),
        ("OpenAI-compatible · port 8000", Compatible, 8000, "/models"),
        (
            "ComfyUI",
            ComfyUi,
            8188,
            "/object_info/CheckpointLoaderSimple",
        ),
        (
            "Stable Diffusion WebUI",
            StableDiffusion,
            7860,
            "/sdapi/v1/sd-models",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let mut url = base.clone();
        url.set_port(Some(port)).ok();
        if kind == Compatible {
            url.set_path("/v1");
        }
        let url = url.as_str().trim_end_matches('/').to_owned();
        let http = http.clone();
        tasks.spawn(async move {
            let mut response = http
                .get(format!("{url}{catalog}"))
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.ok()? {
                if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                    return None;
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: Value = serde_json::from_slice(&bytes).ok()?;
            let models = count(kind, &value)?;
            Some((
                order,
                Server {
                    name,
                    kind,
                    url,
                    models,
                },
            ))
        });
    }
    let mut servers = Vec::new();
    while let Some(Ok(found)) = tasks.join_next().await {
        servers.extend(found);
    }
    servers.sort_by_key(|(order, _)| *order);
    Ok(servers.into_iter().map(|(_, server)| server).collect())
}

fn count(kind: Kind, value: &Value) -> Option<usize> {
    match kind {
        Kind::Ollama => count_named(&value["models"], "name"),
        Kind::Compatible => count_named(&value["data"], "id"),
        Kind::ComfyUi => value["CheckpointLoaderSimple"]["input"]["required"]["ckpt_name"][0]
            .as_array()
            .map(Vec::len),
        Kind::StableDiffusion => count_named(value, "model_name"),
    }
}

fn count_named(value: &Value, key: &str) -> Option<usize> {
    let array = value.as_array()?;
    array
        .iter()
        .all(|entry| entry[key].is_string())
        .then_some(array.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn discovery_requires_the_expected_catalog_shape() {
        assert_eq!(
            count(Kind::Ollama, &json!({"models":[{"name":"qwen"}]})),
            Some(1)
        );
        assert_eq!(
            count(Kind::Compatible, &json!({"data":[{"id":"qwen"}]})),
            Some(1)
        );
        assert_eq!(
            count(Kind::Compatible, &json!({"data":[{"model":"qwen"}]})),
            None
        );
        assert_eq!(
            count(Kind::StableDiffusion, &json!([{"model_name":"sdxl"}])),
            Some(1)
        );
        assert_eq!(
            count(
                Kind::ComfyUi,
                &json!({"CheckpointLoaderSimple":{"input":{"required":{"ckpt_name":[["sdxl"]]}}}})
            ),
            Some(1)
        );
    }
}
