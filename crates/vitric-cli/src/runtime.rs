//! Runtime — 把项目数据、规则引擎、脚本引擎装配成一台能跑的游戏。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use vitric_data::{Clip, Project};
use vitric_ecs::World;
use vitric_rules::{Engine, Event, RuleSet, ScriptCall};
use vitric_script::ScriptEngine;
use vitric_sim::{GameLogic, Pcg32, Sim};

/// 游戏逻辑装配体：规则为正门，脚本兜复杂逻辑。
///
/// 每 tick 的执行顺序（固定，确定性的一部分）：
/// 1. 规则消化本 tick 事件（输入/碰撞/上一 tick 脚本发的事件）；
/// 2. 规则产生的 `call` 逐个调脚本函数；
/// 3. 脚本系统按注册顺序各跑一遍；
/// 4. 脚本 emit 的事件进下一 tick 的收件箱。
pub struct Runtime {
    pub rules: Engine,
    pub scripts: ScriptEngine,
    /// 动画片段定义。
    pub animations: BTreeMap<String, Clip>,
    /// 项目根目录（热重载从这里重读磁盘）。
    root: Option<std::path::PathBuf>,
    /// 脚本上一 tick 发出的事件，本 tick 交给规则。
    carryover: Vec<Event>,
    /// 本 tick 规则/脚本 emit 的全部事件副本，主循环取走送进控制面事件日志。
    observed: Vec<Event>,
}

impl Runtime {
    /// 从已加载的项目装配运行时（规则语义校验、脚本求值都在这里发生）。
    pub fn build(project: &Project) -> Result<Runtime, String> {
        // 规则：多个文件合并成一个规则集
        let mut all = RuleSet::default();
        for (file, doc) in &project.rules {
            let set = RuleSet::parse(doc, file).map_err(|r| r.to_string())?;
            all.rules.extend(set.rules);
        }
        let rules = Engine::new(all, project.schema.clone());

        // 脚本（.ts 经 esbuild 转译成 JS 再进 QuickJS）
        let mut scripts = ScriptEngine::new(project.schema.clone()).map_err(|e| e.to_string())?;
        for (file, src) in &project.scripts {
            let js;
            let source = if file.ends_with(".ts") {
                js = transpile_ts(file, src)?;
                &js
            } else {
                src
            };
            scripts.load(file, source).map_err(|e| e.to_string())?;
        }

        Ok(Runtime {
            rules,
            scripts,
            animations: project.animations.clone(),
            root: None,
            carryover: Vec::new(),
            observed: Vec::new(),
        })
    }

    /// 加载项目 + 装配 + 实例化入口场景，给出可以直接跑的 (Sim, Runtime)。
    pub fn boot(dir: &Path) -> Result<(Sim, Runtime), String> {
        let project = Project::load(dir).map_err(|r| r.to_string())?;
        let mut runtime = Runtime::build(&project)?;
        runtime.root = Some(dir.to_path_buf());
        let mut sim = Sim::new(project.manifest.seed);
        vitric_data::instantiate_scene(project.entry_scene(), &project.schema, &mut sim.world)
            .map_err(|r| r.to_string())?;
        Ok((sim, runtime))
    }
}

impl GameLogic for Runtime {
    fn on_tick(
        &mut self,
        world: &mut vitric_ecs::World,
        events: Vec<Event>,
        rng: &mut Pcg32,
        tick: u64,
    ) -> Result<(), String> {
        let mut inbox = std::mem::take(&mut self.carryover);
        inbox.extend(events);

        // 1. 规则
        let out = self.rules.process_tick(world, inbox).map_err(|e| e.to_string())?;
        self.observed.extend(out.emitted);

        // 2. 规则 -> 脚本函数调用
        for ScriptCall { function, args, self_entity } in out.calls {
            let so = self
                .scripts
                .call_fn(&function, &args, self_entity, world, rng, tick)
                .map_err(|e| e.to_string())?;
            self.observed.extend(so.events.iter().cloned());
            self.carryover.extend(so.events);
        }

        // 3. 脚本系统
        let so = self.scripts.run_systems(world, rng, tick).map_err(|e| e.to_string())?;
        self.observed.extend(so.events.iter().cloned());
        self.carryover.extend(so.events);

        // 4. 动画推帧（引擎独占 Sprite.image 的写权——动画永远不会被别的逻辑"打断"，
        //    想换动画只有一条正路：改 Anim.clip）
        let anim_events = advance_animations(world, &self.animations)?;
        self.observed.extend(anim_events.iter().cloned());
        self.carryover.extend(anim_events);

        Ok(())
    }

