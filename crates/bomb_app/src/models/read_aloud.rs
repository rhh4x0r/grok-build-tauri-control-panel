//! Reading replies aloud: which message is playing, where, how fast, in which voice. One message
//! plays at a time; starting another replaces it. Playback is this Mac's, so it lives here rather
//! than in bomb_core (which also runs on servers). See `crate::speech` for the engine.

use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::runtime::{services, spawn_service};
use crate::speech::{self, HelperEngine, SpeechEngine, SpeechEvent, SpeechStatus, Voice, VoiceList};

const SETTINGS_KEY: &str = "read_aloud";
/// The speeds the player offers.
pub const RATES: [f32; 5] = [0.75, 1.0, 1.25, 1.5, 2.0];

pub struct ReadAloudHandle(pub Entity<ReadAloud>);
impl Global for ReadAloudHandle {}

pub fn read_aloud(cx: &App) -> Entity<ReadAloud> {
    cx.global::<ReadAloudHandle>().0.clone()
}

/// Saved choices: the Fish Audio voice (none → the default) and the speed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadAloudSettings {
    /// A Fish Audio voice id. Older settings held an Apple voice here; those aren't Fish ids.
    pub voice: Option<String>,
    #[serde(default)]
    pub voice_name: Option<String>,
    pub rate: f32,
}

impl Default for ReadAloudSettings {
    fn default() -> Self {
        Self { voice: None, voice_name: None, rate: 1.0 }
    }
}

/// The message being read: its thread, its entry, and a line to show for it.
#[derive(Debug, Clone, PartialEq)]
pub struct Playing {
    pub thread: uuid::Uuid,
    pub entry: u64,
    pub title: String,
    pub sentences: Vec<String>,
}

#[derive(Default)]
pub struct ReadAloud {
    /// This Mac can read aloud (known once checked).
    pub available: bool,
    pub settings: ReadAloudSettings,
    /// The Fish Audio key, once loaded (None: not set).
    pub api_key: Option<String>,
    /// The voice picker: which list, the search, what came back, and any trouble.
    pub voices: Vec<Voice>,
    pub voice_list: VoiceList,
    pub query: String,
    pub voices_loading: bool,
    pub voices_error: Option<String>,
    /// A key being checked with Fish Audio before it's saved, and why the last one wasn't.
    checking_key: Option<String>,
    pub key_error: Option<String>,
    /// A key was just saved: the settings page empties its field.
    pub clear_key_input: bool,
    /// The voice whose preview is playing.
    pub previewing: Option<String>,
    /// The settings page's text fields, made on first show (they need a window).
    pub key_input: Option<Entity<gpui_kit::component::input::InputState>>,
    pub search_input: Option<Entity<gpui_kit::component::input::InputState>>,
    pub playing: Option<Playing>,
    pub status: SpeechStatus,
    engine: Option<HelperEngine>,
    /// Ask the thread view to scroll to the playing message.
    pub reveal: Option<(uuid::Uuid, u64)>,
}

