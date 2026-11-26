//! Settings › Models › Audio: which server and models speak and listen.

use super::*;
use anyhow::Result;
use std::sync::mpsc;

#[derive(Default)]
pub(super) struct VoiceForm {
    loaded: bool,
    url: String,
    key: String,
    tts: String,
    voice: String,
    stt: String,
    read_aloud: bool,
    voices: Vec<String>,
    voices_for: String,
    loading: Option<mpsc::Receiver<Result<Vec<String>>>>,
    tts_provider: String,
    stt_provider: String,
    fish_key: String,
    fish_url: String,
    fish_tts: String,
    fish_stt: String,
    fish_voice: String,
    fish_voice_name: String,
    fish_search: String,
    fish_voices: Vec<crate::speech::FishVoice>,
    fish_loading: Option<mpsc::Receiver<Result<Vec<crate::speech::FishVoice>>>>,
    fish_status: String,
    /// Saving a voice on a local Fish Speech server.
    add_open: bool,
    add_path: String,
    add_id: String,
    add_text: String,
    add_status: String,
    adding: Option<mpsc::Receiver<Result<()>>>,
    writing: Option<mpsc::Receiver<Result<String>>>,
    checking: Option<mpsc::Receiver<Result<()>>>,
}

/// Run a future on a background thread and hand back its result.
fn spawn<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl std::future::Future<Output = Result<T>> + Send + 'static,
) -> mpsc::Receiver<Result<T>> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let r = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(anyhow::Error::from)
            .and_then(|rt| rt.block_on(work));
        let _ = tx.send(r);
        ctx.request_repaint();
    });
    rx
}

const FISH_TTS_MODELS: [&str; 6] = [
    "s2.1-pro",
    "s2-pro",
    "s3-pro",
    "drama-3",
    "s1",
    "s2.1-pro-free",
];
const FISH_STT_MODELS: [&str; 2] = ["transcribe-1-pro", "transcribe-1"];

fn provider_name(p: &str) -> &'static str {
    if p == "fish" {
        "Fish Audio"
    } else {
        "Ollama or speech server"
    }
}

