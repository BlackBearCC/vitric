//! Audio output — convention-driven events:
//! - `play-sound` (data: {"sound": "coin.wav", "volume": 0.6}) plays a sound effect once;
//! - `play-music` (data: {"sound": "bgm.ogg", "volume": 0.4}) loops background music; there is a
//!   single global music slot, and a new track displaces the old one (the old one is stopped
//!   before the new one plays);
//! - `stop-music` (data: {}) stops the current background music.
//!
//! volume is optional, 0..=1, default 1.0; out-of-range / non-number is an explicit error (a
//! structured stderr line), not a silent clamp.
//!
//! Audio is a pure output side effect and does not enter simulation state — deterministic replay
//! is unaffected. Failing to open a device in a soundless environment (container / CI / headless
//! server) is a legal state: the startup banner says audio disabled, and events keep flowing
//! with no sound.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use vitric_rules::Event;

/// An audio command parsed from an event. Pure data, does not touch devices — the parse / verify
/// logic can be tested in a soundless environment.
#[derive(Debug, Clone, PartialEq)]
pub enum SoundCmd {
    /// play-sound: play a sound effect once.
    Play { sound: String, volume: f32 },
    /// play-music: loop background music (single slot, new track displaces the old one).
    PlayMusic { sound: String, volume: f32 },
    /// stop-music: stop the current background music.
    StopMusic,
}

/// Parse an audio convention event. Non-audio events return None; an audio event with invalid
/// parameters returns Some(Err).
pub fn parse_sound_cmd(event: &Event) -> Option<Result<SoundCmd, String>> {
    match event.name.as_str() {
        "stop-music" => return Some(Ok(SoundCmd::StopMusic)),
        "play-sound" | "play-music" => {}
        _ => return None,
    }
    let Some(sound) = event.data.get("sound").and_then(|v| v.as_str()) else {
        return Some(Err(format!("{} 事件缺少 sound 字段（文本）", event.name)));
    };
    let volume = match parse_volume(&event.name, sound, event.data.get("volume")) {
        Ok(v) => v,
        Err(e) => return Some(Err(e)),
    };
    let sound = sound.to_string();
    Some(Ok(match event.name.as_str() {
        "play-sound" => SoundCmd::Play { sound, volume },
        _ => SoundCmd::PlayMusic { sound, volume },
    }))
}

/// volume is optional, default 1.0; must be a number in 0..=1. Out-of-range / non-number
/// explicitly errors, no silent clamp.
fn parse_volume(event: &str, sound: &str, v: Option<&Value>) -> Result<f32, String> {
    let Some(v) = v else { return Ok(1.0) };
    let Some(n) = v.as_f64() else {
        return Err(format!("{event} 事件（sound: {sound:?}）的 volume 必须是数字，收到 {v}"));
    };
    if !(0.0..=1.0).contains(&n) {
        return Err(format!(
            "{event} 事件（sound: {sound:?}）的 volume 必须在 0..=1 范围内，收到 {n}"
        ));
    }
    Ok(n as f32)
}

/// Single music slot: at most one background music track at a time. Switching tracks = stop the
/// old one then play the new one, so put/take both hand the old value back to the caller, who is
/// responsible for stop — the slot itself only enforces "there is only one".
pub(crate) struct MusicSlot<T> {
    current: Option<T>,
}

impl<T> MusicSlot<T> {
    pub(crate) fn empty() -> MusicSlot<T> {
        MusicSlot { current: None }
    }

    /// Put in new music, returning the displaced old music (None if there was none).
    pub(crate) fn put(&mut self, new: T) -> Option<T> {
        self.current.replace(new)
    }

    /// Take out the current music (for stop-music); the slot becomes empty.
    pub(crate) fn take(&mut self) -> Option<T> {
        self.current.take()
    }
}

pub struct Audio {
    device: rodio::MixerDeviceSink,
    sounds_dir: PathBuf,
    /// Filename -> raw bytes (decoding happens on each play; file reads are cached).
    cache: HashMap<String, Arc<[u8]>>,
    /// Background music slot: music must keep playing across ticks, so the Player is not detached
    /// and is held here.
    music: MusicSlot<rodio::Player>,
}

