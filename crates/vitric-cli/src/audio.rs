//! 音频输出 — 约定事件驱动：
//! - `play-sound`（data: {"sound": "coin.wav", "volume": 0.6}）播一次音效；
//! - `play-music`（data: {"sound": "bgm.ogg", "volume": 0.4}）循环播放背景音乐，
//!   全局只有一个音乐槽，新歌顶掉旧歌（旧的先停再放新的）；
//! - `stop-music`（data: {}）停掉当前背景音乐。
//!
//! volume 可选，0..=1，默认 1.0；越界/非数字是显式错误（结构化 stderr 行），不静默截断。
//!
//! 音频是纯输出副作用，不进模拟状态——确定性回放不受影响。
//! 无声卡环境（容器/CI/无头服务器）打不开设备是合法状态：
//! 启动横幅明说 audio disabled，事件照常流动只是没声。

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use vitric_rules::Event;

/// 从事件解析出的音频指令。纯数据不碰设备——解析/校验逻辑在无声卡环境也能测。
#[derive(Debug, Clone, PartialEq)]
pub enum SoundCmd {
    /// play-sound：播一次音效。
    Play { sound: String, volume: f32 },
    /// play-music：循环播放背景音乐（单槽位，新歌顶掉旧歌）。
    PlayMusic { sound: String, volume: f32 },
    /// stop-music：停掉当前背景音乐。
    StopMusic,
}

/// 解析音频约定事件。非音频事件返回 None；音频事件但参数不合法返回 Some(Err)。
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

/// volume 可选，默认 1.0；必须是 0..=1 的数字。越界/非数字显式报错，不静默 clamp。
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

/// 单一音乐槽：同一时刻最多一首背景音乐。换歌 = 先停旧的再放新的，所以
/// put/take 都把旧值交还调用方，由调用方负责 stop——槽本身只管"只有一个"。
pub(crate) struct MusicSlot<T> {
    current: Option<T>,
}

impl<T> MusicSlot<T> {
    pub(crate) fn empty() -> MusicSlot<T> {
        MusicSlot { current: None }
    }

    /// 放入新音乐，交还被顶掉的旧音乐（没有则 None）。
    pub(crate) fn put(&mut self, new: T) -> Option<T> {
        self.current.replace(new)
    }

    /// 取出当前音乐（用于 stop-music），槽变空。
    pub(crate) fn take(&mut self) -> Option<T> {
        self.current.take()
    }
}

pub struct Audio {
    device: rodio::MixerDeviceSink,
    sounds_dir: PathBuf,
    /// 文件名 -> 原始字节（解码每次播放时做，文件读取缓存住）。
    cache: HashMap<String, Arc<[u8]>>,
    /// 背景音乐槽：音乐要跨 tick 持续播，所以 Player 不 detach、存在这里。
    music: MusicSlot<rodio::Player>,
}

impl Audio {
    /// 打开默认音频设备。失败（无声卡）返回错误，调用方决定降级。
    pub fn open(sounds_dir: PathBuf) -> Result<Audio, String> {
        let device = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|e| format!("音频设备打开失败: {e}"))?;
        Ok(Audio { device, sounds_dir, cache: HashMap::new(), music: MusicSlot::empty() })
    }

    /// 读取（带缓存）一个音频文件的原始字节。
    /// 文件名来自事件 data（运行时可拼接），不许逃出 sounds/ 目录。
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

    /// 播放一个音效（项目 sounds/ 目录下的 wav/ogg/mp3/flac），volume 已校验过 0..=1。
    pub fn play(&mut self, name: &str, volume: f32) -> Result<(), String> {
        let bytes = self.load(name)?;
        let player = rodio::play(self.device.mixer(), Cursor::new(bytes.to_vec()))
            .map_err(|e| format!("音效 {name:?} 播放失败: {e}。支持 wav/ogg/mp3/flac"))?;
        player.set_volume(volume);
        player.detach(); // 播完自停，不跟随本帧生命周期
        Ok(())
    }

    /// 循环播放背景音乐。单槽位：先停掉旧的再起新的，不会两首叠着响。
    /// 解码先于换槽——新文件坏了旧音乐继续播，不留无声空槽。
    pub fn play_music(&mut self, name: &str, volume: f32) -> Result<(), String> {
        let bytes = self.load(name)?;
        // new_looped：解码器自带无限循环，播完一遍 seek 回头接着放
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

    /// 停掉当前背景音乐。没在播也合法（幂等）。
    pub fn stop_music(&mut self) {
        if let Some(old) = self.music.take() {
            old.stop();
        }
    }
}

/// 从一帧的事件流里挑出音频约定事件并执行；错误以结构化行上报 stderr（不崩游戏）。
/// audio 为 None（无声卡）时静默消费——事件照常流动只是没声。
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
        // 整数 0/1 也是合法数字
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
        // stop-music 不需要任何字段
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
        // 单槽语义：第一次放没有旧值；第二次放交还旧值（调用方负责 stop）
        let mut slot: MusicSlot<&str> = MusicSlot::empty();
        assert_eq!(slot.put("bgm1.ogg"), None);
        assert_eq!(slot.put("bgm2.ogg"), Some("bgm1.ogg"));
        // take 取出当前并清空，再 take 是 None（stop-music 幂等）
        assert_eq!(slot.take(), Some("bgm2.ogg"));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn handle_sound_events_without_device_consumes_gracefully() {
        // 无声卡（audio = None）时事件照常流动，坏数据也不崩
        let mut audio: Option<Audio> = None;
        let events = vec![
            ev("play-sound", json!({"sound": "x.wav", "volume": "loud"})),
            ev("play-music", json!({})),
            ev("stop-music", json!({})),
        ];
        handle_sound_events(&mut audio, &events);
    }
}