impl VoiceForm {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        prefs: &crate::state::Preferences,
        effective: &crate::speech::Settings,
        speaking: &std::collections::BTreeSet<String>,
        hearing: &std::collections::BTreeSet<String>,
    ) -> Option<String> {
        if !self.loaded {
            self.loaded = true;
            self.url = prefs.speech_url.clone();
            self.key = prefs.speech_key.clone();
            self.tts = prefs.tts_model.clone();
            self.voice = prefs.tts_voice.clone();
            self.stt = prefs.stt_model.clone();
            self.read_aloud = prefs.read_aloud;
            self.tts_provider = prefs.tts_provider.clone();
            self.stt_provider = prefs.stt_provider.clone();
            self.fish_key = prefs.fish_key.clone();
            self.fish_url = prefs.fish_url.clone();
            self.fish_tts = prefs.fish_tts_model.clone();
            self.fish_stt = prefs.fish_stt_model.clone();
            self.fish_voice = prefs.fish_voice.clone();
            self.fish_voice_name = prefs.fish_voice_name.clone();
        }
        if let Some(rx) = &self.checking
            && let Ok(r) = rx.try_recv()
        {
            self.fish_status = match r {
                Ok(()) => "The Fish Speech server is running".into(),
                Err(e) => format!("{e:#}"),
            };
            self.checking = None;
        }
        if let Some(rx) = &self.fish_loading
            && let Ok(r) = rx.try_recv()
        {
            match r {
                Ok(v) => {
                    self.fish_status = format!("{} voices found", v.len());
                    self.fish_voices = v;
                }
                Err(e) => self.fish_status = format!("{e:#}"),
            }
            self.fish_loading = None;
        }
        // Voices for the model that would speak, from Ollama.
        let model = if self.tts.is_empty() {
            crate::speech::Settings::from(
                &crate::state::Preferences {
                    tts_provider: String::new(),
                    ..prefs.clone()
                },
                &speaking.iter().cloned().collect::<Vec<_>>(),
                &[],
            )
            .tts_model
        } else {
            self.tts.clone()
        };
        if !model.is_empty() && self.voices_for != model && self.loading.is_none() {
            self.voices_for = model.clone();
            let url = effective.url.clone();
            let (tx, rx) = mpsc::channel();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let r = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(anyhow::Error::from)
                    .and_then(|rt| rt.block_on(crate::speech::voices(&url, &model)));
                let _ = tx.send(r);
                ctx.request_repaint();
            });
            self.loading = Some(rx);
        }
        if let Some(rx) = &self.loading
            && let Ok(r) = rx.try_recv()
        {
            self.voices = r.unwrap_or_default();
            self.loading = None;
        }
        let tags = |set: &std::collections::BTreeSet<String>| -> Vec<String> {
            set.iter()
                .filter(|m| route(m).0 == "ollama")
                .map(|m| route(m).1.to_owned())
                .collect()
        };
        let (speakers, hearers) = (tags(speaking), tags(hearing));
        // What the Ollama side would use, whichever side Fish Audio takes.
        let plain = crate::speech::Settings::from(
            &crate::state::Preferences {
                tts_provider: String::new(),
                stt_provider: String::new(),
                ..prefs.clone()
            },
            &speaking.iter().cloned().collect::<Vec<_>>(),
            &hearing.iter().cloned().collect::<Vec<_>>(),
        );
        let mut command = None;
        egui::Frame::new()
            .fill(panel())
            .inner_margin(16)
            .corner_radius(10)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(Text::new("Audio").strong().size(18.));
                ui.weak("Dictate with the microphone button, hear replies with the speaker button, and attach recordings to have them written down. Ollama does this with models that speak and hear, such as lfm3-omni; Fish Audio does it as a paid service with many voices.");
                ui.add_space(6.);
                let auto = |chosen: &str, effective: &str| {
                    if effective.is_empty() {
                        "None available".to_owned()
                    } else if chosen.is_empty() {
                        format!("Automatic ({effective})")
                    } else {
                        chosen.to_owned()
                    }
                };
                egui::Grid::new("audio-settings")
                    .num_columns(2)
                    .spacing([10., 8.])
                    .show(ui, |ui| {
                        for (label, id, value) in [
                            ("Speak with", "tts-provider", &mut self.tts_provider),
                            ("Transcribe with", "stt-provider", &mut self.stt_provider),
                        ] {
                            ui.label(label);
                            crate::widgets::InlineChoice::from_id_salt(id)
                                .selected_text(provider_name(value))
                                .width(280.)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(value, String::new(), provider_name(""));
                                    ui.selectable_value(value, "fish".to_owned(), provider_name("fish"));
                                });
                            ui.end_row();
                        }
                        let ollama_tts = self.tts_provider != "fish";
                        let ollama_stt = self.stt_provider != "fish";
                        if ollama_tts {
                            ui.label("Text to speech");
                            crate::widgets::InlineChoice::from_id_salt("tts-model")
                                .selected_text(auto(&self.tts, &plain.tts_model))
                                .width(280.)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.tts, String::new(), "Automatic");
                                    for m in &speakers {
                                        ui.selectable_value(&mut self.tts, m.clone(), m);
                                    }
                                });
                            ui.end_row();
                            ui.label("Voice");
                            crate::widgets::InlineChoice::from_id_salt("tts-voice")
                                .selected_text(if self.voice.is_empty() {
                                    "default"
                                } else {
                                    &self.voice
                                })
                                .width(280.)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.voice, String::new(), "default");
                                    for v in self.voices.iter().filter(|v| *v != "default") {
                                        ui.selectable_value(&mut self.voice, v.clone(), v);
                                    }
                                });
                            ui.end_row();
                        }
                        if ollama_stt {
                            ui.label("Speech to text");
                            crate::widgets::InlineChoice::from_id_salt("stt-model")
                                .selected_text(auto(&self.stt, &plain.stt_model))
                                .width(280.)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.stt, String::new(), "Automatic");
                                    for m in &hearers {
                                        ui.selectable_value(&mut self.stt, m.clone(), m);
                                    }
                                });
                            ui.end_row();
                        }
                        if ollama_tts || ollama_stt {
                            ui.label("Server");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.url)
                        .id_salt("audio_settings-field-1")
                                    .hint_text(format!("Ollama ({})", crate::ollama::endpoint()))
                                    .desired_width((ui.available_width().max(230.)).min(280.)),
                            )
                            .on_hover_text("Any server with the OpenAI audio API (/v1/audio/speech and /v1/audio/transcriptions). Leave empty for Ollama. With another server, type its model names below.");
                            ui.end_row();
                            if !self.url.trim().is_empty() {
                                ui.label("API key");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.key)
                        .id_salt("audio_settings-field-2")
                                        .password(true)
                                        .hint_text("Only if the server needs one")
                                        .desired_width((ui.available_width().max(230.)).min(280.)),
                                );
                                ui.end_row();
                                ui.label("Model names");
                                ui.horizontal_wrapped(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.tts)
                        .id_salt("audio_settings-field-3")
                                            .hint_text("speech model")
                                            .desired_width(135.),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.stt)
                        .id_salt("audio_settings-field-4")
                                            .hint_text("transcription model")
                                            .desired_width(135.),
                                    );
                                });
                                ui.end_row();
                            }
                        }
                        if !ollama_tts || !ollama_stt {
                            self.fish_rows(ui, ollama_tts, ollama_stt, effective);
                        }
                    });
                ui.checkbox(&mut self.read_aloud, "Read each reply aloud when it finishes");
                if effective.tts_model.is_empty() && effective.stt_model.is_empty() {
                    ui.weak("Nothing can speak or hear yet. In Ollama: ollama pull lfm3-omni, or set up Fish below.");
                }
                ui.add_space(4.);
                let changed = self.url.trim() != prefs.speech_url
                    || self.key != prefs.speech_key
                    || self.tts.trim() != prefs.tts_model
                    || self.voice != prefs.tts_voice
                    || self.stt.trim() != prefs.stt_model
                    || self.read_aloud != prefs.read_aloud
                    || self.tts_provider != prefs.tts_provider
                    || self.stt_provider != prefs.stt_provider
                    || self.fish_key.trim() != prefs.fish_key
                    || self.fish_url.trim() != prefs.fish_url
                    || self.fish_tts != prefs.fish_tts_model
                    || self.fish_stt != prefs.fish_stt_model
                    || self.fish_voice != prefs.fish_voice;
                if ui.add_enabled(changed, egui::Button::new("Save")).clicked() {
                    let mut p = prefs.clone();
                    p.speech_url = if self.url.trim().is_empty() {
                        String::new()
                    } else {
                        super::side_tabs::to_url(&self.url)
                    };
                    p.speech_key = self.key.clone();
                    p.tts_model = self.tts.trim().to_owned();
                    p.tts_voice = self.voice.clone();
                    p.stt_model = self.stt.trim().to_owned();
                    p.read_aloud = self.read_aloud;
                    p.tts_provider = self.tts_provider.clone();
                    p.stt_provider = self.stt_provider.clone();
                    p.fish_key = self.fish_key.trim().to_owned();
                    p.fish_url = if self.fish_url.trim().is_empty() {
                        String::new()
                    } else {
                        super::side_tabs::to_url(&self.fish_url)
                    };
                    p.fish_tts_model = self.fish_tts.clone();
                    p.fish_stt_model = self.fish_stt.clone();
                    p.fish_voice = self.fish_voice.clone();
                    p.fish_voice_name = self.fish_voice_name.clone();
                    self.url = p.speech_url.clone();
                    command = Some(format!(
                        "/preferences {}",
                        serde_json::to_string(&p).unwrap()
                    ));
                }
            });
        command
    }

    fn fish(&self) -> crate::speech::Fish {
        crate::speech::Fish {
            url: if self.fish_url.trim().is_empty() {
                crate::speech::FISH.into()
            } else {
                super::side_tabs::to_url(self.fish_url.trim())
                    .trim_end_matches('/')
                    .to_owned()
            },
            key: self.fish_key.trim().to_owned(),
        }
    }

    fn fish_rows(
        &mut self,
        ui: &mut egui::Ui,
        ollama_tts: bool,
        ollama_stt: bool,
        effective: &crate::speech::Settings,
    ) {
        let local = !self.fish_url.trim().is_empty();
        ui.label(Text::new("Fish").strong());
        ui.horizontal_wrapped(|ui| {
            if ui.radio(!local, "Fish Audio service").clicked() {
                self.fish_url.clear();
                self.fish_voices.clear();
                self.fish_status.clear();
            }
            if ui.radio(local, "My Fish Speech server").clicked() && !local {
                self.fish_url = "http://127.0.0.1:8080".into();
                self.fish_voices.clear();
                self.fish_status.clear();
            }
        });
        ui.end_row();
        if local {
            ui.label("Server");
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.fish_url)
                        .id_salt("audio_settings-field-5")
                        .hint_text("http://127.0.0.1:8080")
                        .desired_width((ui.available_width().max(230.)).min(200.)),
                );
                if ui
                    .add_enabled(self.checking.is_none(), egui::Button::new("Test"))
                    .clicked()
                {
                    let fish = self.fish();
                    self.checking = Some(spawn(ui.ctx(), async move {
                        crate::speech::fish_health(&fish).await
                    }));
                    self.fish_status = "Connecting…".into();
                }
            });
            ui.end_row();
            ui.label("");
            ui.vertical(|ui| {
                ui.weak("Start it from your fish-speech folder:");
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        Text::new(crate::speech::FISH_LOCAL_COMMAND)
                            .monospace()
                            .size(11.),
                    );
                    if ui.small_button("Copy").clicked() {
                        ui.ctx().copy_text(crate::speech::FISH_LOCAL_COMMAND.into());
                    }
                });
            });
            ui.end_row();
            ui.label("API key");
            ui.add(
                egui::TextEdit::singleline(&mut self.fish_key)
                    .id_salt("audio_settings-field-6")
                    .password(true)
                    .hint_text("Only if started with --api-key")
                    .desired_width((ui.available_width().max(230.)).min(280.)),
            );
            ui.end_row();
        } else {
            ui.label("API key");
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.fish_key)
                        .id_salt("audio_settings-field-7")
                        .password(true)
                        .hint_text("From fish.audio › API keys")
                        .desired_width((ui.available_width().max(230.)).min(200.)),
                );
                ui.hyperlink_to("Get one", "https://fish.audio/app/api-keys/");
            });
            ui.end_row();
        }
        if !ollama_tts {
            if !local {
                ui.label("Speech model");
                crate::widgets::InlineChoice::from_id_salt("fish-tts")
                    .selected_text(if self.fish_tts.is_empty() {
                        crate::speech::FISH_TTS
                    } else {
                        &self.fish_tts
                    })
                    .width(280.)
                    .show_ui(ui, |ui| {
                        for m in FISH_TTS_MODELS {
                            let value = if m == crate::speech::FISH_TTS {
                                String::new()
                            } else {
                                m.to_owned()
                            };
                            ui.selectable_value(&mut self.fish_tts, value, m);
                        }
                    });
                ui.end_row();
            }
            ui.label("Voice");
            ui.vertical(|ui| {
                let default = if local {
                    "The server's default"
                } else {
                    "Fish Audio's default"
                };
                crate::widgets::InlineChoice::from_id_salt("fish-voice")
                    .selected_text(if self.fish_voice.is_empty() {
                        default.to_owned()
                    } else if self.fish_voice_name.is_empty() {
                        self.fish_voice.clone()
                    } else {
                        self.fish_voice_name.clone()
                    })
                    .width(280.)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.fish_voice.is_empty(), default)
                            .clicked()
                        {
                            self.fish_voice.clear();
                            self.fish_voice_name.clear();
                        }
                        for v in &self.fish_voices {
                            let label = if v.mine && !local {
                                format!("{} (yours)", v.title)
                            } else {
                                v.title.clone()
                            };
                            if ui
                                .selectable_label(self.fish_voice == v.id, label)
                                .clicked()
                            {
                                self.fish_voice = v.id.clone();
                                self.fish_voice_name = v.title.clone();
                            }
                        }
                    });
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.fish_search)
                            .id_salt("audio_settings-field-8")
                            .hint_text("Search voices")
                            .desired_width(160.),
                    );
                    let ready =
                        (local || !self.fish_key.trim().is_empty()) && self.fish_loading.is_none();
                    if ui
                        .add_enabled(ready, egui::Button::new("Find voices"))
                        .on_hover_text(if local {
                            "The voices saved on your server"
                        } else {
                            "Your own voices first, then popular ones"
                        })
                        .clicked()
                    {
                        let (fish, search) = (self.fish(), self.fish_search.clone());
                        self.fish_loading = Some(spawn(ui.ctx(), async move {
                            crate::speech::fish_voices(&fish, &search).await
                        }));
                        self.fish_status = "Looking…".into();
                    }
                    if local && ui.button("Add a voice…").clicked() {
                        self.add_open = !self.add_open;
                    }
                    if self.fish_loading.is_some() || self.checking.is_some() {
                        ui.add(egui::Spinner::new().size(14.));
                    }
                });
                if !self.fish_status.is_empty() {
                    ui.weak(&self.fish_status);
                }
                if local && self.add_open {
                    self.add_voice(ui, effective);
                }
            });
            ui.end_row();
        }
        if !ollama_stt {
            if local {
                ui.label("Transcription");
                ui.weak("A Fish Speech server can't transcribe, so Ollama's speech-to-text model does it.");
                ui.end_row();
            } else {
                ui.label("Transcription model");
                crate::widgets::InlineChoice::from_id_salt("fish-stt")
                    .selected_text(if self.fish_stt.is_empty() {
                        crate::speech::FISH_STT
                    } else {
                        &self.fish_stt
                    })
                    .width(280.)
                    .show_ui(ui, |ui| {
                        for m in FISH_STT_MODELS {
                            let value = if m == crate::speech::FISH_STT {
                                String::new()
                            } else {
                                m.to_owned()
                            };
                            ui.selectable_value(&mut self.fish_stt, value, m);
                        }
                    });
                ui.end_row();
            }
        }
    }

    /// Save a new voice on the local server: a 10–30 second recording and what it says.
    fn add_voice(&mut self, ui: &mut egui::Ui, effective: &crate::speech::Settings) {
        if let Some(rx) = &self.writing
            && let Ok(r) = rx.try_recv()
        {
            match r {
                Ok(t) => self.add_text = t,
                Err(e) => self.add_status = format!("{e:#}"),
            }
            self.writing = None;
        }
        if let Some(rx) = &self.adding
            && let Ok(r) = rx.try_recv()
        {
            self.add_status = match r {
                Ok(()) => {
                    self.fish_voice = self.add_id.trim().to_owned();
                    self.fish_voice_name = self.fish_voice.clone();
                    self.fish_voices.push(crate::speech::FishVoice {
                        id: self.fish_voice.clone(),
                        title: self.fish_voice.clone(),
                        mine: true,
                    });
                    self.add_open = false;
                    format!(
                        "Saved the voice {}. Press Save below to use it.",
                        self.fish_voice
                    )
                }
                Err(e) => format!("{e:#}"),
            };
            self.fish_status = self.add_status.clone();
            self.adding = None;
        }
        egui::Frame::new()
            .stroke(egui::Stroke::new(1_f32, line()))
            .corner_radius(8)
            .inner_margin(10)
            .show(ui, |ui| {
                ui.weak("A clear 10–30 second recording of the voice, and exactly what it says.");
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.add_path)
                            .id_salt("audio_settings-field-9")
                            .hint_text("Recording (.wav or .mp3)")
                            .desired_width((ui.available_width().max(230.)).min(200.)),
                    );
                    #[cfg(windows)]
                    if ui.button("Browse…").clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("Audio", &["wav", "mp3", "flac", "m4a", "ogg"])
                            .pick_file()
                    {
                        self.add_path = path.display().to_string();
                    }
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.add_id)
                        .id_salt("audio_settings-field-10")
                        .hint_text("Voice name, e.g. narrator")
                        .desired_width((ui.available_width().max(230.)).min(200.)),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut self.add_text)
                        .id_salt("audio_settings-field-11")
                        .hint_text("What the recording says")
                        .desired_rows(3)
                        .desired_width((ui.available_width().max(230.)).min(280.)),
                );
                ui.horizontal_wrapped(|ui| {
                    let path = std::path::PathBuf::from(self.add_path.trim());
                    let can_write = !effective.stt_model.is_empty() && path.is_file();
                    if ui
                        .add_enabled(
                            can_write && self.writing.is_none(),
                            egui::Button::new("Write it down for me"),
                        )
                        .on_hover_text("Transcribe the recording with your speech-to-text model")
                        .clicked()
                    {
                        match std::fs::read(&path) {
                            Ok(audio) => {
                                let settings = effective.clone();
                                let name = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                self.writing = Some(spawn(ui.ctx(), async move {
                                    crate::speech::transcribe(&settings, audio, &name).await
                                }));
                            }
                            Err(e) => self.add_status = e.to_string(),
                        }
                    }
                    let ready = path.is_file()
                        && !self.add_id.trim().is_empty()
                        && !self.add_text.trim().is_empty()
                        && self.adding.is_none();
                    if ui
                        .add_enabled(ready, egui::Button::new("Save voice"))
                        .clicked()
                    {
                        match std::fs::read(&path) {
                            Ok(audio) => {
                                let fish = self.fish();
                                let (id, text) = (self.add_id.clone(), self.add_text.clone());
                                let name = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                self.adding = Some(spawn(ui.ctx(), async move {
                                    crate::speech::fish_add_voice(&fish, &id, audio, &name, &text)
                                        .await
                                }));
                                self.add_status = "Saving…".into();
                            }
                            Err(e) => self.add_status = e.to_string(),
                        }
                    }
                    if self.writing.is_some() || self.adding.is_some() {
                        ui.add(egui::Spinner::new().size(14.));
                    }
                });
                if !self.add_status.is_empty() {
                    ui.weak(&self.add_status);
                }
            });
    }
}