impl Audio {
    /// Open the default audio device. On failure (no sound card) returns an error; the caller
    /// decides whether to degrade.
    pub fn open(sounds_dir: PathBuf) -> Result<Audio, String> {
        let device = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|e| format!("音频设备打开失败: {e}"))?;
        Ok(Audio { device, sounds_dir, cache: HashMap::new(), music: MusicSlot::empty() })
    }

    /// Read (with caching) the raw bytes of an audio file. The filename comes from event data
    /// (may be assembled at runtime) and must not escape the sounds/ directory.
    fn load(&mut self, name: &str) -> Result<Arc<[u8]>, String> {
        if name.contains("..") || name.starts_with('/') || name.contains('\\') {
            return Err(format!("音效名 {name:?} 不合法：只能是 sounds/ 目录内的相对文件名"));
        }
        if let Some(b) = self.cache.get(name) {
            return Ok(b.clone());
        }
        let path = self.sounds_dir.join(name);
        let data = std::fs::read(&path).map_err(|e| {
            format!(
                "音效 {name:?} 读取失败: {e}。提示：音效放项目 sounds/ 目录，\
                 事件写法 {{\"emit\": \"play-sound\", \"data\": {{\"sound\": \"{name}\"}}}}"
            )
        })?;
        let arc: Arc<[u8]> = data.into();
        self.cache.insert(name.to_string(), arc.clone());
        Ok(arc)
    }

    /// Play a sound effect (a wav/ogg/mp3/flac under the project's sounds/ directory); volume is
    /// already verified to be 0..=1.
    pub fn play(&mut self, name: &str, volume: f32) -> Result<(), String> {
        let bytes = self.load(name)?;
        let player = rodio::play(self.device.mixer(), Cursor::new(bytes.to_vec()))
            .map_err(|e| format!("音效 {name:?} 播放失败: {e}。支持 wav/ogg/mp3/flac"))?;
        player.set_volume(volume);
        player.detach(); // stops on its own when done; does not follow this frame's lifetime
        Ok(())
    }

    /// Loop background music. Single slot: the old one is stopped before the new one starts, so
    /// two tracks never overlap. Decoding happens before swapping the slot — if the new file is
    /// broken the old music keeps playing, leaving no silent empty slot.
    pub fn play_music(&mut self, name: &str, volume: f32) -> Result<(), String> {
        let bytes = self.load(name)?;
        // new_looped: the decoder has a built-in infinite loop; after one pass it seeks back and
        // keeps playing
        let source = rodio::Decoder::new_looped(Cursor::new(bytes.to_vec()))
            .map_err(|e| format!("音乐 {name:?} 解码失败: {e}。支持 wav/ogg/mp3/flac"))?;
        if let Some(old) = self.music.take() {
            old.stop();
        }
        let player = rodio::Player::connect_new(self.device.mixer());
        player.set_volume(volume);
        player.append(source);
        self.music.put(player);
        Ok(())
    }

    /// Stop the current background music. Legal even when nothing is playing (idempotent).
    pub fn stop_music(&mut self) {
        if let Some(old) = self.music.take() {
            old.stop();
        }
    }
}

