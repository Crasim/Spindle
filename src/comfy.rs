//! Image generation and editing with ComfyUI.
//!
//! Spindle sends ComfyUI a workflow in its API format: a built-in text-to-image graph for
//! SDXL-style checkpoints (with a source image for edits), or the user's own API workflow. In the
//! user's workflow, text fields written as `{{prompt}}`, `{{negative}}`, `{{seed}}`,
//! `{{width}}` and `{{height}}` are filled in. Spindle then waits for the result and
//! downloads the first image the workflow saved.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long one image may take before Spindle gives up.
const LIMIT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum Backend {
    #[default]
    ComfyUi,
    StableDiffusion,
}

#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub url: String,
    /// Checkpoint for the built-in workflow; empty picks one.
    pub checkpoint: String,
    /// Path to a workflow in ComfyUI's API format; empty uses the built-in one.
    pub workflow: String,
    pub edit_workflow: String,
    pub backend: Backend,
}

impl Settings {
    pub fn from(prefs: &crate::state::Preferences) -> Self {
        Self {
            url: match prefs.image_backend {
                Backend::ComfyUi => &prefs.comfyui,
                Backend::StableDiffusion => &prefs.sd_webui,
            }
            .trim()
            .trim_end_matches('/')
            .to_owned(),
            checkpoint: prefs.comfy_checkpoint.trim().to_owned(),
            workflow: prefs.comfy_workflow.trim().to_owned(),
            edit_workflow: prefs.comfy_edit_workflow.trim().to_owned(),
            backend: prefs.image_backend,
        }
    }
    pub fn ready(&self) -> bool {
        !self.url.is_empty()
    }
    pub fn label(&self) -> &'static str {
        match self.backend {
            Backend::ComfyUi => "ComfyUI",
            Backend::StableDiffusion => "Stable Diffusion WebUI",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub prompt: String,
    pub negative: String,
    pub width: u32,
    pub height: u32,
    pub seed: u64,
}

impl Request {
    pub fn new(prompt: &str) -> Self {
        Self {
            prompt: prompt.trim().to_owned(),
            negative: "blurry, low quality, watermark, text, deformed".into(),
            width: 1024,
            height: 1024,
            seed: seed(),
        }
    }
}

fn seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    (nanos as u64 ^ (nanos >> 64) as u64) % 1_000_000_000_000
}

/// What came back: the image and what made it.
pub struct Image {
    pub png: Vec<u8>,
    pub checkpoint: String,
    pub seconds: u64,
}

pub const MAX_SOURCE_BYTES: usize = 6 * 1024 * 1024;

pub struct Source {
    pub bytes: Vec<u8>,
    pub format: image::ImageFormat,
    dimensions: (u32, u32),
}

impl Source {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        ensure!(bytes.len() <= MAX_SOURCE_BYTES, "Use an image up to 6 MB");
        let format = image::guess_format(&bytes).context("Select a PNG or JPEG to edit")?;
        ensure!(
            matches!(format, image::ImageFormat::Png | image::ImageFormat::Jpeg),
            "Select a PNG or JPEG to edit"
        );
        let dimensions = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format)
            .into_dimensions()
            .context("Read the source image dimensions")?;
        ensure!(
            dimensions.0 <= 4096 && dimensions.1 <= 4096,
            "Use an image up to 4,096 pixels per side"
        );
        Ok(Self {
            bytes,
            format,
            dimensions,
        })
    }
}

fn client() -> Result<reqwest::Client> {
    crate::local_servers::client(Duration::from_secs(60))
}

