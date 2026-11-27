//! Image generation and editing with ComfyUI.
//!
//! Spindle sends ComfyUI a workflow in its API format: a built-in text-to-image graph for
//! SDXL-style checkpoints (with a source image for edits), or the user's own API workflow. In the
//! user's workflow, text fields written as `{{prompt}}`, `{{negative}}`, `{{seed}}`,
//! `{{width}}` and `{{height}}` are filled in. Spindle then waits for the result and
//! downloads the images saved by the workflow.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
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
    pub aspect_ratio: String,
    pub transparent: bool,
    pub profiles: Vec<Workflow>,
    pub default_profile: String,
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
            aspect_ratio: prefs.image_aspect_ratio.clone(),
            transparent: prefs.image_transparent,
            profiles: prefs.comfy_profiles.clone(),
            default_profile: prefs.comfy_profile.clone(),
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

    pub async fn describe(&self) -> Result<String> {
        ensure!(
            self.ready(),
            "Set an image server in Settings › Models › Image generation"
        );
        let mut lines = vec![format!(
            "{} · default aspect {}",
            self.label(),
            if self.aspect_ratio.is_empty() {
                "1:1"
            } else {
                &self.aspect_ratio
            }
        )];
        if self.backend == Backend::StableDiffusion {
            let url = crate::local_servers::address(&self.url)?;
            let values: Value = client()?
                .get(format!("{url}/sdapi/v1/sd-models"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            lines.extend(
                values
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v["title"].as_str())
                    .take(60)
                    .map(|title| format!("Checkpoint: {title}")),
            );
        } else {
            let models = catalog(&self.url).await?;
            lines.push(format!(
                "Built-in transparent PNG: {}",
                if models.alpha() {
                    "available"
                } else {
                    "background removal model missing"
                }
            ));
            lines.extend(
                models
                    .checkpoints
                    .iter()
                    .take(50)
                    .map(|title| format!("Checkpoint: {title}")),
            );
            lines.extend(models.diffusion.iter().take(50).map(|name| {
                format!(
                    "Diffusion: {name}{}",
                    if Catalog::builtin_diffusion(name) {
                        " (built-in)"
                    } else {
                        " (requires workflow)"
                    }
                )
            }));
            lines.extend(self.profiles.iter().map(|p| {
                format!(
                    "Workflow {} ({}) · options: {}",
                    p.name,
                    if p.edit { "edit" } else { "generate" },
                    json!(p.parameters)
                )
            }));
        }
        Ok(lines.join("\n"))
    }
}

pub const ASPECT_RATIOS: [&str; 7] = ["1:1", "4:3", "3:4", "16:9", "9:16", "16:10", "10:16"];
const MAX_IMAGES: usize = 4;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Workflow {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub edit: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, Value>,
}

impl Workflow {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=48).contains(&self.name.len())
                && !self.name.eq_ignore_ascii_case("builtin")
                && !self.name.chars().any(char::is_control),
            "Give the workflow a name up to 48 characters"
        );
        ensure!(
            !self.path.trim().is_empty(),
            "Choose an API workflow JSON file"
        );
        ensure!(self.parameters.len() <= 20, "Use up to 20 workflow options");
        for (name, value) in &self.parameters {
            ensure!(
                !name.is_empty()
                    && name.len() <= 48
                    && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    && ![
                        "prompt",
                        "negative",
                        "seed",
                        "width",
                        "height",
                        "count",
                        "transparent",
                        "model",
                        "image"
                    ]
                    .contains(&name.as_str()),
                "Invalid workflow option name {name}"
            );
            ensure!(
                scalar(value),
                "Use text, numbers, or booleans as workflow defaults"
            );
        }
        Ok(())
    }
}