/// Pick out audio convention events from a frame's event stream and execute them; errors are
/// reported to stderr as structured lines (do not crash the game). When audio is None (no sound
/// card) they are silently consumed — events keep flowing with no sound.
pub fn handle_sound_events(audio: &mut Option<Audio>, events: &[Event]) {
    let Some(audio) = audio else { return };
    for e in events {
        let Some(cmd) = parse_sound_cmd(e) else { continue };
        let result = match cmd {
            Ok(SoundCmd::Play { sound, volume }) => audio.play(&sound, volume),
            Ok(SoundCmd::PlayMusic { sound, volume }) => audio.play_music(&sound, volume),
            Ok(SoundCmd::StopMusic) => {
                audio.stop_music();
                Ok(())
            }
            Err(err) => Err(err),
        };
        if let Err(err) = result {
            eprintln!("{}", serde_json::json!({"audio_error": err}));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(name: &str, data: Value) -> Event {
        Event::new(name, data)
    }

    #[test]
    fn parse_play_sound_default_volume() {
        let cmd = parse_sound_cmd(&ev("play-sound", json!({"sound": "coin.wav"})));
        assert_eq!(
            cmd,
            Some(Ok(SoundCmd::Play { sound: "coin.wav".into(), volume: 1.0 }))
        );
    }

    #[test]
    fn parse_play_sound_with_volume() {
        let cmd = parse_sound_cmd(&ev("play-sound", json!({"sound": "x.wav", "volume": 0.6})));
        assert_eq!(cmd, Some(Ok(SoundCmd::Play { sound: "x.wav".into(), volume: 0.6 })));
        // Integer 0/1 is also a legal number
        let cmd = parse_sound_cmd(&ev("play-sound", json!({"sound": "x.wav", "volume": 0})));
        assert_eq!(cmd, Some(Ok(SoundCmd::Play { sound: "x.wav".into(), volume: 0.0 })));
    }

    #[test]
    fn parse_volume_out_of_range_is_error() {
        for bad in [json!(1.5), json!(-0.1), json!(2)] {
            let cmd =
                parse_sound_cmd(&ev("play-sound", json!({"sound": "x.wav", "volume": bad})));
            let err = cmd.unwrap().unwrap_err();
            assert!(err.contains("0..=1"), "错误信息要点明范围: {err}");
            assert!(err.contains("x.wav"), "错误信息要带上音效名: {err}");
        }
    }

    #[test]
    fn parse_volume_non_number_is_error() {
        for bad in [json!("0.5"), json!(true), json!(null), json!([0.5])] {
            let cmd =
                parse_sound_cmd(&ev("play-music", json!({"sound": "bgm.ogg", "volume": bad})));
            let err = cmd.unwrap().unwrap_err();
            assert!(err.contains("必须是数字"), "非数字要显式报错: {err}");
        }
    }

    #[test]
    fn parse_missing_sound_field_is_error() {
        let err = parse_sound_cmd(&ev("play-sound", json!({}))).unwrap().unwrap_err();
        assert!(err.contains("play-sound") && err.contains("sound 字段"));
        let err = parse_sound_cmd(&ev("play-music", json!({"volume": 0.4})))
            .unwrap()
            .unwrap_err();
        assert!(err.contains("play-music") && err.contains("sound 字段"));
    }

    #[test]
    fn parse_play_music_and_stop_music() {
        let cmd = parse_sound_cmd(&ev("play-music", json!({"sound": "bgm.ogg", "volume": 0.4})));
        assert_eq!(
            cmd,
            Some(Ok(SoundCmd::PlayMusic { sound: "bgm.ogg".into(), volume: 0.4 }))
        );
        // stop-music needs no fields
        assert_eq!(
            parse_sound_cmd(&ev("stop-music", json!({}))),
            Some(Ok(SoundCmd::StopMusic))
        );
    }

    #[test]
    fn parse_ignores_non_audio_events() {
        assert_eq!(parse_sound_cmd(&ev("collision", json!({"a": "1", "b": "2"}))), None);
        assert_eq!(parse_sound_cmd(&ev("input", json!({"action": "jump"}))), None);
    }

    #[test]
    fn music_slot_replaces_and_returns_old() {
        // Single-slot semantics: first put has no old value; second put returns the old value
        // (caller is responsible for stop)
        let mut slot: MusicSlot<&str> = MusicSlot::empty();
        assert_eq!(slot.put("bgm1.ogg"), None);
        assert_eq!(slot.put("bgm2.ogg"), Some("bgm1.ogg"));
        // take removes the current and empties the slot; a second take is None (stop-music idempotent)
        assert_eq!(slot.take(), Some("bgm2.ogg"));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn handle_sound_events_without_device_consumes_gracefully() {
        // With no sound card (audio = None) events keep flowing, and bad data does not crash
        let mut audio: Option<Audio> = None;
        let events = vec![
            ev("play-sound", json!({"sound": "x.wav", "volume": "loud"})),
            ev("play-music", json!({})),
            ev("stop-music", json!({})),
        ];
        handle_sound_events(&mut audio, &events);
    }
}
