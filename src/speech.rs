//! Speech: text to speech and speech to text over the OpenAI-compatible audio API.
//!
//! Ollama serves both (`/v1/audio/speech` and `/v1/audio/transcriptions`) for models that
//! report the "speech" and "audio" capabilities, such as lfm3-omni; so do OpenAI and many
//! local servers (Speaches, Kokoro-FastAPI, LocalAI). By default Spindle uses the Ollama
//! server it already talks to and picks models by their capabilities.

use anyhow::{Context, Result, bail, ensure};
use std::time::Duration;

/// Whether a text-to-speech and a speech-to-text model are available, kept up to date by the
/// engine so the chat prompt only offers tools that can work.
static AVAILABLE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_available(speak: bool, hear: bool) {
    AVAILABLE.store(
        speak as u8 | (hear as u8) << 1,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// (can speak, can transcribe).
pub fn available() -> (bool, bool) {
    let v = AVAILABLE.load(std::sync::atomic::Ordering::Relaxed);
    (v & 1 != 0, v & 2 != 0)
}

/// Ollama's limit on text per speech request.
const CHUNK: usize = 3500;

/// Fish Audio's API.
pub const FISH: &str = "https://api.fish.audio";
pub const FISH_TTS: &str = "s2.1-pro";
pub const FISH_STT: &str = "transcribe-1-pro";

/// A Fish Audio account, or a self-hosted Fish Speech server (tools/api_server.py), which
/// speaks the same /v1/tts but has no transcription, keeps its model fixed at startup, and
/// stores voices as named references.
#[derive(Clone, Debug, Default)]
pub struct Fish {
    pub url: String,
    pub key: String,
}

impl Fish {
    pub fn local(&self) -> bool {
        self.url.trim_end_matches('/') != FISH
    }
    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        auth(req, &self.key)
    }
}

/// The command that starts a local Fish Speech server Spindle can use.
pub const FISH_LOCAL_COMMAND: &str = "python tools/api_server.py --llama-checkpoint-path checkpoints/s2-pro --decoder-checkpoint-path checkpoints/s2-pro/codec.pth --listen 127.0.0.1:8080";

#[derive(Clone, Debug, Default)]
pub struct Settings {
    /// Server base, such as http://127.0.0.1:11434 (Spindle adds /v1/audio/…).
    pub url: String,
    pub key: String,
    pub tts_model: String,
    pub voice: String,
    pub stt_model: String,
    /// Speak with Fish Audio instead of the server above.
    pub tts_fish: Option<Fish>,
    /// Transcribe with Fish Audio instead of the server above.
    pub stt_fish: Option<Fish>,
}

impl Settings {
    /// From preferences, filling in the Ollama server and a model with each capability.
    pub fn from(
        prefs: &crate::state::Preferences,
        speech_models: &[String],
        audio_models: &[String],
    ) -> Self {
        let url = if prefs.speech_url.trim().is_empty() {
            crate::ollama::endpoint().to_owned()
        } else {
            prefs.speech_url.trim().trim_end_matches('/').to_owned()
        };
        let pick = |chosen: &str, list: &[String], prefer: &str| {
            if !chosen.trim().is_empty() {
                return chosen.trim().to_owned();
            }
            list.iter()
                .find(|m| m.contains(prefer))
                .or(list.first())
                .map(|m| tag(m).to_owned())
                .unwrap_or_default()
        };
        // Fish Audio's service needs a key; a self-hosted server only needs its address.
        let fish =
            (!prefs.fish_key.trim().is_empty() || !prefs.fish_url.trim().is_empty()).then(|| {
                Fish {
                    url: if prefs.fish_url.trim().is_empty() {
                        FISH.into()
                    } else {
                        prefs.fish_url.trim().trim_end_matches('/').to_owned()
                    },
                    key: prefs.fish_key.trim().to_owned(),
                }
            });
        let or = |v: &str, default: &str| {
            if v.trim().is_empty() {
                default.to_owned()
            } else {
                v.trim().to_owned()
            }
        };
        if prefs.tts_provider == "fish" || prefs.stt_provider == "fish" {
            let mut s = Self::from(
                &crate::state::Preferences {
                    tts_provider: String::new(),
                    stt_provider: String::new(),
                    ..prefs.clone()
                },
                speech_models,
                audio_models,
            );
            // With no key, Fish Audio can't be used: leave that side with no model.
            if prefs.tts_provider == "fish" {
                s.tts_fish = fish.clone();
                s.tts_model = if fish.is_some() {
                    or(&prefs.fish_tts_model, FISH_TTS)
                } else {
                    String::new()
                };
                s.voice = prefs.fish_voice.trim().to_owned();
            }
            // A self-hosted Fish Speech server can't transcribe: Ollama's side does it.
            if prefs.stt_provider == "fish" && fish.as_ref().is_some_and(|f| !f.local()) {
                s.stt_fish = fish;
                s.stt_model = if s.stt_fish.is_some() {
                    or(&prefs.fish_stt_model, FISH_STT)
                } else {
                    String::new()
                };
            }
            return s;
        }
        Self {
            url,
            key: prefs.speech_key.clone(),
            tts_model: pick(&prefs.tts_model, speech_models, "lfm"),
            voice: if prefs.tts_voice.trim().is_empty() {
                "default".into()
            } else {
                prefs.tts_voice.trim().to_owned()
            },
            stt_model: pick(&prefs.stt_model, audio_models, "lfm"),
            tts_fish: None,
            stt_fish: None,
        }
    }
}

/// "ollama:lfm3-omni:latest" → "lfm3-omni:latest".
fn tag(model: &str) -> &str {
    crate::providers::route(model).1
}

fn client(timeout: u64) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(timeout))
        .build()?)
}