fn scalar(value: &Value) -> bool {
    value.is_boolean() || value.is_number() || value.as_str().is_some_and(|s| s.len() <= 2048)
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Options {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transparent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, Value>,
}

impl Options {
    pub fn dimensions(&self, default: &str) -> Result<(u32, u32)> {
        ensure!(
            self.aspect_ratio.is_none() || self.width.is_none() && self.height.is_none(),
            "Choose an aspect ratio or set both width and height"
        );
        match (self.width, self.height) {
            (Some(w), Some(h)) => {
                ensure!(
                    [w, h]
                        .into_iter()
                        .all(|n| (256..=2048).contains(&n) && n % 8 == 0),
                    "Width and height each need to be multiples of 8 from 256 to 2,048"
                );
                Ok((w, h))
            }
            (None, None) => Ok(match self.aspect_ratio.as_deref().unwrap_or(default) {
                "" | "1:1" => (1024, 1024),
                "4:3" => (1024, 768),
                "3:4" => (768, 1024),
                "16:9" => (1024, 576),
                "9:16" => (576, 1024),
                "16:10" => (1024, 640),
                "10:16" => (640, 1024),
                _ => bail!(
                    "Choose one of these aspect ratios: {}",
                    ASPECT_RATIOS.join(", ")
                ),
            }),
            _ => bail!("Set both width and height"),
        }
    }

    pub fn command(mut text: &str) -> Result<(Self, &str)> {
        let mut options = Self::default();
        while text.trim_start().starts_with("--") {
            let (flag, rest) = text
                .trim_start()
                .split_once(char::is_whitespace)
                .unwrap_or((text.trim_start(), ""));
            text = rest.trim_start();
            match flag {
                "--transparent" => options.transparent = Some(true),
                "--opaque" => options.transparent = Some(false),
                "--aspect" | "--size" | "--count" | "--model" | "--workflow" | "--option" => {
                    let (value, rest) = if let Some(quoted) = text.strip_prefix('"') {
                        quoted
                            .split_once('"')
                            .context("Close the quoted image option")?
                    } else {
                        text.split_once(char::is_whitespace).unwrap_or((text, ""))
                    };
                    ensure!(!value.is_empty(), "Give a value for {flag}");
                    text = rest.trim_start();
                    match flag {
                        "--aspect" => options.aspect_ratio = Some(value.into()),
                        "--count" => {
                            options.count = Some(value.parse().context("Enter an image count")?)
                        }
                        "--model" => options.model = Some(value.into()),
                        "--workflow" => options.workflow = Some(value.into()),
                        "--option" => {
                            let (key, value) =
                                value.split_once('=').context("Use --option NAME=VALUE")?;
                            options.parameters.insert(
                                key.into(),
                                serde_json::from_str(value).unwrap_or_else(|_| json!(value)),
                            );
                        }
                        _ => {
                            let (w, h) = value
                                .split_once(['x', 'X', '×'])
                                .context("Use --size WIDTHxHEIGHT")?;
                            options.width = Some(w.parse().context("Enter a width")?);
                            options.height = Some(h.parse().context("Enter a height")?);
                        }
                    }
                }
                "--" => break,
                _ => bail!("Unknown image option {flag}"),
            }
        }
        Ok((options, text))
    }
}

#[derive(Clone, Debug)]
pub struct Request {
    pub prompt: String,
    pub negative: String,
    pub width: u32,
    pub height: u32,
    pub seed: u64,
    pub count: u32,
    pub transparent: bool,
    pub model: Option<String>,
    pub workflow: Option<String>,
    pub parameters: BTreeMap<String, Value>,
    use_source_dimensions: bool,
}

impl Request {
    pub fn new(prompt: &str) -> Self {
        Self {
            prompt: prompt.trim().to_owned(),
            negative: "blurry, low quality, watermark, text, deformed".into(),
            width: 1024,
            height: 1024,
            seed: seed(),
            count: 1,
            transparent: false,
            model: None,
            workflow: None,
            parameters: BTreeMap::new(),
            use_source_dimensions: true,
        }
    }

    pub fn configured(prompt: &str, options: &Options, settings: &Settings) -> Result<Self> {
        let mut request = Self::new(prompt);
        (request.width, request.height) = options.dimensions(&settings.aspect_ratio)?;
        request.use_source_dimensions = options.aspect_ratio.is_none() && options.width.is_none();
        request.count = options.count.unwrap_or(1);
        ensure!(
            (1..=MAX_IMAGES as u32).contains(&request.count),
            "Request one to {MAX_IMAGES} images"
        );
        request.transparent = options.transparent.unwrap_or(settings.transparent);
        request.model = options
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        ensure!(
            request
                .model
                .as_ref()
                .is_none_or(|s| s.len() <= 256 && !s.chars().any(char::is_control)),
            "Invalid image model name"
        );
        request.workflow = options
            .workflow
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        ensure!(
            options.parameters.len() <= 20 && options.parameters.values().all(scalar),
            "Use up to 20 text, number, or boolean options"
        );
        request.parameters.clone_from(&options.parameters);
        Ok(request)
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
    pub images: Vec<Vec<u8>>,
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

#[derive(Default)]
pub struct Catalog {
    pub checkpoints: Vec<String>,
    pub diffusion: Vec<String>,
    encoders: Vec<String>,
    vaes: Vec<String>,
    background: Vec<String>,
    reference: bool,
}

async fn object(http: &reqwest::Client, url: &str, node: &str) -> Result<Value> {
    let value: Value = http
        .get(format!("{url}/object_info/{node}"))
        .timeout(Duration::from_secs(5))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(value.get(node).cloned().unwrap_or_default())
}

fn choices(node: &Value, field: &str) -> Vec<String> {
    let specs = &node["input"]["required"][field];
    specs[0]
        .as_array()
        .or_else(|| specs[1]["options"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

pub async fn catalog(url: &str) -> Result<Catalog> {
    let url = url.trim().trim_end_matches('/');
    ensure!(!url.is_empty(), "Set the ComfyUI address in Settings first");
    let http = client()?;
    let (checkpoints, diffusion, clip, vae, background, reference) = tokio::join!(
        object(&http, url, "CheckpointLoaderSimple"),
        object(&http, url, "UNETLoader"),
        object(&http, url, "CLIPLoader"),
        object(&http, url, "VAELoader"),
        object(&http, url, "LoadBackgroundRemovalModel"),
        object(&http, url, "ReferenceLatent"),
    );
    ensure!(
        checkpoints.is_ok() || diffusion.is_ok(),
        "Couldn't connect to ComfyUI at {url}"
    );
    Ok(Catalog {
        checkpoints: choices(&checkpoints.unwrap_or_default(), "ckpt_name"),
        diffusion: choices(&diffusion.unwrap_or_default(), "unet_name"),
        encoders: choices(&clip.unwrap_or_default(), "clip_name"),
        vaes: choices(&vae.unwrap_or_default(), "vae_name"),
        background: choices(&background.unwrap_or_default(), "bg_removal_name"),
        reference: reference.is_ok_and(|node| !node.is_null()),
    })
}

impl Catalog {
    pub fn alpha(&self) -> bool {
        !self.background.is_empty()
    }

    pub fn builtin_diffusion(name: &str) -> bool {
        let name = normal_model(name);
        name.contains("flux3") && name.contains("klein")
            || name.contains("flux2") && name.contains("klein")
            || name.contains("flux1") && (name.contains("schnell") || name.contains("dev"))
    }

    fn model(&self, wanted: &str) -> Result<String> {
        if wanted.is_empty() {
            return pick(&self.checkpoints)
                .or_else(|| {
                    self.diffusion
                        .iter()
                        .find(|name| Self::builtin_diffusion(name))
                        .cloned()
                })
                .context("No compatible built-in image model found. Register a ComfyUI workflow");
        }
        self.checkpoints
            .iter()
            .chain(&self.diffusion)
            .find(|name| name.eq_ignore_ascii_case(wanted))
            .or_else(|| {
                let family = normal_model(wanted);
                matches!(family.as_str(), "flux1" | "flux2" | "flux3")
                    .then(|| {
                        self.diffusion.iter().find(|name| {
                            normal_model(name).contains(&family) && Self::builtin_diffusion(name)
                        })
                    })
                    .flatten()
            })
            .cloned()
            .with_context(|| format!("ComfyUI does not list the image model {wanted}"))
    }
}

fn normal_model(name: &str) -> String {
    name.to_ascii_lowercase().replace(['.', '-', '_', ' '], "")
}

/// The checkpoint the built-in workflow uses when none is chosen: an SDXL image model.
pub fn pick(names: &[String]) -> Option<String> {
    let lower = |n: &String| n.to_lowercase();
    let image = |n: &&String| {
        let l = lower(n);
        ![
            "inpaint", "audio", "sd3", "ace_step", "refiner", "video", "svd", "flux",
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
        "5": {"class_type": "EmptyLatentImage", "inputs": {"width": r.width, "height": r.height, "batch_size": r.count}},
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
            json!({"class_type": "VAEEncode", "inputs": {"pixels": ["11", 0], "vae": ["4", 2]}});
        graph["11"] = json!({"class_type":"ImageScale","inputs":{
            "image":["10",0],"upscale_method":"lanczos","width":r.width,"height":r.height,"crop":"disabled"
        }});
        if r.count > 1 {
            graph["12"] = json!({"class_type":"RepeatLatentBatch","inputs":{"samples":["5",0],"amount":r.count}});
            graph["3"]["inputs"]["latent_image"] = json!(["12", 0]);
        }
        graph["3"]["inputs"]["denoise"] = json!(0.6);
    }
    graph
}

fn flux_graph(name: &str, models: &Catalog, r: &Request, source: Option<&str>) -> Result<Value> {
    let family = normal_model(name);
    ensure!(
        Catalog::builtin_diffusion(name),
        "Register a ComfyUI workflow for the diffusion model {name}"
    );
    let new = family.contains("flux3");
    let klein = family.contains("klein");
    let full = family.contains("base");
    let required = |list: &[String], needle: &str| -> Result<String> {
        list.iter()
            .find(|n| normal_model(n).contains(needle))
            .cloned()
            .with_context(|| format!("ComfyUI is missing {needle} for {name}"))
    };
    let (clip, vae, steps, cfg, latent) = if new || klein {
        let encoder = if new {
            "qwen3vl4b"
        } else if family.contains("9b") {
            "qwen38b"
        } else {
            "qwen34b"
        };
        let clip = json!({"class_type":"CLIPLoader","inputs":{
            "clip_name":required(&models.encoders,encoder)?,
            "type":if new {"flux3"} else {"flux2"},"device":"default"
        }});
        let steps = if full {
            50
        } else if new {
            16
        } else {
            4
        };
        (
            clip,
            required(&models.vaes, "flux2vae")?,
            steps,
            if full { 3.5 } else { 1.0 },
            "EmptyFlux2LatentImage",
        )
    } else {
        let clip = json!({"class_type":"DualCLIPLoader","inputs":{
            "clip_name1":required(&models.encoders,"clipl")?,
            "clip_name2":required(&models.encoders,"t5xxl")?,
            "type":"flux","device":"default"
        }});
        let vae = models
            .vaes
            .iter()
            .find(|n| n.rsplit(['/', '\\']).next() == Some("ae.safetensors"))
            .context("ComfyUI is missing ae.safetensors for FLUX.1")?;
        (
            clip,
            vae.clone(),
            if family.contains("schnell") { 4 } else { 20 },
            1.0,
            "EmptySD3LatentImage",
        )
    };
    let align = if klein { 16 } else { 8 };
    let mut graph = json!({
        "1":{"class_type":"UNETLoader","inputs":{"unet_name":name,"weight_dtype":"default"}},
        "2":clip,
        "3":{"class_type":"VAELoader","inputs":{"vae_name":vae}},
        "4":{"class_type":"CLIPTextEncode","inputs":{"clip":["2",0],"text":r.prompt}},
        "5":{"class_type":"ConditioningZeroOut","inputs":{"conditioning":["4",0]}},
        "6":{"class_type":latent,"inputs":{"width":r.width.next_multiple_of(align),"height":r.height.next_multiple_of(align),"batch_size":r.count}},
        "7":{"class_type":"KSampler","inputs":{"model":["1",0],"positive":["4",0],"negative":["5",0],
           "latent_image":["6",0],"seed":r.seed,"steps":steps,"cfg":cfg,"sampler_name":"euler","scheduler":"simple","denoise":1.0}},
        "8":{"class_type":"VAEDecode","inputs":{"samples":["7",0],"vae":["3",0]}},
        "9":{"class_type":"SaveImage","inputs":{"filename_prefix":"spindle","images":["8",0]}}
    });
    if !klein && !family.contains("schnell") {
        graph["10"] =
            json!({"class_type":"FluxGuidance","inputs":{"conditioning":["4",0],"guidance":3.5}});
        graph["7"]["inputs"]["positive"] = json!(["10", 0]);
    }
    if let Some(source) = source {
        ensure!(
            klein && models.reference,
            "Register a reference-image edit workflow for {name}"
        );
        graph["11"] = json!({"class_type":"LoadImage","inputs":{"image":source}});
        graph["12"] = json!({"class_type":"VAEEncode","inputs":{"pixels":["11",0],"vae":["3",0]}});
        for (id, input, conditioning) in [("13", "positive", "4"), ("14", "negative", "5")] {
            graph[id] = json!({"class_type":"ReferenceLatent","inputs":{
                "conditioning":[conditioning,0],"latent":["12",0]
            }});
            graph["7"]["inputs"][input] = json!([id, 0]);
        }
    }
    Ok(graph)
}

fn transparent_output(graph: &mut Value, models: &Catalog) -> Result<()> {
    let model = models.background.iter().find(|s| s.to_ascii_lowercase().contains("birefnet"))
        .or_else(|| models.background.first())
        .context("ComfyUI needs a background-removal model to make a transparent PNG. You can also select an alpha-output workflow")?;
    let images = graph["9"]["inputs"]["images"].clone();
    graph["90"] =
        json!({"class_type":"LoadBackgroundRemovalModel","inputs":{"bg_removal_name":model}});
    graph["91"] = json!({"class_type":"RemoveBackground","inputs":{"image":images,"bg_removal_model":["90",0]}});
    graph["92"] = json!({"class_type":"InvertMask","inputs":{"mask":["91",0]}});
    graph["93"] =
        json!({"class_type":"JoinImageWithAlpha","inputs":{"image":images,"alpha":["92",0]}});
    graph["9"]["inputs"]["images"] = json!(["93", 0]);
    Ok(())
}

/// Fill a user's workflow: `{{prompt}}` and the others become the request's values. A field
/// that is exactly `{{seed}}`, `{{width}}` or `{{height}}` becomes a number.
fn fill(workflow: &Value, r: &Request, source: Option<&str>) -> Value {
    match workflow {
        Value::String(s) => match s.trim() {
            "{{seed}}" => json!(r.seed),
            "{{width}}" => json!(r.width),
            "{{height}}" => json!(r.height),
            "{{count}}" => json!(r.count),
            "{{transparent}}" => json!(r.transparent),
            "{{model}}" => r
                .model
                .as_ref()
                .map_or_else(|| workflow.clone(), |name| json!(name)),
            "{{image}}" => source.map_or_else(|| workflow.clone(), |name| json!(name)),
            key if key.starts_with("{{")
                && key.ends_with("}}")
                && r.parameters.contains_key(&key[2..key.len() - 2]) =>
            {
                r.parameters[&key[2..key.len() - 2]].clone()
            }
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

fn workflow<'a>(
    settings: &'a Settings,
    r: &Request,
    edit: bool,
) -> Result<Option<(&'a str, Option<&'a Workflow>)>> {
    let wanted = r.workflow.as_deref().unwrap_or(&settings.default_profile);
    if wanted.eq_ignore_ascii_case("builtin") {
        ensure!(
            r.parameters.is_empty(),
            "Choose a registered workflow to set custom node options"
        );
        return Ok(None);
    }
    if !wanted.is_empty() {
        let profile = settings
            .profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(wanted))
            .with_context(|| format!("No registered image workflow named {wanted}"))?;
        if profile.edit == edit {
            profile.validate()?;
            return Ok(Some((&profile.path, Some(profile))));
        }
        ensure!(
            r.workflow.is_none(),
            "Choose a workflow for {}",
            if edit {
                "image editing"
            } else {
                "image generation"
            }
        );
    }
    ensure!(
        r.parameters.is_empty(),
        "Choose a registered workflow to set custom node options"
    );
    let path = if edit {
        &settings.edit_workflow
    } else {
        &settings.workflow
    };
    Ok((!path.is_empty()).then_some((path.as_str(), None)))
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
    let mut request = r.clone();
    if r.use_source_dimensions
        && let Some(source) = &source
    {
        (request.width, request.height) = (
            (source.dimensions.0 / 8 * 8).max(8),
            (source.dimensions.1 / 8 * 8).max(8),
        );
    }
    let r = &request;
    if settings.backend == Backend::StableDiffusion {
        return stable_diffusion(&settings.url, r, source, cancel).await;
    }
    let url = &settings.url;
    let http = client()?;
    let selected = workflow(settings, r, source.is_some())?;
    let (graph, checkpoint) = if let Some((workflow_path, profile)) = selected {
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
        let mut request = r.clone();
        if let Some(profile) = profile {
            let mut options = profile.parameters.clone();
            for (key, value) in &r.parameters {
                let original = options.get_mut(key).with_context(|| {
                    format!("Unknown option {key} for workflow {}", profile.name)
                })?;
                ensure!(
                    original.is_string() == value.is_string()
                        && original.is_boolean() == value.is_boolean(),
                    "Use the configured value type for workflow option {key}"
                );
                *original = value.clone();
            }
            request.parameters = options;
        }
        if text.contains("{{model}}") {
            let name = r.model.as_deref().unwrap_or(&settings.checkpoint);
            request.model = Some(catalog(url).await?.model(name)?);
        } else {
            ensure!(
                r.model.is_none(),
                "Add {{{{model}}}} to the workflow input to switch its model"
            );
        }
        let uploaded = upload(&http, url, r.seed, source).await?;
        let name = std::path::Path::new(workflow_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (
            fill(&workflow, &request, uploaded.as_deref()),
            profile.map_or(name, |p| p.name.clone()),
        )
    } else {
        let models = catalog(url).await?;
        let checkpoint = models.model(r.model.as_deref().unwrap_or(&settings.checkpoint))?;
        let uploaded = upload(&http, url, r.seed, source).await?;
        let mut graph = if models.diffusion.contains(&checkpoint) {
            flux_graph(&checkpoint, &models, r, uploaded.as_deref())?
        } else {
            builtin(&checkpoint, r, uploaded.as_deref())
        };
        if r.transparent {
            transparent_output(&mut graph, &models)?;
        }
        (graph, checkpoint)
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
        if entry["status"]["completed"] != true {
            continue;
        }
        let files = output_images(&entry["outputs"]);
        ensure!(
            !files.is_empty(),
            "The workflow finished without saving an image"
        );
        let mut images = Vec::new();
        for image in files.into_iter().take(MAX_IMAGES) {
            ensure!(!cancel.load(Ordering::Relaxed), "Stopped");
            let mut response = http
                .get(format!("{url}/view"))
                .query(&[
                    ("filename", &image.0),
                    ("subfolder", &image.1),
                    ("type", &image.2),
                ])
                .send()
                .await?
                .error_for_status()?;
            images.push(png_image(
                read_response(&mut response, 32 * 1024 * 1024).await?,
                r,
                selected.is_none(),
            )?);
        }
        return Ok(Image {
            images,
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
    ensure!(
        !r.transparent && r.workflow.is_none() && r.parameters.is_empty(),
        "Choose ComfyUI for transparent PNGs or node workflows"
    );
    let mut input = json!({
        "prompt": r.prompt, "negative_prompt": r.negative, "seed": r.seed,
        "width": r.width, "height": r.height, "steps": 25, "cfg_scale": 6.5, "batch_size": r.count
    });
    if let Some(name) = &r.model {
        let models: Value = http
            .get(format!("{url}/sdapi/v1/sd-models"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let model = models
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["title"] == *name || m["model_name"] == *name)
            .and_then(|m| m["title"].as_str())
            .context("Stable Diffusion WebUI does not list that checkpoint")?;
        input["override_settings"] = json!({"sd_model_checkpoint":model});
    }
    let command = if let Some(source) = source {
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
            serde_json::from_slice(&read_response(&mut response, 90 * 1024 * 1024).await?)?;
        let first = body["info"]
            .as_str()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|info| info["index_of_first_image"].as_u64())
            .unwrap_or(0) as usize;
        let images = body["images"]
            .as_array()
            .into_iter()
            .flatten()
            .skip(first)
            .take(MAX_IMAGES)
            .filter_map(Value::as_str)
            .map(|encoded| png_image(STANDARD.decode(encoded)?, r, true))
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            !images.is_empty(),
            "Stable Diffusion WebUI returned no image"
        );
        Ok(Image {
            images,
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

fn png_image(bytes: Vec<u8>, r: &Request, match_dimensions: bool) -> Result<Vec<u8>> {
    let format =
        image::guess_format(&bytes).context("The image server returned an unreadable file")?;
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut image = reader.decode()?;
    if r.transparent {
        ensure!(
            image.color().has_alpha() && image.to_rgba8().pixels().any(|p| p[3] < 255),
            "The image workflow returned an opaque image. Choose a workflow that produces an alpha channel"
        );
    }
    if format == image::ImageFormat::Png
        && (!match_dimensions || image.width() == r.width && image.height() == r.height)
    {
        return Ok(bytes);
    }
    if match_dimensions && (image.width() != r.width || image.height() != r.height) {
        image = image.resize_exact(r.width, r.height, image::imageops::FilterType::Lanczos3);
    }
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

fn output_images(outputs: &Value) -> Vec<(String, String, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut images: Vec<_> = outputs
        .as_object()
        .into_iter()
        .flat_map(|nodes| nodes.values())
        .flat_map(|node| node["images"].as_array().into_iter().flatten())
        .filter_map(|i| {
            Some((
                i["filename"].as_str()?.to_owned(),
                i["subfolder"].as_str().unwrap_or("").to_owned(),
                i["type"].as_str().unwrap_or("output").to_owned(),
            ))
        })
        .filter(|image| image.2 != "input" && seen.insert(image.clone()))
        .collect();
    if images.iter().any(|image| image.2 == "output") {
        images.retain(|image| image.2 == "output");
    }
    images
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
            parameters: BTreeMap::from([("steps".into(), json!(12))]),
            ..Request::new("")
        };
        let w = json!({"1": {"class_type": "X", "inputs": {"text": "photo of {{prompt}}", "n": "{{negative}}", "seed": "{{seed}}", "w": "{{width}}", "steps": "{{steps}}"}}});
        let f = fill(&w, &r, None);
        assert_eq!(f["1"]["inputs"]["text"], "photo of a fox");
        assert_eq!(f["1"]["inputs"]["n"], "blur");
        assert_eq!(f["1"]["inputs"]["seed"], 7);
        assert_eq!(f["1"]["inputs"]["w"], 512);
        assert_eq!(f["1"]["inputs"]["steps"], 12);
    }

    #[test]
    fn names_images_from_prompts() {
        assert_eq!(file_name("A red fox at dawn!"), "red-fox-at-dawn.png");
        assert_eq!(file_name("???"), "image.png");
    }

    #[test]
    fn finds_saved_images() {
        let out = json!({
            "4": {"images": [{"filename":"preview.png","type":"temp"}]},
            "9": {"images": [{"filename":"one.png","type":"output"}, {"filename":"two.png","type":"output"}]}
        });
        let actual = output_images(&out)
            .into_iter()
            .map(|image| image.0)
            .collect::<Vec<_>>();
        assert_eq!(actual, ["one.png", "two.png"]);
    }

    #[test]
    fn options_bind_exact_dimensions_and_a_transparency_graph_for_flux3() {
        let (opts, prompt) =
            Options::command("--aspect 16:10 --transparent --count 2 --model flux3 a turbine")
                .unwrap();
        let request = Request::configured(prompt, &opts, &Settings::default()).unwrap();
        assert_eq!(
            (request.width, request.height, request.count),
            (1024, 640, 2)
        );
        assert_eq!(request.prompt, "a turbine");
        let models = Catalog {
            diffusion: vec!["flux3_image_klein_sd_bf16.safetensors".into()],
            encoders: vec!["qwen3vl_4b_bf16.safetensors".into()],
            vaes: vec!["flux2-vae.safetensors".into()],
            background: vec!["birefnet.safetensors".into()],
            reference: true,
            ..Default::default()
        };
        let name = models.model(request.model.as_deref().unwrap()).unwrap();
        let mut graph = flux_graph(&name, &models, &request, Some("source.png")).unwrap();
        transparent_output(&mut graph, &models).unwrap();
        assert_eq!(graph["2"]["inputs"]["type"], "flux3");
        assert_eq!(graph["6"]["inputs"]["batch_size"], 2);
        assert_eq!(graph["7"]["inputs"]["positive"], json!(["13", 0]));
        assert_eq!(graph["9"]["inputs"]["images"], json!(["93", 0]));
        assert_eq!(graph["93"]["class_type"], "JoinImageWithAlpha");
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
                &Request {
                    width: 16,
                    height: 24,
                    ..Request::new("Spindle test")
                },
                source,
                Arc::new(AtomicBool::new(false)),
            )
            .await
            .unwrap();
            assert_eq!(result.images, [bytes.clone()]);
        }
        server.abort();
    }
}
