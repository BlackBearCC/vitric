//! 音频输出 — 约定事件驱动：规则/脚本 emit `play-sound` 事件
//! （data: {"sound": "coin.wav"}），这里消费并播放。
//!
//! 音频是纯输出副作用，不进模拟状态——确定性回放不受影响。
//! 无声卡环境（容器/CI/无头服务器）打不开设备是合法状态：
//! 启动横幅明说 audio disabled，事件照常流动只是没声。

use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use vitric_rules::Event;

pub struct Audio {
    device: rodio::MixerDeviceSink,
    sounds_dir: PathBuf,
    /// 文件名 -> 原始字节（解码每次播放时做，文件读取缓存住）。
    cache: HashMap<String, Arc<[u8]>>,
}

impl Audio {
    /// 打开默认音频设备。失败（无声卡）返回错误，调用方决定降级。
    pub fn open(sounds_dir: PathBuf) -> Result<Audio, String> {
        let device = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|e| format!("音频设备打开失败: {e}"))?;
        Ok(Audio { device, sounds_dir, cache: HashMap::new() })
    }

    /// 播放一个音效（项目 sounds/ 目录下的 wav/ogg/mp3/flac）。
    pub fn play(&mut self, name: &str) -> Result<(), String> {
        let bytes = match self.cache.get(name) {
            Some(b) => b.clone(),
            None => {
                let path = self.sounds_dir.join(name);
                let data = std::fs::read(&path).map_err(|e| {
                    format!(
                        "音效 {name:?} 读取失败: {e}。提示：音效放项目 sounds/ 目录，\
                         事件写法 {{\"emit\": \"play-sound\", \"data\": {{\"sound\": \"{name}\"}}}}"
                    )
                })?;
                let arc: Arc<[u8]> = data.into();
                self.cache.insert(name.to_string(), arc.clone());
                arc
            }
        };
        let player = rodio::play(self.device.mixer(), Cursor::new(bytes.to_vec()))
            .map_err(|e| format!("音效 {name:?} 播放失败: {e}。支持 wav/ogg/mp3/flac"))?;
        player.detach(); // 播完自停，不跟随本帧生命周期
        Ok(())
    }
}

/// 从一帧的事件流里挑出 play-sound 并播放；播放错误以结构化行上报 stderr（不崩游戏）。
pub fn handle_sound_events(audio: &mut Option<Audio>, events: &[Event]) {
    let Some(audio) = audio else { return };
    for e in events {
        if e.name != "play-sound" {
            continue;
        }
        let Some(sound) = e.data.get("sound").and_then(|v| v.as_str()) else {
            eprintln!(
                "{}",
                serde_json::json!({"audio_error": "play-sound 事件缺少 sound 字段（文本）"})
            );
            continue;
        };
        if let Err(err) = audio.play(sound) {
            eprintln!("{}", serde_json::json!({"audio_error": err}));
        }
    }
}