fn auth(req: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    if key.trim().is_empty() {
        req
    } else {
        req.bearer_auth(key.trim())
    }
}

/// Turn a reply into something worth hearing: no code, links shown as their text, no
/// Markdown marks, no "Saved file" lines.
pub fn speakable(markdown: &str) -> String {
    let mut out = String::new();
    let mut fenced = false;
    for line in markdown.lines() {
        let t = line.trim();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || t.starts_with("Saved file:") || t.starts_with("<!--") || t.starts_with('|') {
            continue;
        }
        let mut l = t
            .trim_start_matches('#')
            .trim_start_matches(['-', '*', '>', '+'])
            .trim()
            .to_owned();
        // [text](url) → text
        while let Some(open) = l.find('[') {
            let Some(mid) = l[open..].find("](").map(|i| open + i) else {
                break;
            };
            let Some(close) = l[mid..].find(')').map(|i| mid + i) else {
                break;
            };
            l = format!("{}{}{}", &l[..open], &l[open + 1..mid], &l[close + 1..]);
        }
        let l = l.replace(['*', '`', '_'], "");
        if !l.is_empty() {
            out.push_str(&l);
            out.push('\n');
        }
    }
    out.trim().to_owned()
}

/// Split text at paragraph, then sentence breaks, into pieces the server accepts.
fn chunks(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    for sentence in text.split_inclusive(['.', '!', '?', '\n']) {
        if cur.len() + sentence.len() > CHUNK && !cur.trim().is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        if sentence.len() > CHUNK {
            // One enormous "sentence": cut at a character boundary.
            let mut s = sentence;
            while s.len() > CHUNK {
                let mut at = CHUNK;
                while !s.is_char_boundary(at) {
                    at -= 1;
                }
                out.push(s[..at].to_owned());
                s = &s[at..];
            }
            cur.push_str(s);
        } else {
            cur.push_str(sentence);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

/// Read text aloud: a WAV file.
pub async fn speak(s: &Settings, text: &str) -> Result<Vec<u8>> {
    ensure!(
        !s.tts_model.is_empty(),
        "No text-to-speech model. Pull one into Ollama (for example lfm3-omni) or choose one in Settings › Models › Audio."
    );
    let text = text.trim();
    ensure!(!text.is_empty(), "Nothing to read aloud");
    let http = client(300)?;
    let mut parts = vec![];
    if let Some(fish) = &s.tts_fish {
        for piece in chunks(text) {
            let mut body = serde_json::json!({
                "text": piece, "format": "wav", "normalize": true, "latency": "normal"
            });
            if !s.voice.is_empty() {
                body["reference_id"] = serde_json::json!(s.voice);
            }
            let response = fish
                .auth(http.post(format!("{}/v1/tts", fish.url)))
                .header("model", &s.tts_model)
                .json(&body)
                .send()
                .await
                .with_context(|| format!("Couldn't reach Fish Audio at {}", fish.url))?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                bail!(
                    "Fish Audio couldn't speak it ({status}): {}",
                    message(&body)
                );
            }
            parts.push(response.bytes().await?.to_vec());
        }
        return join_wav(&parts);
    }
    for piece in chunks(text) {
        let response = auth(http.post(format!("{}/v1/audio/speech", s.url)), &s.key)
            .json(&serde_json::json!({
                "model": s.tts_model, "input": piece, "voice": s.voice, "response_format": "wav"
            }))
            .send()
            .await
            .with_context(|| format!("Couldn't reach the speech server at {}", s.url))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Text to speech failed ({status}): {}", message(&body));
        }
        parts.push(response.bytes().await?.to_vec());
    }
    join_wav(&parts)
}

/// Write down what was said in a recording (WAV, MP3 and the like).
pub async fn transcribe(s: &Settings, audio: Vec<u8>, name: &str) -> Result<String> {
    ensure!(
        !s.stt_model.is_empty(),
        "No speech-to-text model. Pull one into Ollama (for example lfm3-omni) or choose one in Settings › Models › Audio."
    );
    ensure!(!audio.is_empty(), "The recording is empty");
    let mime = match name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        "ogg" | "oga" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "webm" => "audio/webm",
        _ => "audio/wav",
    };
    if let Some(fish) = &s.stt_fish {
        let form = reqwest::multipart::Form::new()
            .text("ignore_timestamps", "true")
            .part(
                "audio",
                reqwest::multipart::Part::bytes(audio)
                    .file_name(name.to_owned())
                    .mime_str(mime)?,
            );
        let response = fish
            .auth(client(1800)?.post(format!("{}/v1/asr", fish.url)))
            .header("model", &s.stt_model)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("Couldn't reach Fish Audio at {}", fish.url))?;
        let status = response.status();
        let body = response.text().await?;
        ensure!(
            status.is_success(),
            "Fish Audio couldn't transcribe it ({status}): {}",
            message(&body)
        );
        let v: serde_json::Value = serde_json::from_str(&body)?;
        return Ok(v["text"].as_str().unwrap_or("").trim().to_owned());
    }
    let form = reqwest::multipart::Form::new()
        .text("model", s.stt_model.clone())
        .part(
            "file",
            reqwest::multipart::Part::bytes(audio)
                .file_name(name.to_owned())
                .mime_str(mime)?,
        );
    let response = auth(
        client(600)?.post(format!("{}/v1/audio/transcriptions", s.url)),
        &s.key,
    )
    .multipart(form)
    .send()
    .await
    .with_context(|| format!("Couldn't reach the speech server at {}", s.url))?;
    let status = response.status();
    let body = response.text().await?;
    ensure!(
        status.is_success(),
        "Speech to text failed ({status}): {}",
        message(&body)
    );
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    Ok(v["text"].as_str().unwrap_or(&body).trim().to_owned())
}