impl ReadAloud {
    /// Saved settings, and whether this Mac can read aloud at all.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let state = services(cx);
        let this = cx.entity().downgrade();
        spawn_service(cx, async move {
            let saved = bomb_core::services::kv_get(&state, SETTINGS_KEY).await.ok().flatten();
            let key = bomb_core::services::speech_key::load(state.persistence.clone()).await;
            let available = tokio::task::spawn_blocking(speech::available).await.unwrap_or(false);
            Ok::<_, String>((saved, key, available))
        }, move |res, cx| {
            let Ok((saved, key, available)) = res else { return };
            let _ = this.update(cx, |m, cx| {
                m.available = available;
                m.api_key = key;
                if let Some(mut settings) = saved.and_then(|raw| serde_json::from_str::<ReadAloudSettings>(&raw).ok()) {
                    // An Apple voice from before Fish Audio: start over with the default.
                    if settings.voice.as_deref().is_some_and(|v| v.contains('.')) { settings.voice = None; settings.voice_name = None; }
                    m.settings = settings;
                }
                cx.notify();
            });
        });
    }

    fn save(&self, cx: &mut Context<Self>) {
        let raw = serde_json::to_string(&self.settings).unwrap_or_default();
        let state = services(cx);
        spawn_service(cx, async move { bomb_core::services::kv_set(&state, SETTINGS_KEY, &raw).await }, |_, _| {});
    }

    /// The helper, started on first use; its reports update this model.
    fn engine(&mut self, cx: &mut Context<Self>) -> Option<&mut HelperEngine> {
        if self.engine.is_none() {
            let (engine, events) = HelperEngine::start().ok()?;
            self.engine = Some(engine);
            cx.spawn(async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    if this.update(cx, |m, cx| { m.on_event(event, cx); cx.notify(); }).is_err() { break; }
                }
            })
            .detach();
        }
        self.engine.as_mut()
    }

    fn on_event(&mut self, event: SpeechEvent, cx: &mut Context<Self>) {
        match event {
            SpeechEvent::Status(status) => {
                // A report about something no longer playing doesn't count.
                if self.playing.is_some() || status.key.is_empty() { self.status = status; }
            }
            SpeechEvent::Voices { voices, list, query } => {
                // Only the answer to the latest search counts.
                if list == self.voice_list.key() && query == self.query.trim() {
                    self.voices = voices;
                    self.voices_loading = false;
                    self.voices_error = None;
                }
            }
            SpeechEvent::Previewing(voice) => self.previewing = voice,
            SpeechEvent::KeyChecked(result) => {
                let Some(key) = self.checking_key.take() else { return };
                match result {
                    Ok(()) => self.store_api_key(key, cx),
                    Err(error) => self.key_error = Some(error),
                }
            }
            SpeechEvent::VoicesFailed(error) => {
                self.voices_loading = false;
                self.voices_error = Some(error);
            }
        }
    }

    /// The voice to read with: the chosen one, else Fish Audio's "Sarah".
    fn voice(&self) -> String {
        self.settings.voice.clone().unwrap_or_else(|| speech::DEFAULT_VOICE.to_string())
    }

    /// Read a reply, or stop it when it's the one playing.
    pub fn toggle(&mut self, thread: uuid::Uuid, entry: u64, markdown: &str, title: String, cx: &mut Context<Self>) {
        if self.playing.as_ref().is_some_and(|p| p.thread == thread && p.entry == entry) {
            self.close(cx);
            return;
        }
        let sentences = bomb_core::transcript_speech::speakable_sentences(markdown);
        if sentences.is_empty() { return; }
        let voice = self.voice();
        let rate = self.settings.rate;
        let key = speech::cache_key(&format!("{thread}-{entry}"), &voice, &sentences);
        self.playing = Some(Playing { thread, entry, title, sentences: sentences.clone() });
        let Some(api_key) = self.api_key.clone() else {
            self.status = SpeechStatus { key, failed: Some("Add your Fish Audio API key in Settings → Voice to read replies aloud.".into()), ..Default::default() };
            cx.notify();
            return;
        };
        self.status = SpeechStatus { key: key.clone(), playing: true, ..Default::default() };
        if let Some(engine) = self.engine(cx) {
            engine.play(&key, &sentences, &voice, &api_key, rate, 0.0);
        }
        cx.notify();
    }

    pub fn play_pause(&mut self, cx: &mut Context<Self>) {
        let playing = self.status.playing;
        if let Some(engine) = self.engine.as_mut() {
            if playing { engine.pause() } else { engine.resume() }
        }
        cx.notify();
    }

    pub fn seek(&mut self, seconds: f64, cx: &mut Context<Self>) {
        self.status.position = seconds.clamp(0.0, self.status.duration);
        if let Some(engine) = self.engine.as_mut() { engine.seek(seconds); }
        cx.notify();
    }

    pub fn skip(&mut self, seconds: f64, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine.as_mut() { engine.skip(seconds); }
        cx.notify();
    }

    /// The player's speed, which is also the default from now on.
    pub fn set_rate(&mut self, rate: f32, cx: &mut Context<Self>) {
        self.settings.rate = rate;
        self.save(cx);
        if let Some(engine) = self.engine.as_mut() { engine.set_rate(rate); }
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.playing = None;
        self.status = SpeechStatus::default();
        if let Some(engine) = self.engine.as_mut() { engine.stop(); }
        cx.notify();
    }

    /// Stop when the thread being read is no longer the one open.
    pub fn thread_changed(&mut self, open: Option<uuid::Uuid>, cx: &mut Context<Self>) {
        if self.playing.as_ref().is_some_and(|p| Some(p.thread) != open) { self.close(cx); }
    }

    // ── settings ──

    /// Look up the voices for the current list and search.
    pub fn refresh_voices(&mut self, cx: &mut Context<Self>) {
        let api_key = self.api_key.clone().unwrap_or_default();
        let (list, query) = (self.voice_list, self.query.trim().to_string());
        self.voices_loading = true;
        self.voices_error = None;
        if let Some(engine) = self.engine(cx) { engine.request_voices(&api_key, list, &query); }
        cx.notify();
    }

    pub fn set_voice_list(&mut self, list: VoiceList, cx: &mut Context<Self>) {
        if self.voice_list == list { return; }
        self.voice_list = list;
        self.voices.clear();
        self.refresh_voices(cx);
    }

    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if self.query == query { return; }
        self.query = query;
        self.refresh_voices(cx);
    }

    pub fn set_voice(&mut self, voice: &Voice, cx: &mut Context<Self>) {
        self.settings.voice = Some(voice.id.clone());
        self.settings.voice_name = Some(voice.name.clone());
        self.save(cx);
        cx.notify();
    }

    /// The chosen voice's id and name (the default until one is chosen).
    pub fn chosen_voice(&self) -> (String, String) {
        match (&self.settings.voice, &self.settings.voice_name) {
            (Some(id), name) => (id.clone(), name.clone().unwrap_or_else(|| id.clone())),
            (None, _) => (speech::DEFAULT_VOICE.into(), speech::DEFAULT_VOICE_NAME.into()),
        }
    }

    /// Preview a voice, or stop it when it's the one previewing.
    pub fn toggle_preview(&mut self, voice: &Voice, cx: &mut Context<Self>) {
        let api_key = self.api_key.clone().unwrap_or_default();
        let stop = self.previewing.as_deref() == Some(voice.id.as_str());
        // Show the change at once; the helper confirms it.
        self.previewing = (!stop).then(|| voice.id.clone());
        if let Some(engine) = self.engine(cx) {
            if stop { engine.stop_preview() } else { engine.preview(voice, &api_key) }
        }
        cx.notify();
    }

    /// Whether a key is being checked with Fish Audio.
    pub fn checking_key(&self) -> bool {
        self.checking_key.is_some()
    }

    /// Check the key with Fish Audio; it's saved only once Fish Audio accepts it.
    pub fn save_api_key(&mut self, key: String, cx: &mut Context<Self>) {
        let key = key.trim().to_string();
        if key.is_empty() || self.checking_key.is_some() { return; }
        self.key_error = None;
        let Some(engine) = self.engine(cx) else {
            self.key_error = Some("Read-aloud isn't part of this build.".into());
            cx.notify();
            return;
        };
        engine.check_key(&key);
        self.checking_key = Some(key);
        cx.notify();
    }

    fn store_api_key(&mut self, key: String, cx: &mut Context<Self>) {
        let state = services(cx);
        let this = cx.entity().downgrade();
        let saved = key.trim().to_string();
        spawn_service(cx, async move { bomb_core::services::speech_key::save(state.persistence.clone(), key).await }, move |res, cx| {
            let _ = this.update(cx, |m, cx| match res {
                Ok(()) => { m.api_key = Some(saved.clone()); m.clear_key_input = true; m.refresh_voices(cx); }
                Err(e) => { m.key_error = Some(e); cx.notify(); }
            });
        });
    }

    pub fn remove_api_key(&mut self, cx: &mut Context<Self>) {
        self.api_key = None;
        let state = services(cx);
        spawn_service(cx, async move { bomb_core::services::speech_key::remove(state.persistence.clone()).await }, |_, _| {});
        cx.notify();
    }

    pub fn reveal_playing(&mut self, cx: &mut Context<Self>) {
        self.reveal = self.playing.as_ref().map(|p| (p.thread, p.entry));
        cx.notify();
    }
}
