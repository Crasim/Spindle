//! Voice in the window: record from the microphone and transcribe into the message box,
//! read replies aloud, and transcribe audio files the user attaches.
//!
//! Recording uses the default input device (cpal); playback uses the default output
//! (rodio) on its own thread. Talking to the speech server happens on background threads, so
//! the window never waits.

use super::*;
use anyhow::{Context, Result, bail};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

/// The longest recording Spindle keeps (in seconds).
const MAX_SECONDS: usize = 600;
/// Recordings are sent at this rate: speech models want 16 kHz mono.
const RATE: u32 = 16_000;

struct Recording {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    channels: u16,
    started: std::time::Instant,
}

struct Player {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

/// Run a future on a background thread with its own runtime and send back the result.
fn background<T: Send + 'static>(
    ctx: Option<&egui::Context>,
    work: impl std::future::Future<Output = Result<T>> + Send + 'static,
) -> mpsc::Receiver<Result<T>> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.cloned();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)
            .and_then(|rt| rt.block_on(work));
        let _ = tx.send(result);
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    });
    rx
}

#[derive(Default)]
pub(super) struct Voice {
    recording: Option<Recording>,
    transcribing: Option<mpsc::Receiver<Result<String>>>,
    /// Speech being made, and which message (chat, index) it is for.
    making: Option<(mpsc::Receiver<Result<Vec<u8>>>, (u64, usize))>,
    player: Option<Player>,
    /// The message being read aloud.
    pub playing: Option<(u64, usize)>,
    /// Audio files being transcribed to attach: (chat, file name, result).
    attaching: Vec<(u64, String, mpsc::Receiver<Result<String>>)>,
    /// The newest reply already considered for reading aloud, per chat.
    read_up_to: std::collections::HashMap<u64, usize>,
}

impl Voice {
    pub fn recording(&self) -> Option<std::time::Duration> {
        self.recording.as_ref().map(|r| r.started.elapsed())
    }
    pub fn transcribing(&self) -> bool {
        self.transcribing.is_some()
    }
    pub fn busy_with(&self, at: (u64, usize)) -> bool {
        self.making.as_ref().is_some_and(|(_, m)| *m == at)
    }