/// The voices a model offers, from Ollama's /api/show.
pub async fn voices(url: &str, model: &str) -> Result<Vec<String>> {
    let v: serde_json::Value = client(15)?
        .post(format!("{}/api/show", url.trim_end_matches('/')))
        .json(&serde_json::json!({"model": model}))
        .send()
        .await?
        .json()
        .await?;
    Ok(v["voices"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

/// A Fish Audio voice: its id (sent as `reference_id`) and name.
#[derive(Clone, Debug, PartialEq)]
pub struct FishVoice {
    pub id: String,
    pub title: String,
    pub mine: bool,
}

/// Your own Fish Audio voices, then popular ones (optionally matching `search`); on a
/// self-hosted server, its saved references.
pub async fn fish_voices(fish: &Fish, search: &str) -> Result<Vec<FishVoice>> {
    let http = client(20)?;
    if fish.local() {
        let response = fish
            .auth(http.get(format!("{}/v1/references/list", fish.url)))
            // Without this the server answers in MessagePack.
            .header("Accept", "application/json")
            .send()
            .await
            .with_context(|| format!("Couldn't reach the Fish Speech server at {}", fish.url))?;
        let status = response.status();
        let body = response.text().await?;
        ensure!(
            status.is_success(),
            "The Fish Speech server didn't list voices ({status}): {}",
            message(&body)
        );
        let v: serde_json::Value = serde_json::from_str(&body)?;
        let search = search.trim().to_lowercase();
        return Ok(v["reference_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str())
            .filter(|id| search.is_empty() || id.to_lowercase().contains(&search))
            .map(|id| FishVoice {
                id: id.into(),
                title: id.into(),
                mine: true,
            })
            .collect());
    }
    let mut out = vec![];
    for mine in [true, false] {
        let mut query = vec![
            ("page_size", "40".to_owned()),
            ("self", mine.to_string()),
            ("sort_by", "score".into()),
        ];
        if !search.trim().is_empty() {
            query.push(("title", search.trim().to_owned()));
        }
        let response = fish
            .auth(http.get(format!("{}/model", fish.url)))
            .query(&query)
            .send()
            .await
            .with_context(|| format!("Couldn't reach Fish Audio at {}", fish.url))?;
        let status = response.status();
        let body = response.text().await?;
        ensure!(
            status.is_success(),
            "Fish Audio didn't list voices ({status}): {}",
            message(&body)
        );
        let v: serde_json::Value = serde_json::from_str(&body)?;
        for item in v["items"].as_array().into_iter().flatten() {
            let (Some(id), Some(title)) = (item["_id"].as_str(), item["title"].as_str()) else {
                continue;
            };
            if !out.iter().any(|x: &FishVoice| x.id == id) {
                out.push(FishVoice {
                    id: id.into(),
                    title: title.into(),
                    mine,
                });
            }
        }
    }
    Ok(out)
}

/// Check a self-hosted Fish Speech server is up.
pub async fn fish_health(fish: &Fish) -> Result<()> {
    let response = fish
        .auth(client(10)?.get(format!("{}/v1/health", fish.url)))
        .send()
        .await
        .with_context(|| format!("Couldn't reach the Fish Speech server at {}", fish.url))?;
    ensure!(
        response.status().is_success(),
        "The Fish Speech server answered {}",
        response.status()
    );
    Ok(())
}

/// Save a voice on a self-hosted Fish Speech server from a short recording and what it says.
pub async fn fish_add_voice(
    fish: &Fish,
    id: &str,
    audio: Vec<u8>,
    name: &str,
    text: &str,
) -> Result<()> {
    let id = id.trim();
    ensure!(
        !id.is_empty()
            && id.len() <= 255
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ' ')),
        "Name the voice with letters, numbers, spaces, - or _"
    );
    ensure!(!text.trim().is_empty(), "Write what the recording says");
    let form = reqwest::multipart::Form::new()
        .text("id", id.to_owned())
        .text("text", text.trim().to_owned())
        .part(
            "audio",
            reqwest::multipart::Part::bytes(audio).file_name(name.to_owned()),
        );
    let response = fish
        .auth(client(120)?.post(format!("{}/v1/references/add", fish.url)))
        .header("Accept", "application/json")
        .multipart(form)
        .send()
        .await
        .with_context(|| format!("Couldn't reach the Fish Speech server at {}", fish.url))?;
    let status = response.status();
    let body = response.text().await?;
    ensure!(
        status.is_success(),
        "The voice wasn't saved ({status}): {}",
        message(&body)
    );
    Ok(())
}

fn message(body: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    v["error"]["message"]
        .as_str()
        .or(v["error"].as_str())
        .or(v["message"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| body.chars().take(200).collect())
}

/// The format and samples of a PCM WAV file.
pub struct Wav {
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    pub data: Vec<u8>,
}

pub fn parse_wav(bytes: &[u8]) -> Result<Wav> {
    ensure!(
        bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "Not a WAV file"
    );
    let (mut fmt, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let mut len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into()?) as usize;
        let body = at + 8;
        // Streamed WAVs may give a placeholder length for the data chunk.
        if id == b"data" && (len == 0 || len == u32::MAX as usize || body + len > bytes.len()) {
            len = bytes.len() - body;
        }
        let end = (body + len).min(bytes.len());
        match id {
            b"fmt " if len >= 16 => fmt = Some(&bytes[body..end]),
            b"data" => data = Some(&bytes[body..end]),
            _ => {}
        }
        at = end + (len & 1);
    }
    let fmt = fmt.context("WAV file has no format")?;
    Ok(Wav {
        channels: u16::from_le_bytes([fmt[2], fmt[3]]),
        rate: u32::from_le_bytes(fmt[4..8].try_into()?),
        bits: u16::from_le_bytes([fmt[14], fmt[15]]),
        data: data.context("WAV file has no audio")?.to_vec(),
    })
}

/// A PCM WAV file.
pub fn wav(channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
    let block = channels * bits / 8;
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * block as u32).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// Join WAV pieces of the same format into one file.
fn join_wav(parts: &[Vec<u8>]) -> Result<Vec<u8>> {
    // Rewritten even when there's one piece: a streamed WAV can carry placeholder lengths
    // that some players reject. Anything that isn't a WAV passes through untouched.
    if parts.len() == 1 && parse_wav(&parts[0]).is_err() {
        return Ok(parts[0].clone());
    }
    let mut first: Option<Wav> = None;
    for p in parts {
        let w = parse_wav(p)?;
        match &mut first {
            None => first = Some(w),
            Some(f) => {
                ensure!(
                    f.rate == w.rate && f.channels == w.channels && f.bits == w.bits,
                    "The speech server changed audio format between pieces"
                );
                f.data.extend_from_slice(&w.data);
            }
        }
    }
    let f = first.context("No audio")?;
    Ok(wav(f.channels, f.rate, f.bits, &f.data))
}

/// Audio file types Spindle transcribes when attached.
pub fn is_audio(name: &str) -> bool {
    matches!(
        name.rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "wav" | "mp3" | "m4a" | "ogg" | "oga" | "opus" | "flac" | "webm"
    )
}

/// A file name for speech: "Hello there, friend" → "hello-there-friend.wav".
pub fn file_name(text: &str) -> String {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(5)
        .map(str::to_lowercase)
        .collect();
    if words.is_empty() {
        "speech.wav".into()
    } else {
        format!("{}.wav", words.join("-"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_made_speakable() {
        let said = speakable(
            "## Done\n\nI fixed **two** things in [the page](snake.html):\n\n```js\nlet x = 1;\n```\n- the `loop`\n\nSaved file: snake.html",
        );
        assert_eq!(said, "Done\nI fixed two things in the page:\nthe loop");
    }

    #[test]
    fn long_text_is_split_at_sentences() {
        let text = "One sentence here. ".repeat(400);
        let parts = chunks(&text);
        assert!(parts.len() >= 2);
        assert!(parts.iter().all(|p| p.len() <= CHUNK));
        assert!(parts[0].trim_end().ends_with('.'));
        assert_eq!(parts.concat(), text);
    }

    #[test]
    fn wav_pieces_join() {
        let a = wav(1, 24000, 16, &[1, 0, 2, 0]);
        let b = wav(1, 24000, 16, &[3, 0]);
        let joined = parse_wav(&join_wav(&[a, b]).unwrap()).unwrap();
        assert_eq!(joined.data, vec![1, 0, 2, 0, 3, 0]);
        assert_eq!(joined.rate, 24000);
    }

    /// A one-request HTTP server: returns what it received and answers with `reply`.
    async fn serve_once(
        content_type: &'static str,
        reply: Vec<u8>,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut got = vec![];
            let mut buf = [0u8; 65536];
            loop {
                let n = sock.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got).to_string();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let len = text[..head_end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if got.len() >= head_end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            );
            sock.write_all(head.as_bytes()).await.unwrap();
            sock.write_all(&reply).await.unwrap();
            String::from_utf8_lossy(&got).to_string()
        });
        (url, handle)
    }

    fn fish_prefs(url: &str) -> crate::state::Preferences {
        crate::state::Preferences {
            tts_provider: "fish".into(),
            stt_provider: "fish".into(),
            fish_key: "fish-key".into(),
            fish_url: url.into(),
            fish_voice: "voice-123".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn fish_audio_speaks_with_the_chosen_voice() {
        let (url, server) = serve_once("audio/wav", wav(1, 44100, 16, &[1, 0, 2, 0])).await;
        let settings = Settings::from(&fish_prefs(&url), &[], &[]);
        assert_eq!(settings.tts_model, FISH_TTS);
        let audio = speak(&settings, "Hello there.").await.unwrap();
        assert_eq!(parse_wav(&audio).unwrap().data, vec![1, 0, 2, 0]);
        let got = server.await.unwrap().to_lowercase();
        assert!(got.starts_with("post /v1/tts "), "{got}");
        assert!(got.contains("authorization: bearer fish-key"));
        assert!(got.contains("\r\nmodel: s2.1-pro\r\n"));
        assert!(got.contains(r#""reference_id":"voice-123""#));
        assert!(got.contains(r#""format":"wav""#));
    }

    #[tokio::test]
    async fn fish_audio_transcribes() {
        let (url, server) = serve_once(
            "application/json",
            br#"{"text":" Hello from Spindle. ","duration":1.2}"#.to_vec(),
        )
        .await;
        // Fish Audio's own service (a test server stands in for api.fish.audio).
        let settings = Settings {
            stt_model: FISH_STT.into(),
            stt_fish: Some(Fish {
                url: url.clone(),
                key: "fish-key".into(),
            }),
            ..Default::default()
        };
        let text = transcribe(&settings, b"RIFFfake".to_vec(), "memo.wav")
            .await
            .unwrap();
        assert_eq!(text, "Hello from Spindle.");
        let got = server.await.unwrap().to_lowercase();
        assert!(got.starts_with("post /v1/asr "), "{got}");
        assert!(got.contains("\r\nmodel: transcribe-1-pro\r\n"));
        assert!(got.contains(r#"name="audio"; filename="memo.wav""#));
    }

    #[test]
    fn fish_needs_a_key_and_can_be_mixed_with_ollama() {
        let mut p = fish_prefs("");
        p.fish_key.clear();
        let s = Settings::from(&p, &["lfm3-omni:latest".into()], &[]);
        assert!(s.tts_model.is_empty(), "no key: Fish can't speak");
        let mut p = fish_prefs("");
        p.stt_provider.clear();
        let s = Settings::from(&p, &[], &["lfm3-omni:latest".into()]);
        assert!(s.tts_fish.is_some() && s.stt_fish.is_none());
        assert_eq!(s.stt_model, "lfm3-omni:latest");
        assert_eq!(s.tts_fish.unwrap().url, FISH);
    }

    #[tokio::test]
    async fn a_local_fish_server_needs_no_key_and_lists_its_voices() {
        let (url, server) = serve_once(
            "application/json",
            br#"{"success":true,"reference_ids":["speaker-one","narrator"],"message":"ok"}"#.to_vec(),
        )
        .await;
        let mut p = fish_prefs(&url);
        p.fish_key.clear();
        let s = Settings::from(&p, &[], &["lfm3-omni:latest".into()]);
        let fish = s.tts_fish.clone().expect("a local server needs no key");
        assert!(fish.local());
        assert!(
            s.stt_fish.is_none(),
            "local Fish can't transcribe; Ollama does"
        );
        assert_eq!(s.stt_model, "lfm3-omni:latest");
        let voices = fish_voices(&fish, "").await.unwrap();
        assert_eq!(
            voices.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["speaker-one", "narrator"]
        );
        let got = server.await.unwrap().to_lowercase();
        assert!(got.starts_with("get /v1/references/list "), "{got}");
        assert!(got.contains("\r\naccept: application/json\r\n"));
        assert!(!got.contains("authorization"));
    }
}