/// The checkpoints ComfyUI has, for Settings and for picking one.
pub async fn checkpoints(url: &str) -> Result<Vec<String>> {
    let url = url.trim().trim_end_matches('/');
    ensure!(!url.is_empty(), "Set the ComfyUI address in Settings first");
    let info: Value = client()?
        .get(format!("{url}/object_info/CheckpointLoaderSimple"))
        .send()
        .await
        .with_context(|| format!("Couldn't reach ComfyUI at {url}"))?
        .error_for_status()?
        .json()
        .await?;
    Ok(
        info["CheckpointLoaderSimple"]["input"]["required"]["ckpt_name"][0]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// The checkpoint the built-in workflow uses when none is chosen: an SDXL image model.
pub fn pick(names: &[String]) -> Option<String> {
    let lower = |n: &String| n.to_lowercase();
    let image = |n: &&String| {
        let l = lower(n);
        ![
            "inpaint", "audio", "sd3", "ace_step", "refiner", "video", "svd",
        ]
        .iter()
        .any(|w| l.contains(w))
    };
    names
        .iter()
        .filter(image)
        .find(|n| lower(n).contains("sd_xl_base"))
        .or_else(|| names.iter().filter(image).find(|n| lower(n).contains("xl")))
        .or_else(|| names.iter().find(image))
        .cloned()
}

/// The built-in text-to-image workflow (SDXL settings).
fn builtin(checkpoint: &str, r: &Request, source: Option<&str>) -> Value {
    let mut graph = json!({
        "4": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": checkpoint}},
        "5": {"class_type": "EmptyLatentImage", "inputs": {"width": r.width, "height": r.height, "batch_size": 1}},
        "6": {"class_type": "CLIPTextEncode", "inputs": {"text": r.prompt, "clip": ["4", 1]}},
        "7": {"class_type": "CLIPTextEncode", "inputs": {"text": r.negative, "clip": ["4", 1]}},
        "3": {"class_type": "KSampler", "inputs": {
            "seed": r.seed, "steps": 25, "cfg": 6.5, "sampler_name": "dpmpp_2m",
            "scheduler": "karras", "denoise": 1.0,
            "model": ["4", 0], "positive": ["6", 0], "negative": ["7", 0], "latent_image": ["5", 0]
        }},
        "8": {"class_type": "VAEDecode", "inputs": {"samples": ["3", 0], "vae": ["4", 2]}},
        "9": {"class_type": "SaveImage", "inputs": {"filename_prefix": "spindle", "images": ["8", 0]}}
    });
    if let Some(source) = source {
        graph["10"] = json!({"class_type": "LoadImage", "inputs": {"image": source}});
        graph["5"] =
            json!({"class_type": "VAEEncode", "inputs": {"pixels": ["10", 0], "vae": ["4", 2]}});
        graph["3"]["inputs"]["denoise"] = json!(0.6);
    }
    graph
}

/// Fill a user's workflow: `{{prompt}}` and the others become the request's values. A field
/// that is exactly `{{seed}}`, `{{width}}` or `{{height}}` becomes a number.
fn fill(workflow: &Value, r: &Request, source: Option<&str>) -> Value {
    match workflow {
        Value::String(s) => match s.trim() {
            "{{seed}}" => json!(r.seed),
            "{{width}}" => json!(r.width),
            "{{height}}" => json!(r.height),
            "{{image}}" => source.map_or_else(|| workflow.clone(), |name| json!(name)),
            _ => Value::String(
                s.replace("{{prompt}}", &r.prompt)
                    .replace("{{negative}}", &r.negative)
                    .replace("{{seed}}", &r.seed.to_string()),
            ),
        },
        Value::Array(a) => Value::Array(a.iter().map(|v| fill(v, r, source)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), fill(v, r, source)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Make one image. Checks `cancel` while waiting.
pub async fn generate(settings: &Settings, r: &Request, cancel: Arc<AtomicBool>) -> Result<Image> {
    run(settings, r, None, cancel).await
}

pub async fn run(
    settings: &Settings,
    r: &Request,
    source: Option<Source>,
    cancel: Arc<AtomicBool>,
) -> Result<Image> {
    ensure!(
        settings.ready(),
        "Set the image server in Settings › Models › Image generation"
    );
    ensure!(!r.prompt.is_empty(), "Describe the image to make");
    if settings.backend == Backend::StableDiffusion {
        return stable_diffusion(&settings.url, r, source, cancel).await;
    }
    let url = &settings.url;
    let http = client()?;
    let workflow_path = if source.is_some() {
        &settings.edit_workflow
    } else {
        &settings.workflow
    };
    let (graph, checkpoint) = if workflow_path.is_empty() {
        let checkpoint = if settings.checkpoint.is_empty() {
            pick(&checkpoints(url).await?)
                .context("ComfyUI has no image checkpoints (models/checkpoints is empty)")?
        } else {
            settings.checkpoint.clone()
        };
        let uploaded = upload(&http, url, r.seed, source).await?;
        (builtin(&checkpoint, r, uploaded.as_deref()), checkpoint)
    } else {
        let text = std::fs::read_to_string(workflow_path)
            .with_context(|| format!("Couldn't read the workflow {workflow_path}"))?;
        let workflow: Value = serde_json::from_str(&text)
            .context("The workflow isn't valid JSON. In ComfyUI use Workflow › Export (API).")?;
        ensure!(
            workflow
                .as_object()
                .is_some_and(|o| !o.is_empty() && o.values().all(|n| n.get("class_type").is_some())),
            "The workflow isn't in API format. In ComfyUI use Workflow › Export (API)."
        );
        if source.is_some() {
            ensure!(
                workflow
                    .as_object()
                    .unwrap()
                    .values()
                    .any(
                        |node| node["inputs"].as_object().is_some_and(|inputs| inputs
                            .values()
                            .any(|v| v.as_str().is_some_and(|s| s.trim() == "{{image}}")))
                    ),
                "The edit workflow needs {{{{image}}}} in an input, usually LoadImage's image field"
            );
        }
        let uploaded = upload(&http, url, r.seed, source).await?;
        let name = std::path::Path::new(workflow_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (fill(&workflow, r, uploaded.as_deref()), name)
    };
    ensure!(!cancel.load(Ordering::Relaxed), "Stopped");
    let started = Instant::now();
    let queued: Value = http
        .post(format!("{url}/prompt"))
        .json(&json!({"prompt": graph, "client_id": format!("spindle-{}", r.seed)}))
        .send()
        .await
        .with_context(|| format!("Couldn't reach ComfyUI at {url}"))?
        .json()
        .await?;
    if let Some(err) = queued.get("error") {
        let details = queued["node_errors"]
            .as_object()
            .map(|n| {
                n.values()
                    .flat_map(|e| e["errors"].as_array().cloned().unwrap_or_default())
                    .filter_map(|e| {
                        e["details"]
                            .as_str()
                            .or(e["message"].as_str())
                            .map(str::to_owned)
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default();
        bail!(
            "ComfyUI refused the workflow: {}{}",
            err["message"].as_str().unwrap_or("unknown error"),
            if details.is_empty() {
                String::new()
            } else {
                format!(" ({details})")
            }
        );
    }
    let id = queued["prompt_id"]
        .as_str()
        .context("ComfyUI didn't return a prompt id")?
        .to_owned();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = http
                .post(format!("{url}/queue"))
                .json(&json!({"delete": [id]}))
                .send()
                .await;
            let _ = http
                .post(format!("{url}/interrupt"))
                .json(&json!({"prompt_id": id}))
                .send()
                .await;
            bail!("Stopped");
        }
        ensure!(
            started.elapsed() < LIMIT,
            "ComfyUI took longer than {} minutes",
            LIMIT.as_secs() / 60
        );
        tokio::time::sleep(Duration::from_millis(800)).await;
        let history: Value = match http.get(format!("{url}/history/{id}")).send().await {
            Ok(r) => r.json().await.unwrap_or(Value::Null),
            Err(_) => continue,
        };
        let Some(entry) = history.get(&id) else {
            continue;
        };
        if entry["status"]["status_str"] == "error" {
            let message = entry["status"]["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| m[0] == "execution_error")
                .filter_map(|m| m[1]["exception_message"].as_str())
                .next()
                .unwrap_or("the workflow failed")
                .trim()
                .to_owned();
            bail!("ComfyUI: {message}");
        }
        let Some(image) = first_image(&entry["outputs"]) else {
            if entry["status"]["completed"] == true {
                bail!("The workflow finished without saving an image");
            }
            continue;
        };
        let mut response = http
            .get(format!("{url}/view"))
            .query(&[
                ("filename", image.0.as_str()),
                ("subfolder", image.1.as_str()),
                ("type", image.2.as_str()),
            ])
            .send()
            .await?
            .error_for_status()?;
        let png = png_image(read_response(&mut response, 32 * 1024 * 1024).await?)?;
        return Ok(Image {
            png,
            checkpoint,
            seconds: started.elapsed().as_secs(),
        });
    }
}

async fn stable_diffusion(
    url: &str,
    r: &Request,
    source: Option<Source>,
    cancel: Arc<AtomicBool>,
) -> Result<Image> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    let http = crate::local_servers::client(LIMIT)?;
    let url = crate::local_servers::address(url)?;
    let mut input = json!({
        "prompt": r.prompt, "negative_prompt": r.negative, "seed": r.seed,
        "width": r.width, "height": r.height, "steps": 25, "cfg_scale": 6.5
    });
    let command = if let Some(source) = source {
        let (width, height) = source.dimensions;
        input["width"] = json!((width / 8 * 8).max(8));
        input["height"] = json!((height / 8 * 8).max(8));
        input["init_images"] = json!([STANDARD.encode(source.bytes)]);
        input["denoising_strength"] = json!(0.6);
        "img2img"
    } else {
        "txt2img"
    };
    let started = Instant::now();
    ensure!(!cancel.load(Ordering::Relaxed), "Stopped");
    let request = async {
        let mut response = http
            .post(format!("{url}/sdapi/v1/{command}"))
            .json(&input)
            .send()
            .await
            .with_context(|| format!("Couldn't reach Stable Diffusion WebUI at {url}"))?
            .error_for_status()?;
        let body: Value =
            serde_json::from_slice(&read_response(&mut response, 45 * 1024 * 1024).await?)?;
        let encoded = body["images"][0]
            .as_str()
            .context("Stable Diffusion WebUI returned no image")?;
        let png = png_image(STANDARD.decode(encoded)?)?;
        Ok(Image {
            png,
            checkpoint: "Stable Diffusion WebUI".into(),
            seconds: started.elapsed().as_secs(),
        })
    };
    tokio::select! {
        result = request => result,
        _ = async {
            while !cancel.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        } => bail!("Stopped waiting for Stable Diffusion WebUI"),
    }
}

async fn read_response(response: &mut reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= limit,
            "Image server response is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn png_image(bytes: Vec<u8>) -> Result<Vec<u8>> {
    let format =
        image::guess_format(&bytes).context("The image server returned an unreadable file")?;
    if format == image::ImageFormat::Png {
        return Ok(bytes);
    }
    let image = image::load_from_memory_with_format(&bytes, format)?;
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png)?;
    Ok(png.into_inner())
}

async fn upload(
    http: &reqwest::Client,
    url: &str,
    seed: u64,
    source: Option<Source>,
) -> Result<Option<String>> {
    let Some(source) = source else {
        return Ok(None);
    };
    let ext = if source.format == image::ImageFormat::Jpeg {
        "jpg"
    } else {
        "png"
    };
    let name = format!("spindle-edit-{seed}.{ext}");
    let form = reqwest::multipart::Form::new()
        .text("type", "input")
        .text("overwrite", "false")
        .part(
            "image",
            reqwest::multipart::Part::bytes(source.bytes).file_name(name),
        );
    let data: Value = http
        .post(format!("{url}/upload/image"))
        .multipart(form)
        .send()
        .await
        .with_context(|| format!("Couldn't upload the source image to {url}"))?
        .error_for_status()?
        .json()
        .await?;
    let name = data["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context("ComfyUI didn't return the uploaded filename")?;
    let subfolder = data["subfolder"].as_str().unwrap_or_default();
    Ok(Some(if subfolder.is_empty() {
        name.to_owned()
    } else {
        format!("{subfolder}/{name}")
    }))
}

/// The first saved image in a history entry's outputs: file name, subfolder and type.
fn first_image(outputs: &Value) -> Option<(String, String, String)> {
    let mut nodes: Vec<(&String, &Value)> = outputs.as_object()?.iter().collect();
    // Prefer saved images over previews.
    nodes.sort_by_key(|(_, v)| v["images"][0]["type"] != "output");
    nodes.into_iter().find_map(|(_, v)| {
        let i = &v["images"][0];
        Some((
            i["filename"].as_str()?.to_owned(),
            i["subfolder"].as_str().unwrap_or("").to_owned(),
            i["type"].as_str().unwrap_or("output").to_owned(),
        ))
    })
}

/// A file name for an image from its prompt: "a red fox at dawn" → "red-fox-at-dawn.png".
pub fn file_name(prompt: &str) -> String {
    let words: Vec<String> = prompt
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| {
            !w.is_empty() && !matches!(w.to_lowercase().as_str(), "a" | "an" | "the" | "of")
        })
        .take(6)
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() {
        "image.png".into()
    } else {
        format!("{}.png", words.join("-"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_an_sdxl_image_checkpoint() {
        let names: Vec<String> = [
            "ace_step_1.5_turbo_aio.safetensors",
            "juggernautXL_versionXInpaint.safetensors",
            "OfficialStableDiffusion\\sd_xl_base_1.0.safetensors",
            "sd3_medium_incl_clips_t5xxlfp8.safetensors",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            pick(&names).unwrap(),
            "OfficialStableDiffusion\\sd_xl_base_1.0.safetensors"
        );
        assert_eq!(pick(&names[..2]), None);
    }

    #[test]
    fn fills_a_workflow() {
        let r = Request {
            prompt: "a fox".into(),
            negative: "blur".into(),
            width: 512,
            height: 768,
            seed: 7,
        };
        let w = json!({"1": {"class_type": "X", "inputs": {"text": "photo of {{prompt}}", "n": "{{negative}}", "seed": "{{seed}}", "w": "{{width}}"}}});
        let f = fill(&w, &r, None);
        assert_eq!(f["1"]["inputs"]["text"], "photo of a fox");
        assert_eq!(f["1"]["inputs"]["n"], "blur");
        assert_eq!(f["1"]["inputs"]["seed"], 7);
        assert_eq!(f["1"]["inputs"]["w"], 512);
    }

    #[test]
    fn names_images_from_prompts() {
        assert_eq!(file_name("A red fox at dawn!"), "red-fox-at-dawn.png");
        assert_eq!(file_name("???"), "image.png");
    }

    #[test]
    fn finds_saved_images() {
        let out = json!({"9": {"images": [{"filename": "spindle_0001_.png", "subfolder": "", "type": "output"}]}});
        assert_eq!(first_image(&out).unwrap().0, "spindle_0001_.png");
    }

    #[test]
    fn source_image_replaces_the_empty_latent_only_for_edit() {
        let r = Request::new("make it blue");
        let generate = builtin("sdxl", &r, None);
        let edit = builtin("sdxl", &r, Some("uploads/source.png"));
        assert_eq!(generate["5"]["class_type"], "EmptyLatentImage");
        assert_eq!(generate["3"]["inputs"]["denoise"], 1.0);
        assert_eq!(edit["10"]["inputs"]["image"], "uploads/source.png");
        assert_eq!(edit["5"]["class_type"], "VAEEncode");
        assert_eq!(edit["3"]["inputs"]["denoise"], 0.6);
        assert_eq!(
            fill(
                &json!({"image":"{{image}}"}),
                &r,
                Some("uploads/source.png")
            ),
            json!({"image":"uploads/source.png"})
        );
    }

    #[tokio::test]
    async fn stable_diffusion_routes_requests_by_presence_of_source() {
        use axum::{Json, Router, extract::Path, routing::post};
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(16, 24)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let bytes = png.into_inner();
        let encoded = STANDARD.encode(&bytes);
        let router = Router::new().route(
            "/sdapi/v1/{operation}",
            post(
                move |Path(operation): Path<String>, Json(body): Json<Value>| {
                    let encoded = encoded.clone();
                    async move {
                        match operation.as_str() {
                            "txt2img" => assert!(body.get("init_images").is_none()),
                            "img2img" => {
                                assert_eq!(body["init_images"][0], encoded);
                                assert_eq!(body["width"], 16);
                                assert_eq!(body["height"], 24);
                            }
                            _ => panic!("Unexpected Stable Diffusion operation"),
                        }
                        assert_eq!(body["prompt"], "Spindle test");
                        Json(json!({"images":[encoded]}))
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let settings = Settings {
            backend: Backend::StableDiffusion,
            url: format!("http://{}", listener.local_addr().unwrap()),
            ..Default::default()
        };
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        for source in [None, Some(Source::new(bytes.clone()).unwrap())] {
            let result = run(
                &settings,
                &Request::new("Spindle test"),
                source,
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
            assert_eq!(result.png, bytes);
        }
        server.abort();
    }
}
