//! Reading replies aloud: which message is playing, where, how fast, in which voice. One message
//! plays at a time; starting another replaces it. Playback is this Mac's, so it lives here rather
//! than in bomb_core (which also runs on servers). See `crate::speech` for the engine.

use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::runtime::{services, spawn_service};
use crate::speech::{self, HelperEngine, SpeechEngine, SpeechEvent, SpeechStatus, Voice};

const SETTINGS_KEY: &str = "read_aloud";
/// The speeds the player offers.
pub const RATES: [f32; 5] = [0.75, 1.0, 1.25, 1.5, 2.0];

pub struct ReadAloudHandle(pub Entity<ReadAloud>);
impl Global for ReadAloudHandle {}

pub fn read_aloud(cx: &App) -> Entity<ReadAloud> {
    cx.global::<ReadAloudHandle>().0.clone()
}

/// Saved choices: the voice (none → the best installed) and the speed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadAloudSettings {
    pub voice: Option<String>,
    pub rate: f32,
}

impl Default for ReadAloudSettings {
    fn default() -> Self {
        Self { voice: None, rate: 1.0 }
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
    pub voices: Vec<Voice>,
    pub needs_better_voice: bool,
    best_voice: String,
    pub personal_voice: Option<bool>,
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
            let available = tokio::task::spawn_blocking(speech::available).await.unwrap_or(false);
            Ok::<_, String>((saved, available))
        }, move |res, cx| {
            let Ok((saved, available)) = res else { return };
            let _ = this.update(cx, |m, cx| {
                m.available = available;
                if let Some(settings) = saved.and_then(|raw| serde_json::from_str(&raw).ok()) { m.settings = settings; }
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
                    if this.update(cx, |m, cx| { m.on_event(event); cx.notify(); }).is_err() { break; }
                }
            })
            .detach();
        }
        self.engine.as_mut()
    }

    fn on_event(&mut self, event: SpeechEvent) {
        match event {
            SpeechEvent::Status(status) => {
                // A report about something no longer playing doesn't count.
                if self.playing.is_some() || status.key.is_empty() { self.status = status; }
            }
            SpeechEvent::Voices { voices, needs_better, best } => {
                self.voices = voices;
                self.needs_better_voice = needs_better;
                self.best_voice = best;
            }
            SpeechEvent::PersonalVoice(granted) => self.personal_voice = Some(granted),
        }
    }

    /// The voice to read with: the chosen one while it's installed, else the best.
    fn voice(&self) -> Option<String> {
        self.settings.voice.clone().filter(|v| self.voices.is_empty() || self.voices.iter().any(|x| &x.id == v))
            .or_else(|| Some(self.best_voice.clone()).filter(|b| !b.is_empty()))
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
        let key = speech::cache_key(&format!("{thread}-{entry}"), voice.as_deref().unwrap_or("default"), &sentences);
        self.playing = Some(Playing { thread, entry, title, sentences: sentences.clone() });
        self.status = SpeechStatus { key: key.clone(), playing: true, ..Default::default() };
        if let Some(engine) = self.engine(cx) {
            engine.request_voices();
            engine.play(&key, &sentences, voice.as_deref(), rate, 0.0);
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

    pub fn refresh_voices(&mut self, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine(cx) { engine.request_voices(); }
    }

    pub fn set_voice(&mut self, voice: String, cx: &mut Context<Self>) {
        self.settings.voice = Some(voice);
        self.save(cx);
        cx.notify();
    }

    pub fn chosen_voice(&self) -> Option<String> {
        self.voice()
    }

    pub fn preview(&mut self, voice: &str, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine(cx) { engine.preview(voice); }
    }

    pub fn request_personal_voice(&mut self, cx: &mut Context<Self>) {
        if let Some(engine) = self.engine(cx) { engine.request_personal_voice(); }
    }

    pub fn reveal_playing(&mut self, cx: &mut Context<Self>) {
        self.reveal = self.playing.as_ref().map(|p| (p.thread, p.entry));
        cx.notify();
    }
}