    pub fn start_recording(&mut self) -> Result<()> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = cpal::default_host()
            .default_input_device()
            .context("No microphone found")?;
        let config = device
            .default_input_config()
            .context("The microphone can't be opened")?;
        let (rate, channels) = (config.sample_rate().0, config.channels());
        let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
        let limit = MAX_SECONDS * rate as usize * channels as usize;
        let err = |e| eprintln!("Microphone: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let s = samples.clone();
                device.build_input_stream(
                    &config.into(),
                    move |d: &[f32], _: &_| {
                        let mut b = s.lock().unwrap();
                        if b.len() < limit {
                            b.extend_from_slice(d);
                        }
                    },
                    err,
                    None,
                )?
            }
            cpal::SampleFormat::I16 => {
                let s = samples.clone();
                device.build_input_stream(
                    &config.into(),
                    move |d: &[i16], _: &_| {
                        let mut b = s.lock().unwrap();
                        if b.len() < limit {
                            b.extend(d.iter().map(|&x| x as f32 / 32768.));
                        }
                    },
                    err,
                    None,
                )?
            }
            cpal::SampleFormat::U16 => {
                let s = samples.clone();
                device.build_input_stream(
                    &config.into(),
                    move |d: &[u16], _: &_| {
                        let mut b = s.lock().unwrap();
                        if b.len() < limit {
                            b.extend(d.iter().map(|&x| (x as f32 - 32768.) / 32768.));
                        }
                    },
                    err,
                    None,
                )?
            }
            other => bail!("The microphone uses a sample format Spindle can't read ({other})"),
        };
        stream.play()?;
        self.recording = Some(Recording {
            _stream: stream,
            samples,
            rate,
            channels,
            started: std::time::Instant::now(),
        });
        Ok(())
    }

    /// Stop recording and send it off to be transcribed.
    pub fn stop_recording(&mut self, ctx: &egui::Context, settings: crate::speech::Settings) {
        let Some(rec) = self.recording.take() else {
            return;
        };
        let samples = std::mem::take(&mut *rec.samples.lock().unwrap());
        drop(rec._stream);
        let wav = to_wav(&samples, rec.rate, rec.channels);
        self.transcribing = Some(background(Some(ctx), async move {
            crate::speech::transcribe(&settings, wav, "recording.wav").await
        }));
    }

    pub fn cancel_recording(&mut self) {
        self.recording = None;
    }

    /// Read text aloud for a message.
    pub fn speak(
        &mut self,
        ctx: &egui::Context,
        settings: crate::speech::Settings,
        text: String,
        at: (u64, usize),
    ) {
        self.stop_playing();
        let said = crate::speech::speakable(&text);
        self.making = Some((
            background(Some(ctx), async move {
                crate::speech::speak(&settings, &said).await
            }),
            at,
        ));
    }

    pub fn stop_playing(&mut self) {
        if let Some(p) = self.player.take() {
            p.stop.store(true, Ordering::Relaxed);
        }
        self.making = None;
        self.playing = None;
    }

    /// Transcribe an attached audio file; the transcript is attached when it's ready.
    pub fn attach(
        &mut self,
        settings: crate::speech::Settings,
        chat: u64,
        path: &std::path::Path,
    ) -> Result<String> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let audio = std::fs::read(path)?;
        anyhow::ensure!(audio.len() <= 100 << 20, "{name} is larger than 100 MB");
        let n = name.clone();
        self.attaching.push((
            chat,
            name.clone(),
            background(None, async move {
                crate::speech::transcribe(&settings, audio, &n).await
            }),
        ));
        Ok(name)
    }

    /// Whether a new reply should be read aloud now: the newest assistant message of the
    /// chat, once, and not while the chat is still working.
    pub fn should_read(
        &mut self,
        chat: u64,
        history: &[crate::ollama::Message],
        busy: bool,
    ) -> Option<usize> {
        let last = history.iter().rposition(|m| m.role == "assistant")?;
        let seen = self.read_up_to.entry(chat).or_insert(last);
        if busy || last <= *seen {
            return None;
        }
        *seen = last;
        Some(last)
    }

    /// Check on background work. Returns finished transcripts for the message box, and
    /// transcripts of attached files as (chat, name, text), and any error to show.
    pub fn poll(&mut self, ctx: &egui::Context) -> Polled {
        let mut out = Polled::default();
        if self.recording.is_some() || !self.attaching.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if let Some(rx) = &self.transcribing {
            match rx.try_recv() {
                Ok(Ok(text)) => {
                    out.dictated = Some(text);
                    self.transcribing = None;
                }
                Ok(Err(e)) => {
                    out.error = Some(format!("Couldn't transcribe: {e:#}"));
                    self.transcribing = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.transcribing = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((rx, at)) = &self.making {
            let at = *at;
            match rx.try_recv() {
                Ok(Ok(wav)) => {
                    self.making = None;
                    self.player = Some(play(ctx, wav));
                    self.playing = Some(at);
                }
                Ok(Err(e)) => {
                    out.error = Some(format!("Couldn't read it aloud: {e:#}"));
                    self.making = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.making = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self
            .player
            .as_ref()
            .is_some_and(|p| p.done.load(Ordering::Relaxed))
        {
            self.player = None;
            self.playing = None;
        }
        self.attaching
            .retain(|(chat, name, rx)| match rx.try_recv() {
                Ok(Ok(text)) => {
                    out.attached.push((*chat, name.clone(), text));
                    false
                }
                Ok(Err(e)) => {
                    out.error = Some(format!("Couldn't transcribe {name}: {e:#}"));
                    false
                }
                Err(mpsc::TryRecvError::Disconnected) => false,
                Err(mpsc::TryRecvError::Empty) => true,
            });
        out
    }
}

#[derive(Default)]
pub(super) struct Polled {
    pub dictated: Option<String>,
    pub attached: Vec<(u64, String, String)>,
    pub error: Option<String>,
}

/// Play a sound file on the default output, on its own thread.
fn play(ctx: &egui::Context, bytes: Vec<u8>) -> Player {
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let (s, d, ctx) = (stop.clone(), done.clone(), ctx.clone());
    std::thread::spawn(move || {
        let run = || -> Result<()> {
            let (_stream, handle) = rodio::OutputStream::try_default()?;
            let sink = rodio::Sink::try_new(&handle)?;
            sink.append(rodio::Decoder::new(std::io::Cursor::new(bytes))?);
            while !sink.empty() {
                if s.load(Ordering::Relaxed) {
                    sink.stop();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
            Ok(())
        };
        if let Err(e) = run() {
            eprintln!("Playback: {e:#}");
        }
        d.store(true, Ordering::Relaxed);
        ctx.request_repaint();
    });
    Player { stop, done }
}

/// Mix to mono, resample to 16 kHz and write 16-bit WAV.
pub(super) fn to_wav(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
    let ch = channels.max(1) as usize;
    let mono: Vec<f32> = samples
        .chunks(ch)
        .map(|f| f.iter().sum::<f32>() / f.len() as f32)
        .collect();
    let out_len = (mono.len() as u64 * RATE as u64 / rate.max(1) as u64) as usize;
    let mut pcm = Vec::with_capacity(out_len * 2);
    for i in 0..out_len {
        let pos = i as f64 * rate as f64 / RATE as f64;
        let j = pos as usize;
        let frac = (pos - j as f64) as f32;
        let a = mono.get(j).copied().unwrap_or(0.);
        let b = mono.get(j + 1).copied().unwrap_or(a);
        let v = ((a + (b - a) * frac).clamp(-1., 1.) * 32767.) as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    crate::speech::wav(1, RATE, 16, &pcm)
}

/// The small speaker button on a reply: Listen, or Stop while it plays.
pub(super) fn listen_button(ui: &mut egui::Ui, playing: bool, making: bool) -> bool {
    if making {
        ui.add(egui::Spinner::new().size(12.));
        return false;
    }
    let (r, res) = ui.allocate_exact_size(egui::vec2(18., 18.), egui::Sense::click());
    let color = if res.hovered() || playing {
        accent()
    } else {
        muted()
    };
    draw(
        ui,
        if playing { Icon::Stop } else { Icon::Speaker },
        r.shrink(if playing { 3. } else { 1. }),
        color,
    );
    res.on_hover_text(if playing {
        "Stop reading"
    } else {
        "Read aloud"
    })
    .clicked()
}

#[cfg(test)]
mod tests {
    #[test]
    fn recordings_become_16k_mono_wav() {
        // One second of 48 kHz stereo.
        let samples = vec![0.5f32; 48_000 * 2];
        let wav = super::to_wav(&samples, 48_000, 2);
        let w = crate::speech::parse_wav(&wav).unwrap();
        assert_eq!((w.rate, w.channels, w.bits), (16_000, 1, 16));
        assert_eq!(w.data.len(), 16_000 * 2);
    }
}