    /// 取走本 tick 规则/脚本发出的事件（控制面观测用）。
    fn drain_observed(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.observed)
    }

    /// 热重载：从磁盘重读规则+脚本，整体重建后原子替换；
    /// 任何一步失败都保持旧逻辑不动（不会半死不活）。
    /// 注意：schema/场景改动不在热重载范围（它们定义世界形状，改了要重启）。
    fn reload(&mut self) -> Result<serde_json::Value, String> {
        let root = self.root.clone().ok_or("该运行时没有项目目录，无法热重载")?;
        let project = Project::load(&root).map_err(|r| r.to_string())?;
        let fresh = Runtime::build(&project)?;
        self.rules = fresh.rules;
        self.scripts = fresh.scripts;
        // carryover 里是纯数据事件，跨重载安全，保留不丢
        Ok(serde_json::json!({
            "reloaded": ["rules", "scripts"],
            "note": "schema/场景的改动不走热重载，需要重启进程",
            "rules": self.rules.rules.rules.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            "systems": self.scripts.systems.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            "fns": self.scripts.fns.clone(),
        }))
    }

    /// carryover（脚本上一 tick 发的、还没进规则的事件）是跨 tick 状态，
    /// 不进快照的话 restore 后第一个 tick 的事件流就和原轨迹不一样了。
    fn snapshot_state(&self) -> Value {
        json!({
            "carryover": self
                .carryover
                .iter()
                .map(|e| json!({"name": e.name, "data": e.data}))
                .collect::<Vec<_>>(),
        })
    }

    fn restore_state(&mut self, snap: &Value) -> Result<(), String> {
        let items = snap
            .get("carryover")
            .and_then(|v| v.as_array())
            .ok_or("快照的 logic 状态缺 carryover（旧版快照与当前版本不兼容，重新 sim/snapshot）")?;
        let mut carryover = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("carryover[{i}] 缺 name"))?;
            let data = item.get("data").cloned().unwrap_or(json!({}));
            carryover.push(Event::new(name, data));
        }
        self.carryover = carryover;
        self.observed.clear();
        Ok(())
    }
}

/// 动画系统：每 tick 推帧。状态全在 Anim 组件里（快照/回放安全）：
/// `clip` 当前片段（空串=不播）、`prev` 引擎用来检测切换、`t` 片段内 tick 数、
/// `done` 非循环片段是否已播完（播完那一刻发一次 `anim-finished` 事件）。
pub fn advance_animations(
    world: &mut World,
    clips: &BTreeMap<String, Clip>,
) -> Result<Vec<Event>, String> {
    let mut events = Vec::new();
    for id in world.query(&["Anim", "Sprite"]) {
        let clip_name = world
            .get_field(id, "Anim.clip")
            .map_err(|e| e.to_string())?
            .as_str()
            .ok_or_else(|| format!("实体 {id} 的 Anim.clip 必须是文本"))?
            .to_string();
        if clip_name.is_empty() {
            continue; // 空串 = 不播动画，Sprite.image 归还给用户
        }
        let clip = clips.get(&clip_name).ok_or_else(|| {
            format!(
                "实体 {id} 的 Anim.clip {clip_name:?} 没有定义。已定义片段: [{}]。\
                 提示：片段在 animations 文件的 clips 里定义",
                clips.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;
        let prev = world
            .get_field(id, "Anim.prev")
            .map_err(|e| e.to_string())?
            .as_str()
            .unwrap_or("")
            .to_string();
        let mut t = world
            .get_field(id, "Anim.t")
            .map_err(|e| e.to_string())?
            .as_i64()
            .ok_or_else(|| format!("实体 {id} 的 Anim.t 必须是整数"))?;
        let mut done = world
            .get_field(id, "Anim.done")
            .map_err(|e| e.to_string())?
            .as_bool()
            .unwrap_or(false);

        if clip_name != prev {
            // 切换片段：从头播
            t = 0;
            done = false;
            world.set_field(id, "Anim.prev", json!(clip_name)).map_err(|e| e.to_string())?;
        } else {
            t += 1;
        }

        // 整数运算保确定性：第 t tick 对应第 t*fps/60 帧
        let raw = (t as u64 * clip.fps as u64 / vitric_sim::TICKS_PER_SECOND) as usize;
        let idx = if clip.looping {
            raw % clip.frames.len()
        } else {
            if raw >= clip.frames.len() && !done {
                done = true;
                events.push(Event::new(
                    "anim-finished",
                    json!({"entity": id.to_string(), "clip": clip_name}),
                ));
            }
            raw.min(clip.frames.len() - 1)
        };

        world.set_field(id, "Anim.t", json!(t)).map_err(|e| e.to_string())?;
        world.set_field(id, "Anim.done", json!(done)).map_err(|e| e.to_string())?;
        world
            .set_field(id, "Sprite.image", json!(clip.frames[idx]))
            .map_err(|e| e.to_string())?;
    }
    Ok(events)
}

/// TypeScript → JavaScript（esbuild 子进程，只剥类型不打包）。
/// 找 esbuild 的顺序：环境变量 ESBUILD_BIN → PATH 上的 esbuild。
fn transpile_ts(file: &str, src: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let bin = std::env::var("ESBUILD_BIN").unwrap_or_else(|_| "esbuild".to_string());
    let mut child = Command::new(&bin)
        .args(["--loader=ts"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "{file} 是 TypeScript，需要 esbuild 转译，但启动 {bin:?} 失败: {e}。\
                 提示：npm i -g esbuild，或设环境变量 ESBUILD_BIN 指向 esbuild 二进制；\
                 不想装就把脚本写成 .js"
            )
        })?;
    child
        .stdin
        .take()
        .expect("piped")
        .write_all(src.as_bytes())
        .map_err(|e| format!("{file}: 喂给 esbuild 失败: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("{file}: esbuild 执行失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{file} TypeScript 转译失败:\n{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 递归扫描规则文档里 `{"emit": "play-sound"|"play-music", "data": {"sound": "字面量"}}`
/// 的音效/音乐引用（两者共用同一套字面名规则，含路径逃逸校验）。
fn scan_sound_refs(doc: &Value, file: &str, sounds_dir: &Path, missing: &mut Vec<String>) {
    match doc {
        Value::Object(map) => {
            if matches!(
                map.get("emit").and_then(|v| v.as_str()),
                Some("play-sound" | "play-music")
            ) {
                if let Some(sound) = map
                    .get("data")
                    .and_then(|d| d.get("sound"))
                    .and_then(|s| s.as_str())
                {
                    // 引用值也可能是 "event.xxx" 这类运行时路径，只校验字面文件名
                    let is_ref = sound.starts_with("self.")
                        || sound.starts_with("other.")
                        || sound.starts_with("event.")
                        || sound.starts_with('@');
                    if !is_ref {
                        // 与运行时同一条规则：不许逃出 sounds/ 目录
                        if sound.contains("..") || sound.starts_with('/') || sound.contains('\\') {
                            missing.push(format!(
                                "{file} 的音效名 {sound:?} 不合法：只能是 sounds/ 目录内的相对文件名"
                            ));
                        } else if !sounds_dir.join(sound).exists() {
                            missing.push(format!(
                                "{file} 引用了不存在的音效 {sound:?}（应在项目 sounds/ 目录）"
                            ));
                        }
                    }
                }
            }
            for v in map.values() {
                scan_sound_refs(v, file, sounds_dir, missing);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                scan_sound_refs(v, file, sounds_dir, missing);
            }
        }
        _ => {}
    }
}

/// `vitric check`：只验数据不开跑。返回人类/AI 可读的完整报告。
pub fn check(dir: &Path) -> Result<Value, String> {
    let project = Project::load(dir).map_err(|r| r.to_string())?;
    let runtime = Runtime::build(&project)?;
    // 实例化入口场景到一次性世界，把落地期错误也暴露出来
    let mut sim = Sim::new(project.manifest.seed);
    vitric_data::instantiate_scene(project.entry_scene(), &project.schema, &mut sim.world)
        .map_err(|r| r.to_string())?;
    // 素材：加载即校验（坏图/超尺寸），再查场景引用的图都在
    let assets = vitric_render::Assets::load_dir(&dir.join("assets"))?;
    let mut missing = Vec::new();
    for id in sim.world.query(&["Sprite"]) {
        if let Ok(image) = sim.world.get_field(id, "Sprite.image") {
            if let Some(name) = image.as_str().filter(|s| !s.is_empty()) {
                if assets.image(name).is_none() {
                    missing.push(format!(
                        "实体 {}{} 引用了不存在的素材 {name:?}",
                        id,
                        sim.world.name_of(id).map(|n| format!("({n})")).unwrap_or_default(),
                    ));
                }
            }
        }
    }
    // 动画：clip 引用的帧图都在素材库、场景里 Anim.clip 都已定义
    for (cname, clip) in &project.animations {
        for frame in &clip.frames {
            if assets.image(frame).is_none() {
                missing.push(format!("动画片段 {cname:?} 引用了不存在的帧图 {frame:?}"));
            }
        }
    }
    for id in sim.world.query(&["Anim"]) {
        if let Ok(clip) = sim.world.get_field(id, "Anim.clip") {
            if let Some(name) = clip.as_str().filter(|s| !s.is_empty()) {
                if !project.animations.contains_key(name) {
                    missing.push(format!(
                        "实体 {}{} 的 Anim.clip 引用了未定义的片段 {name:?}（已定义: [{}]）",
                        id,
                        sim.world.name_of(id).map(|n| format!("({n})")).unwrap_or_default(),
                        project.animations.keys().cloned().collect::<Vec<_>>().join(", "),
                    ));
                }
            }
        }
    }
    // 音效：规则里字面引用的 play-sound 音效文件必须存在
    for (file, doc) in &project.rules {
        scan_sound_refs(doc, file, &dir.join("sounds"), &mut missing);
    }
    if !missing.is_empty() {
        return Err(format!(
            "素材/动画/音效引用校验失败:\n  {}\n现有素材: [{}]",
            missing.join("\n  "),
            assets.names().join(", ")
        ));
    }
    Ok(serde_json::json!({
        "project": project.manifest.name,
        "scenes": project.scenes.keys().collect::<Vec<_>>(),
        "rules": runtime.rules.rules.rules.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        "systems": runtime.scripts.systems.iter().map(|s| serde_json::json!({
            "name": s.name, "query": s.query, "writes": s.writes,
        })).collect::<Vec<_>>(),
        "fns": runtime.scripts.fns,
        "entities": sim.world.entities().len(),
        "assets": {
            "count": assets.count(),
            "decoded_kb": assets.total_bytes() / 1024,
        },
        "initial_hash": format!("{:#018x}", sim.world.state_hash()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scan(doc: Value) -> Vec<String> {
        let mut missing = Vec::new();
        // 指向一个肯定不存在的目录：所有字面引用都应报"不存在"
        scan_sound_refs(&doc, "rules/test.json", Path::new("/nonexistent/sounds"), &mut missing);
        missing
    }

    #[test]
    fn scan_flags_missing_play_music_file() {
        let missing = scan(json!({"then": [{"emit": "play-music", "data": {"sound": "bgm.ogg"}}]}));
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("bgm.ogg"), "报错要带上文件名: {}", missing[0]);
        assert!(missing[0].contains("rules/test.json"), "报错要带上来源文件: {}", missing[0]);
    }

    #[test]
    fn scan_flags_path_traversal_in_play_music() {
        // 路径逃逸是显式"不合法"错误，不是"文件不存在"
        let missing =
            scan(json!({"emit": "play-music", "data": {"sound": "../secret.ogg"}}));
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("不合法"), "要点明不合法而非不存在: {}", missing[0]);
    }

    #[test]
    fn scan_still_covers_play_sound_and_skips_runtime_refs() {
        // play-sound 老规则照旧；运行时引用（event.* 等）不做静态校验
        let missing = scan(json!([
            {"emit": "play-sound", "data": {"sound": "coin.wav"}},
            {"emit": "play-music", "data": {"sound": "event.bgm"}},
            {"emit": "stop-music", "data": {}},
        ]));
        assert_eq!(missing.len(), 1, "只有 coin.wav 该被报: {missing:?}");
        assert!(missing[0].contains("coin.wav"));
    }
}
