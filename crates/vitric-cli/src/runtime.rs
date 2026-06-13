//! Runtime — 把项目数据、规则引擎、脚本引擎装配成一台能跑的游戏。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use vitric_data::{Clip, Project, Scene, Schema, SeqStep, Sequence};
use vitric_ecs::{EntityId, World};
use vitric_rules::{Engine, Event, RuleSet, ScriptCall};
use vitric_script::ScriptEngine;
use vitric_sim::{GameLogic, Pcg32, Sim};

/// UI 布局在模拟状态里参照的视口尺寸（像素）。布局是相对比例 + 像素偏移，
/// 解算结果写进组件（进哈希进存档）必须与具体窗口分辨率解耦——否则同一局在
/// 不同分辨率机器上状态哈希分歧。约定 1920×1080：跨机器确定。渲染时 CPU/GPU
/// 各自用真实窗口分辨率重解算（同一份 solve_layout 纯函数），UI 锚定视口自然缩放。
pub const UI_REFERENCE_VIEWPORT: (u32, u32) = (1920, 1080);

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
    /// 序列（时间轴）静态轨道定义。运行时 Sequence 组件按名字引用，
    /// 静态轨道不进每实例快照（组件只存最小播放状态）。
    pub sequences: BTreeMap<String, Sequence>,
    /// 主题（样式卷）。装配期常量，不进世界状态；UI 控件按名字引用取样式。
    /// 每 tick 把 Button.state 对应的主题底色解算进 Panel.color（render 读 Panel.color，
    /// 渲染层不依赖主题表——code 只递交数据）。
    pub themes: BTreeMap<String, vitric_data::Theme>,
    /// schema（场景切换时实例化新场景用，与规则/脚本持有的是同一份定义）。
    schema: Schema,
    /// 清单里的全部场景（装配期预载的不可变副本）。场景切换从这里取数据
    /// 而不是切换时读磁盘——运行中磁盘上的场景文件被改了也不影响本进程的
    /// 切换结果，重放和原局加载的是同一份内存数据，确定性不被热编辑破坏。
    scenes: BTreeMap<String, Scene>,
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
            sequences: project.sequences.clone(),
            themes: project.themes.clone(),
            schema: project.schema.clone(),
            scenes: project.scenes.clone(),
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

    /// 场景切换（约定事件 `load-scene {"scene": "scenes/xxx.json"}` 的执行端）。
    ///
    /// 时机约束：切换发生在 on_tick 尾部、仍在 sim.step 的确定性流水线之内——
    /// 触发它的 load-scene 事件本身由规则/脚本确定性地产生，所以重放同一份
    /// 录像会在同一 tick 走到这里、装出同一个世界，校验点哈希照常对得上。
    ///
    /// 语义：默认整个世界推倒重来（clear_entities 走正规 despawn，旧句柄全部
    /// 失效）；想跨场景活下来的实体挂 `Persist` 标记组件——它的全部组件被
    /// 原样搬进新世界（同名重建，槽位序）。新场景的初始化钩子是下一 tick 的
    /// `scene-loaded` 事件；`start` 只在整局的 tick 0 发一次，不会重发。
    fn switch_scene(&mut self, world: &mut World, scene_rel: &str) -> Result<(), String> {
        let scene = self.scenes.get(scene_rel).ok_or_else(|| {
            format!(
                "load-scene 引用的场景 {scene_rel:?} 不在清单 scenes 列表里。\
                 可用场景: [{}]。提示：新场景文件要先加进 vitric.json 的 scenes 数组",
                self.scenes.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;

        // Persist 幸存者：先拍下（名字 + 全部组件），错误都在动世界之前暴露
        let mut survivors: Vec<(String, Vec<(String, Value)>)> = Vec::new();
        for id in world.query(&["Persist"]) {
            let name = world.name_of(id).map(String::from).ok_or_else(|| {
                format!(
                    "实体 {id} 挂了 Persist 但没有名字。跨场景幸存的实体必须命名——\
                     没有名字，新场景里的规则就没有办法引用它"
                )
            })?;
            let comps = world
                .components_of(id)
                .into_iter()
                .map(|c| {
                    let v = world.get_component(id, &c).expect("components_of 列出").clone();
                    (c, v)
                })
                .collect();
            survivors.push((name, comps));
        }

        // 推倒 → 重建。注意 carryover 里本 tick 已发出的事件（含动画事件）不清：
        // 事件是纯数据，照常送达下一 tick——和"脚本 emit 的事件跨 tick 送达"同一条约定。
        world.clear_entities();
        vitric_data::instantiate_scene(scene, &self.schema, world)
            .map_err(|r| format!("切换到场景 {scene_rel:?} 失败:\n{r}"))?;

        for (name, comps) in survivors {
            let id = world.spawn_named(&name).map_err(|e| {
                format!(
                    "Persist 实体 {name:?} 无法进入场景 {scene_rel:?}: {e}。\
                     提示：要携带跨场景的实体，名字不能和目标场景里的实体重名——\
                     要么改 Persist 实体的名字，要么从目标场景里删掉同名实体"
                )
            })?;
            for (c, v) in comps {
                world.set_component(id, &c, v).expect("实体刚创建必然存活");
            }
        }

        // 新场景的"start"：下一 tick 的 scene-loaded 事件（进 observed 让控制面可见）
        let loaded = Event::new("scene-loaded", json!({"scene": scene_rel}));
        self.observed.push(loaded.clone());
        self.carryover.push(loaded);
        Ok(())
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
        // 序列 wait barrier 要看本 tick 的事件：input（含 skip 跳过）、上一 tick 脚本
        // 发的 carryover、以及本 tick 规则/脚本 emit 的命名事件（player-confirm 这类
        // 放行 barrier 的就是它们）。规则 emit 的事件在 process_tick 的级联里消化、
        // 不进 carryover，序列得在这里**额外**收一份本 tick 全部 emit 的事件副本。
        let mut seq_inbox = inbox.clone();

        // 本 tick 规则/脚本 emit 的 load-scene（场景切换约定事件）。切换推迟到
        // 流水线尾部统一执行——发起方 emit 之后本 tick 的剩余逻辑仍面对旧世界，
        // 时序清楚；只看"自己发的"事件，外部注入想换场景也得走规则这道正门。
        let mut loads: Vec<Event> = Vec::new();
        let collect_loads = |events: &[Event], loads: &mut Vec<Event>| {
            loads.extend(events.iter().filter(|e| e.name == "load-scene").cloned());
        };

        // 1. 规则
        let out = self.rules.process_tick(world, inbox).map_err(|e| e.to_string())?;
        collect_loads(&out.emitted, &mut loads);
        seq_inbox.extend(out.emitted.iter().cloned());
        self.observed.extend(out.emitted);

        // 2. 规则 -> 脚本函数调用
        for ScriptCall { function, args, self_entity } in out.calls {
            let so = self
                .scripts
                .call_fn(&function, &args, self_entity, world, rng, tick)
                .map_err(|e| e.to_string())?;
            collect_loads(&so.events, &mut loads);
            seq_inbox.extend(so.events.iter().cloned());
            self.observed.extend(so.events.iter().cloned());
            self.carryover.extend(so.events);
        }

        // 3. 脚本系统
        let so = self.scripts.run_systems(world, rng, tick).map_err(|e| e.to_string())?;
        collect_loads(&so.events, &mut loads);
        seq_inbox.extend(so.events.iter().cloned());
        self.observed.extend(so.events.iter().cloned());
        self.carryover.extend(so.events);

        // 4. 动画推帧（引擎独占 Sprite.image 的写权——动画永远不会被别的逻辑"打断"，
        //    想换动画只有一条正路：改 Anim.clip）
        let anim_events = advance_animations(world, &self.animations)?;
        self.observed.extend(anim_events.iter().cloned());
        self.carryover.extend(anim_events);

        // 4.5 序列推进（通用时间轴）：和动画同级的引擎系统，状态全在 Sequence
        //     组件里（快照/回放安全），按相对起跑 tick 发动通用动词。序列 emit 的
        //     load-scene/play-sound 走和规则同一条尾部管道，所以放在场景切换之前。
        let seq_events = advance_sequences(world, &self.sequences, &self.schema, &seq_inbox, tick)?;
        collect_loads(&seq_events, &mut loads);
        self.observed.extend(seq_events.iter().cloned());
        self.carryover.extend(seq_events);

        // 4.6 UI 布局（通用控件）：和动画/序列同级的引擎系统。在序列推进之后跑——
        //     序列可能 spawn/改 UI 节点，布局要看到本 tick 的最终 UI 树。脏标记保证
        //     静止 UI 零重算；解算结果写回 Ui.rx/ry/rw/rh（进哈希进存档，快照安全）。
        //     参照视口固定（[`UI_REFERENCE_VIEWPORT`]）——布局状态与渲染分辨率解耦，
        //     跨机器确定；渲染时 CPU/GPU 各自按真实分辨率重解算（纯函数，同一份逻辑）。
        advance_ui_layout(world, UI_REFERENCE_VIEWPORT)?;

        // 4.7 UI 交互（焦点导航 + 点击激活，1.2）：在布局之后跑——拾取/焦点几何要读
        //     本 tick 解算好的 Ui.rx/ry/rw/rh（参照系 1920×1080）。消费本 tick 的
        //     ui-up/down/left/right/confirm 输入和 ui-click 回复（坐标已是参照系归一化），
        //     更新 Button.state / UiRoot.focus / Button.press_t（全进组件 = 快照/录像安全），
        //     激活按钮 emit `ui-activate {id, action}` 让规则/序列接（UI 不内置题材动作）。
        //     ui-activate 不是 load-scene，本 tick 进 carryover，规则下一 tick 接龙——
        //     和序列 emit 同一条跨 tick 约定，确定且重放一致。
        let ui_events = advance_ui_interaction(world, &seq_inbox, UI_REFERENCE_VIEWPORT)?;
        collect_loads(&ui_events, &mut loads); // ui-activate 不会是 load-scene，但口径统一
        self.observed.extend(ui_events.iter().cloned());
        self.carryover.extend(ui_events);

        // 4.8 主题应用：把 Button.state 对应的主题底色解算进 Panel.color——render 只读
        //     Panel.color，渲染层不依赖主题表（code 只递交数据）。Panel.color 是确定性
        //     状态写入（进哈希），不是渲染装饰。按下反馈的 scale/modulate 才是 render 侧
        //     读 Button.press_t 的纯函数装饰（不进状态、不改布局）。
        apply_ui_theme(world, &self.themes)?;

        // 5. 场景切换（必须在确定性流水线内执行，重放才会在同一 tick 复现）
        if let Some(load) = loads.first() {
            if loads.len() > 1 {
                let wanted: Vec<String> = loads
                    .iter()
                    .map(|e| e.data.get("scene").cloned().unwrap_or(Value::Null).to_string())
                    .collect();
                return Err(format!(
                    "同一 tick 发出了 {} 个 load-scene（{}），去哪个场景没有答案。\
                     提示：给切换规则加条件互斥，一个 tick 只发一次 load-scene",
                    loads.len(),
                    wanted.join(", ")
                ));
            }
            let scene_rel = load
                .data
                .get("scene")
                .and_then(|v| v.as_str())
                .map(String::from)
                .ok_or_else(|| {
                    format!(
                        "load-scene 事件缺少 scene 字段（文本）。写法: \
                         {{\"emit\": \"load-scene\", \"data\": {{\"scene\": \"scenes/level2.json\"}}}}，\
                         实际 data: {}",
                        Value::Object(load.data.clone())
                    )
                })?;
            self.switch_scene(world, &scene_rel)?;
        }

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

/// 序列系统：通用时间轴原语，每 tick 推进活跃的 `Sequence` 组件。
///
/// 状态全在 `Sequence` 组件里（快照/回放安全）：`track`（引用哪条静态序列）、
/// `cursor`（下一条要发动的条目下标）、`start`（起跑 tick，-1=还没起跑）、
/// `wait`（在等的命名事件，空串=没在等）、`id`（完成事件带的 id，空=取 track 名）。
/// 静态轨道（条目数组）在 `catalog` 里，不进组件、不进每实例快照。
///
/// 语义合同：
/// - **空场零成本**：没有任何 Sequence 组件时每 tick 直接 early-return；
/// - 第一次被处理的 tick 把 `start` 从 -1 盖成当前 tick（elapsed=0）；
/// - 每 tick 发动所有 `at ≤ elapsed` 且下标 ≥ cursor 的条目（按下标序），
///   游标随之前进，直到撞上 `wait`（barrier）或没有到点的条目；
/// - `wait`：游标停在 barrier，直到 `inbox` 里出现那个命名事件才放行；
/// - **skip 跳过**：inbox 里有 `skip` 输入 → 把剩余条目的终态全部落定（无视 at
///   和 wait），随即发完成事件。skip 是输入、进录像，重放一致；
/// - 跑到末尾发 `sequence-finished {id, track}` 事件，序列实体自动 despawn；
/// - `tween` 动作 = spawn 一个 Tween 组件交给 sim 的 advance_tweens 执行（零重复）。
///
/// 序列借 `emit`（含 `sound`→play-sound）与"场景"解耦：切场景就 emit load-scene，
/// 由项目规则接去 load-scene；序列本身不认识"场景""关卡""过场"。
pub fn advance_sequences(
    world: &mut World,
    catalog: &BTreeMap<String, Sequence>,
    schema: &Schema,
    inbox: &[Event],
    tick: u64,
) -> Result<Vec<Event>, String> {
    let ids = world.query(&["Sequence"]);
    if ids.is_empty() {
        return Ok(Vec::new()); // 空场零成本：没有序列在播，零分配零遍历
    }
    // skip 是输入：录像里就是一条 {"action":"skip","phase":"pressed"} 的 input 事件
    let skip = inbox
        .iter()
        .any(|e| e.name == "input" && e.data.get("action").and_then(|v| v.as_str()) == Some("skip"));

    let mut events = Vec::new();
    for id in ids {
        if !world.is_alive(id) {
            continue; // 前一条序列的动作可能 despawn 了它
        }
        let track_name = world
            .get_field(id, "Sequence.track")
            .map_err(|e| e.to_string())?
            .as_str()
            .ok_or_else(|| format!("实体 {id} 的 Sequence.track 必须是文本（序列名）"))?
            .to_string();
        if track_name.is_empty() {
            continue; // 空 track = 没装序列，跳过
        }
        let seq = catalog.get(&track_name).ok_or_else(|| {
            format!(
                "实体 {id} 的 Sequence.track {track_name:?} 没有定义。已定义序列: [{}]。\
                 提示：序列在清单 sequences 列表的文件里定义",
                catalog.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;

        // 起跑盖章：start 从 -1 盖成当前 tick（进组件 = 进哈希进存档）
        let mut start = world
            .get_field(id, "Sequence.start")
            .map_err(|e| e.to_string())?
            .as_i64()
            .ok_or_else(|| format!("实体 {id} 的 Sequence.start 必须是整数"))?;
        if start < 0 {
            start = tick as i64;
            world.set_field(id, "Sequence.start", json!(start)).map_err(|e| e.to_string())?;
        }
        let elapsed = (tick as i64 - start).max(0) as u64;

        let mut cursor = world
            .get_field(id, "Sequence.cursor")
            .map_err(|e| e.to_string())?
            .as_i64()
            .ok_or_else(|| format!("实体 {id} 的 Sequence.cursor 必须是整数"))?
            .max(0) as usize;
        let mut waiting = world
            .get_field(id, "Sequence.wait")
            .map_err(|e| e.to_string())?
            .as_str()
            .unwrap_or("")
            .to_string();
        let seq_id = world
            .get_field(id, "Sequence.id")
            .ok()
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| track_name.clone());

        // barrier：在等某事件，本 tick inbox 里出现了它才放行（skip 也放行）
        if !waiting.is_empty() {
            let released = skip || inbox.iter().any(|e| e.name == waiting);
            if released {
                waiting.clear();
            } else {
                // 还在等：状态不变，继续下一条序列
                continue;
            }
        }

        // 发动条目。skip 时无视 at/wait 一口气落定剩余全部终态。
        let finished = loop {
            if cursor >= seq.steps.len() {
                break true; // 跑到末尾
            }
            let step = &seq.steps[cursor];
            if !skip && step.at > elapsed {
                break false; // 还没到点，等下一 tick
            }
            if step.kind == "wait" && !skip {
                // 撞上 barrier：记下要等的事件名，游标越过它，本 tick 停在这
                let name = step.action.get("wait").and_then(|v| v.as_str()).unwrap_or("");
                waiting = name.to_string();
                cursor += 1;
                break false;
            }
            // 执行动作（wait 在 skip 下当空操作直接跳过）
            if step.kind != "wait" {
                exec_seq_action(world, schema, step, &mut events)
                    .map_err(|e| format!("序列 {track_name:?} 第 {cursor} 条（{}）: {e}", step.kind))?;
            }
            cursor += 1;
        };

        if finished {
            events.push(Event::new(
                "sequence-finished",
                json!({"id": seq_id, "track": track_name}),
            ));
            if world.is_alive(id) {
                world.despawn(id).map_err(|e| e.to_string())?;
            }
        } else if world.is_alive(id) {
            // 回写最小播放状态（游标 + barrier 标志）
            world.set_field(id, "Sequence.cursor", json!(cursor as i64)).map_err(|e| e.to_string())?;
            world.set_field(id, "Sequence.wait", json!(waiting)).map_err(|e| e.to_string())?;
        }
    }
    Ok(events)
}

/// UI 布局系统（每 tick，和动画/序列同级的引擎系统）。**脏标记 + 一趟树遍历**：
/// 只在 UI 树的结构/尺寸（或视口尺寸）变了才真解算，把每个 Ui 节点的解算矩形
/// 写回 `Ui.rx/ry/rw/rh`（进组件 = 进哈希进存档，快照/录像安全）。
///
/// 脏判定靠 `UiRoot.layout_hash`：当前输入哈希（[`vitric_render::layout_input_hash`]，
/// 不含 rx/ry/rw/rh 输出本身）和上次存的相等 = 静止 = 跳过重算（"静止 UI 连播 N tick
/// 布局重算 0 次"的落点）。哈希变了才解算 + 回写 + 盖章新哈希。
///
/// 性能：场上没有 UI（无 UiRoot）= 第一行零成本 early-return（零分配零遍历）。
/// `viewport`：布局参照的视口尺寸（像素）。窗口/截图用真实分辨率，无头逻辑测试用
/// 一个约定的参照分辨率——布局是相对比例 + 像素偏移，参照尺寸进哈希一并跟踪。
pub fn advance_ui_layout(world: &mut World, viewport: (u32, u32)) -> Result<(), String> {
    let roots = world.query(&["UiRoot"]);
    if roots.is_empty() {
        return Ok(()); // 空 UI 零成本：没有 UiRoot，零分配零遍历
    }
    let (vw, vh) = viewport;
    let want = vitric_render::layout_input_hash(world, vw, vh);
    // 上次盖的哈希（取第一个 UiRoot 上的 layout_hash 字段，缺/不同 = 脏）
    let root = roots[0];
    let have = world
        .get_field(root, "UiRoot.layout_hash")
        .ok()
        .and_then(|v| v.as_str())
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok());
    if have == Some(want) {
        return Ok(()); // 静止：输入没变，布局结果就是上次写回的，跳过重算
    }
    // 脏：真解算一趟，把矩形写回每个节点
    let layout = vitric_render::solve_layout(world, vw, vh)?;
    for (id, r) in &layout {
        world.set_field(*id, "Ui.rx", json!(r.x)).map_err(|e| e.to_string())?;
        world.set_field(*id, "Ui.ry", json!(r.y)).map_err(|e| e.to_string())?;
        world.set_field(*id, "Ui.rw", json!(r.w)).map_err(|e| e.to_string())?;
        world.set_field(*id, "Ui.rh", json!(r.h)).map_err(|e| e.to_string())?;
    }
    // 盖章新哈希（仅当 UiRoot 有 layout_hash 字段时——schema 没声明就不写，
    // 退化成"每 tick 都解算"，仍然正确，只是不省那一趟；声明了才享受脏标记）
    if world.has_component(root, "UiRoot") && world.get_field(root, "UiRoot.layout_hash").is_ok() {
        world
            .set_field(root, "UiRoot.layout_hash", json!(format!("0x{want:016x}")))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// UI 交互系统（焦点导航 + 点击激活，1.2）。和动画/序列/布局同级的引擎系统，
/// **跑在布局之后**——焦点几何和点击拾取都读本 tick 解算好的 `Ui.rx/ry/rw/rh`
/// （参照系 1920×1080）。
///
/// 状态全进组件（快照/录像安全）：
/// - `UiRoot.focus`：当前焦点按钮的实体名（""=无焦点，引擎首次交互时落第一个可聚焦按钮）；
/// - `Button.state`：normal/focused/pressed/disabled；
/// - `Button.press_t`：按下反馈计时（-1=不在反馈中，0..PRESS_TICKS 递增；解析式不累加）。
///
/// 消费本 tick 的 `inbox`：
/// - `input {action: "ui-up"|"ui-down"|"ui-left"|"ui-right"}` → 按布局相邻关系移焦点；
/// - `input {action: "ui-confirm"}` → 激活当前焦点按钮；
/// - `ui-click {nx, ny, button}` → 屏幕归一化坐标（0..1）换算到参照系（×1920/×1080）
///   再判断落在哪个按钮矩形内，命中 = 激活（顺带把焦点移到它）。
///
/// **坐标换算链（关键接线）**：布局输出 rx/ry/rw/rh 是参照系 1920×1080 的像素矩形
/// （进哈希、与渲染分辨率解耦）；而点击源头是物理屏幕/窗口像素。窗口/RPC 注入端先把
/// 点击除以视口尺寸归一化成 0..1（[`vitric_control::inject_ui_click`]），本系统再乘回
/// 参照系 1920×1080——这样不管真实分辨率多大，命中判定永远对的是同一份参照系矩形，
/// 重放（录像里存的就是归一化坐标）逐位一致。**不**拿世界坐标比 UI 矩形（那是两套系）。
///
/// 激活 = 按钮置 pressed + press_t=0 + emit `ui-activate {id, action}`（规则/序列接）。
/// disabled 按钮不可聚焦、不响应点击/确认（合同第四节）。
///
/// 性能：焦点环 = query Button 的一趟（O(按钮数)，不全表扫）；空 UI（无 UiRoot）
/// 第一行零成本 early-return。
pub fn advance_ui_interaction(
    world: &mut World,
    inbox: &[Event],
    viewport: (u32, u32),
) -> Result<Vec<Event>, String> {
    if world.query(&["UiRoot"]).is_empty() {
        return Ok(Vec::new()); // 空 UI 零成本：没有 UiRoot，零分配零遍历
    }
    let root = world.query(&["UiRoot"])[0];
    let mut events = Vec::new();

    // 焦点环：所有挂 Button 的实体（query 槽位序 = 确定性），读状态 + 矩形。
    // disabled 不进可聚焦集合（焦点导航跳过它），但仍在表里（点击它要显式忽略）。
    struct Btn {
        id: EntityId,
        action: String,
        state: vitric_render::ButtonState,
        rect: vitric_render::UiRect,
    }
    let mut btns: Vec<Btn> = Vec::new();
    for id in world.query(&["Button", "Ui"]) {
        let state_name = world
            .get_field(id, "Button.state")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "normal".to_string());
        let state = vitric_render::ButtonState::parse(&state_name).ok_or_else(|| {
            format!(
                "实体 {id} 的 Button.state {state_name:?} 不是合法状态。可选: [{}]",
                vitric_render::BUTTON_STATES.join(", ")
            )
        })?;
        let action = world
            .get_field(id, "Button.action")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        let read_num = |path: &str| -> f64 {
            world.get_field(id, path).ok().and_then(Value::as_f64).unwrap_or(0.0)
        };
        let rect = vitric_render::UiRect {
            x: read_num("Ui.rx"),
            y: read_num("Ui.ry"),
            w: read_num("Ui.rw"),
            h: read_num("Ui.rh"),
        };
        btns.push(Btn { id, action, state, rect });
    }

    // 1) 按下反馈计时推进（解析式不累加：press_t 是 tick 计数，scale/modulate 由它一步算）。
    //    到点（press_t ≥ PRESS_TICKS）落回 normal（焦点态在第 3 步统一重置，这里只清反馈）。
    for b in &btns {
        let pt = world
            .get_field(b.id, "Button.press_t")
            .ok()
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        if pt >= 0 {
            let next = pt + 1;
            if next as u64 >= vitric_render::PRESS_TICKS {
                // 反馈结束：清计时 + 回 normal（若是焦点按钮，第 3 步会再置回 focused）
                world.set_field(b.id, "Button.press_t", json!(-1)).map_err(|e| e.to_string())?;
                if b.state == vitric_render::ButtonState::Pressed {
                    world
                        .set_field(b.id, "Button.state", json!("normal"))
                        .map_err(|e| e.to_string())?;
                }
            } else {
                world.set_field(b.id, "Button.press_t", json!(next)).map_err(|e| e.to_string())?;
            }
        }
    }

    // 当前焦点：UiRoot.focus 存的是实体名。空/失效 → 落第一个可聚焦按钮（确定性兜底）。
    let focus_name = world
        .get_field(root, "UiRoot.focus")
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let focusable: Vec<usize> = btns
        .iter()
        .enumerate()
        .filter(|(_, b)| b.state != vitric_render::ButtonState::Disabled)
        .map(|(i, _)| i)
        .collect();
    // 当前焦点在 btns 里的下标（按名字找）。空名/找不到 = None。
    let mut focus_idx: Option<usize> = if focus_name.is_empty() {
        None
    } else {
        btns.iter().position(|b| world.name_of(b.id) == Some(focus_name.as_str()))
    };
    // 焦点指向了 disabled / 不存在 → 收回到第一个可聚焦
    if focus_idx.is_none_or(|i| btns[i].state == vitric_render::ButtonState::Disabled) {
        focus_idx = focusable.first().copied();
    }

    // 2) 方向输入：移动焦点（按布局相邻关系，只在可聚焦集合内）。
    //    多个方向输入同 tick：按到达顺序逐个应用（确定性，inbox 已是固定序）。
    let focus_geom: Vec<vitric_render::Focusable> = focusable
        .iter()
        .map(|&i| vitric_render::Focusable { rect: btns[i].rect })
        .collect();
    for e in inbox {
        if e.name != "input" {
            continue;
        }
        let action = e.data.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let phase = e.data.get("phase").and_then(|v| v.as_str()).unwrap_or("pressed");
        if phase != "pressed" {
            continue; // 只在按下沿移焦点（释放不动）
        }
        let Some(dir_name) = action.strip_prefix("ui-") else { continue };
        let Some(dir) = vitric_render::Dir::parse(dir_name) else { continue };
        let Some(cur) = focus_idx.and_then(|fi| focusable.iter().position(|&x| x == fi)) else {
            // 还没有焦点：方向键先落到第一个可聚焦（标准菜单手感）
            focus_idx = focusable.first().copied();
            continue;
        };
        let next_in_ring = vitric_render::navigate(&focus_geom, cur, dir);
        focus_idx = Some(focusable[next_in_ring]);
    }

    // 3) 重置焦点态：可聚焦按钮里，焦点那个置 focused，其余非 pressed 的置 normal。
    //    pressed（反馈中）的不被焦点重置盖掉——反馈结束（第 1 步）才回 normal。
    let focus_id = focus_idx.map(|i| btns[i].id);
    for b in &btns {
        if b.state == vitric_render::ButtonState::Disabled {
            continue; // disabled 状态固定，不被焦点逻辑动
        }
        // 当前组件里的真实状态（第 1 步可能刚改过 press_t/state，重读）
        let cur = world
            .get_field(b.id, "Button.state")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "normal".to_string());
        if cur == "pressed" {
            continue; // 反馈中，别盖
        }
        let want = if Some(b.id) == focus_id { "focused" } else { "normal" };
        if cur != want {
            world.set_field(b.id, "Button.state", json!(want)).map_err(|e| e.to_string())?;
        }
    }
    // 焦点名字写回 UiRoot（进哈希进存档）
    let new_focus_name = focus_id.and_then(|id| world.name_of(id)).unwrap_or("").to_string();
    if new_focus_name != focus_name {
        world.set_field(root, "UiRoot.focus", json!(new_focus_name)).map_err(|e| e.to_string())?;
    }

    // 4) 确认键：激活当前焦点按钮。
    let confirm = inbox.iter().any(|e| {
        e.name == "input"
            && e.data.get("action").and_then(|v| v.as_str()) == Some("ui-confirm")
            && e.data.get("phase").and_then(|v| v.as_str()).unwrap_or("pressed") == "pressed"
    });
    if confirm {
        if let Some(fid) = focus_id {
            let b = btns.iter().find(|b| b.id == fid).expect("focus_id 来自 btns");
            activate_button(world, b.id, &b.action, &mut events)?;
        }
    }

    // 5) 点击：屏幕归一化坐标 → 参照系 1920×1080 → 命中按钮矩形 → 激活。
    let (vw, vh) = viewport;
    for e in inbox {
        if e.name != "ui-click" {
            continue;
        }
        let nx = e.data.get("nx").and_then(|v| v.as_f64());
        let ny = e.data.get("ny").and_then(|v| v.as_f64());
        let (Some(nx), Some(ny)) = (nx, ny) else { continue };
        // 归一化 → 参照系像素（和 rx/ry/rw/rh 同坐标系）
        let px = nx * vw as f64;
        let py = ny * vh as f64;
        // 命中判定：query 倒序（后画盖在上面，优先命中），disabled 不响应。
        let hit = btns.iter().rev().find(|b| {
            b.state != vitric_render::ButtonState::Disabled
                && px >= b.rect.x
                && px < b.rect.x + b.rect.w
                && py >= b.rect.y
                && py < b.rect.y + b.rect.h
        });
        if let Some(b) = hit {
            // 点击命中也把焦点移到它（点和焦点统一），再激活
            let name = world.name_of(b.id).unwrap_or("").to_string();
            world.set_field(root, "UiRoot.focus", json!(name)).map_err(|e| e.to_string())?;
            activate_button(world, b.id, &b.action, &mut events)?;
        }
    }

    Ok(events)
}

/// 激活一个按钮：置 pressed + 起按下反馈计时 + emit `ui-activate {id, action}`。
/// action 空串也照发（check 已拦空 action，运行时不二次拒绝——显式行为）。
fn activate_button(
    world: &mut World,
    id: EntityId,
    action: &str,
    events: &mut Vec<Event>,
) -> Result<(), String> {
    world.set_field(id, "Button.state", json!("pressed")).map_err(|e| e.to_string())?;
    // press_t=0：本 tick 就是反馈第 0 帧（解析式 press_scale(0)=1，下一 tick 起缩）
    world.set_field(id, "Button.press_t", json!(0)).map_err(|e| e.to_string())?;
    let name = world.name_of(id).map(String::from).unwrap_or_else(|| id.to_string());
    events.push(Event::new("ui-activate", json!({"id": name, "action": action})));
    Ok(())
}

/// 主题应用系统：把每个 Button 的 `state` 对应的主题底色写进它的 `Panel.color`。
/// 渲染层只读 Panel.color（不认识主题表）——主题在装配期、状态在组件、解算在这里，
/// 渲染只管画解算好的颜色（code 只递交数据）。
///
/// `Panel.color` 是**确定性状态**（进哈希进存档），不是渲染装饰——同 state 同色，
/// 重放一致。按下反馈的 scale/modulate 才是渲染侧读 `Button.press_t` 的纯函数装饰。
///
/// 没有主题（Button 无 theme 字段或引用空）= 跳过该按钮（保留场景里写死的 Panel.color，
/// 不强加主题）。引用了不存在的主题在 check 期已红灯，这里运行时显式报错兜底。
pub fn apply_ui_theme(
    world: &mut World,
    themes: &BTreeMap<String, vitric_data::Theme>,
) -> Result<(), String> {
    let buttons = world.query(&["Button", "Panel"]);
    if buttons.is_empty() {
        return Ok(()); // 没有按钮 = 零成本
    }
    for id in buttons {
        let theme_name = world
            .get_field(id, "Button.theme")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if theme_name.is_empty() {
            continue; // 没引用主题：保留场景写死的 Panel.color
        }
        let theme = themes.get(&theme_name).ok_or_else(|| {
            format!(
                "实体 {id} 的 Button.theme {theme_name:?} 没有定义。已定义主题: [{}]。\
                 提示：主题文件加进 vitric.json 的 themes 数组",
                themes.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;
        let state = world
            .get_field(id, "Button.state")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "normal".to_string());
        let style = theme.button_style(&state).ok_or_else(|| {
            format!("实体 {id} 的 Button.state {state:?} 在主题 {theme_name:?} 里没有样式（应已 check 拦截）")
        })?;
        // 只在变了才写（避免无谓的"脏"——虽然写同值哈希不变，但少一次写更干净）
        let cur = world.get_field(id, "Panel.color").ok().and_then(|v| v.as_str().map(String::from));
        if cur.as_deref() != Some(style.bg.as_str()) {
            world.set_field(id, "Panel.color", json!(style.bg)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 执行序列里一条动作（动作集已被 vitric-data 校验过类型/字段）。
/// 全部镜像引擎已有通用动词，不新造语义。组件值统一过 schema 归一化
/// （number 一律存浮点形态）——表示唯一，状态哈希不受写入方影响（与场景/规则同口径）。
fn exec_seq_action(
    world: &mut World,
    schema: &Schema,
    step: &SeqStep,
    events: &mut Vec<Event>,
) -> Result<(), String> {
    let obj = step.action.as_object().expect("校验层已确保动作是对象");
    match step.kind.as_str() {
        // tween：起一个 Tween 组件，交给 sim 的 advance_tweens 执行（零重复）
        "tween" => {
            let spec = obj["tween"].as_object().expect("校验过");
            let target = spec.get("target").and_then(|v| v.as_str()).expect("校验过");
            let target_handle = resolve_entity(world, target)?;
            let mut comp = serde_json::Map::new();
            comp.insert("target".into(), json!(target_handle.to_string()));
            for key in ["field", "from", "to", "duration", "ease"] {
                if let Some(v) = spec.get(key) {
                    comp.insert(key.into(), v.clone());
                }
            }
            comp.insert("start".into(), json!(-1)); // 由补间系统起跑时盖章
            comp.insert("id".into(), spec.get("id").cloned().unwrap_or_else(|| json!("")));
            let tw = world.spawn();
            let value = normalize_component(schema, "Tween", Value::Object(comp))?;
            world.set_component(tw, "Tween", value).map_err(|e| e.to_string())?;
        }
        // set：瞬时设字段（镜像规则 set）
        "set" => {
            let target = obj["set"].as_str().expect("校验过");
            let to = obj.get("to").expect("校验过").clone();
            let (id, path) = resolve_field(world, target)?;
            world.set_field(id, &path, to).map_err(|e| e.to_string())?;
        }
        // spawn：生成实体（镜像规则 spawn）
        "spawn" => {
            let spec = obj["spawn"].as_object().expect("校验过");
            let comps = spec.get("components").and_then(|v| v.as_object()).expect("校验过");
            let id = match spec.get("name").and_then(|v| v.as_str()) {
                Some(name) => world.spawn_named(name).map_err(|e| e.to_string())?,
                None => world.spawn(),
            };
            for (cname, cval) in comps {
                let value = normalize_component(schema, cname, cval.clone())?;
                world.set_component(id, cname, value).map_err(|e| e.to_string())?;
            }
        }
        // despawn：销毁实体（镜像规则 despawn）
        "despawn" => {
            let target = obj["despawn"].as_str().expect("校验过");
            let id = resolve_entity(world, target)?;
            world.despawn(id).map_err(|e| e.to_string())?;
        }
        // emit：发事件让规则接龙（与场景解耦的正门）
        "emit" => {
            let name = obj["emit"].as_str().expect("校验过");
            let data = obj.get("data").cloned().unwrap_or_else(|| json!({}));
            events.push(Event::new(name, data));
        }
        // sound：播音效（镜像音频，翻成 play-sound 事件——和规则同一条音频通道）
        "sound" => {
            let sound = obj["sound"].as_str().expect("校验过");
            let mut data = serde_json::Map::new();
            data.insert("sound".into(), json!(sound));
            if let Some(vol) = obj.get("volume") {
                data.insert("volume".into(), vol.clone());
            }
            events.push(Event::new("play-sound", Value::Object(data)));
        }
        other => return Err(format!("未知序列动作 {other:?}（校验层应已拦截）")),
    }
    Ok(())
}

/// 按 schema 归一化一条组件值（number→浮点、填默认值）。组件名未知则显式报错。
/// 序列 spawn/tween 出来的组件和场景/规则一样走这道归一化，状态哈希才一致。
fn normalize_component(schema: &Schema, cname: &str, value: Value) -> Result<Value, String> {
    let cschema = schema.component(cname).ok_or_else(|| {
        format!(
            "未知组件 {cname:?}。schema 里的组件: [{}]",
            schema.components.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;
    let mut report = vitric_data::ValidationReport::default();
    let normalized = cschema.normalize(&value, &format!("sequence/{cname}"), &mut report);
    if !report.ok() {
        return Err(format!("组件值未通过 schema 校验:\n{report}"));
    }
    Ok(normalized)
}

/// 解析实体引用："@名字" / "名字" / "e3v1" 句柄。序列动作里的实体引用走这条。
fn resolve_entity(world: &World, s: &str) -> Result<EntityId, String> {
    let name = s.strip_prefix('@').unwrap_or(s);
    if let Ok(id) = world.entity(name) {
        return Ok(id);
    }
    if let Ok(h) = name.parse::<EntityId>() {
        if world.is_alive(h) {
            return Ok(h);
        }
    }
    Err(format!(
        "实体引用 {s:?} 找不到。提示：填场景/序列里已生成的实体名（可带 @ 前缀）"
    ))
}

/// 解析 "实体.字段路径"（如 "@subtitle.Text.content"）成 (句柄, 字段路径)。
fn resolve_field(world: &World, target: &str) -> Result<(EntityId, String), String> {
    let (ent, path) = target.split_once('.').ok_or_else(|| {
        format!("目标 {target:?} 缺少字段路径，写法 \"@实体名.组件.字段\"")
    })?;
    Ok((resolve_entity(world, ent)?, path.to_string()))
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

/// 扫描脚本**源码文本**里的字面 .png 贴图引用，每个引用必须在素材仓库里。
///
/// 认的写法（真实事故原型：脚本 ctx.spawn 用了不存在的 "dust.png"，check 绿灯、
/// 游戏跑到一半渲染硬炸）：
/// - JS 对象字面量：`image: "dust.png"` / `image: 'dust.png'`
/// - JSON 风格带引号的键：`"image": "dust.png"` / `'image': 'dust.png'`
///
/// 诚实的局限——这是对字面量的 lint，不是数据流分析：
/// 动态拼接（`"dust_" + i + ".png"`）、变量间接引用扫不到，check 过了不等于
/// 运行期一定有图。所以错误提示里劝"尽量用字面名"，让 lint 兜得住。
/// 扫的是磁盘上的原文（.ts 不经转译直接扫——esbuild 只剥类型不动字符串字面量）。
fn scan_script_image_refs(src: &str, file: &str, assets: &vitric_render::Assets, missing: &mut Vec<String>) {
    let bytes = src.as_bytes();
    let mut from = 0;
    while let Some(pos) = src[from..].find("image") {
        let start = from + pos;
        let mut i = start + "image".len();
        from = i; // 无论本次匹配成不成立，下一轮从 "image" 之后继续
        // 键的左边界：前一个字符不能是标识符成分（排掉 bgimage / my_image / e.image）
        let prev = src[..start].chars().next_back();
        if prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.') {
            continue;
        }
        // 键带引号（"image" / 'image'）：闭引号必须与开引号配对、紧跟键名
        if let Some(q @ ('"' | '\'')) = prev {
            if bytes.get(i) != Some(&(q as u8)) {
                continue; // "image arts" 之类普通字符串内容，不是键
            }
            i += 1;
        }
        // 冒号（两侧允许空白）
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        if bytes.get(i) != Some(&b':') {
            continue;
        }
        i += 1;
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        // 值必须是同引号闭合的单段字符串字面量（中途出现拼接/换行就不是字面引用）
        let Some(&vq) = bytes.get(i) else { continue };
        if vq != b'"' && vq != b'\'' {
            continue;
        }
        let vstart = i + 1;
        let Some(rel_end) = src[vstart..].find(vq as char) else { continue };
        let literal = &src[vstart..vstart + rel_end];
        if literal.is_empty() || literal.contains('\n') || literal.contains('\\') {
            continue;
        }
        if !literal.to_ascii_lowercase().ends_with(".png") {
            continue;
        }
        if assets.image(literal).is_none() {
            missing.push(format!(
                "{file} 的脚本字面引用了不存在的贴图 {literal:?}。\
                 提示: 脚本 spawn 的贴图也要放 assets/（路径相对 assets/ 写）；\
                 这是字面量扫描，动态拼接的引用扫不到——尽量用字面名"
            ));
        }
    }
}

/// 递归扫规则文档里 `{"spawn": {"components": {"Sprite": {"image": "字面量"}}}}` 的
/// 贴图引用（与 [`scan_sound_refs`] 同款走法）。运行时引用（self./other./event./@）
/// 不做静态校验——和音效扫描同一条豁免规则。
fn scan_rule_image_refs(doc: &Value, file: &str, assets: &vitric_render::Assets, missing: &mut Vec<String>) {
    match doc {
        Value::Object(map) => {
            if let Some(image) = map
                .get("spawn")
                .and_then(|s| s.get("components"))
                .and_then(|c| c.get("Sprite"))
                .and_then(|s| s.get("image"))
                .and_then(|v| v.as_str())
            {
                let is_ref = image.starts_with("self.")
                    || image.starts_with("other.")
                    || image.starts_with("event.")
                    || image.starts_with('@');
                if !image.is_empty() && !is_ref && assets.image(image).is_none() {
                    missing.push(format!(
                        "{file} 的 spawn 动作引用了不存在的贴图 {image:?}。\
                         提示: 规则 spawn 的贴图也要放 assets/；动态引用（event.* 等）扫不到"
                    ));
                }
            }
            for v in map.values() {
                scan_rule_image_refs(v, file, assets, missing);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                scan_rule_image_refs(v, file, assets, missing);
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
    let mut assets = vitric_render::Assets::load_dir(&dir.join("assets"))?;
    // 字体：清单挂了 font 就在 check 期真的解析一遍（存在性 Project::load 已查，
    // 这里抓"文件在但不是合法 TTF"）
    if let Some(font_rel) = &project.manifest.font {
        assets.load_font(&dir.join(font_rel))?;
    }
    let mut missing = Vec::new();
    // 清单里的**每个**场景都实例化 + 查引用——load-scene 随时可能切过去，
    // 非入口场景的坏引用不在 check 期抓，就会在切换的那一刻才炸
    for (rel, scene) in &project.scenes {
        let mut scratch;
        let world: &World = if rel == &project.manifest.entry {
            &sim.world // 入口已实例化（报告里的 entities/initial_hash 用它）
        } else {
            scratch = World::new();
            vitric_data::instantiate_scene(scene, &project.schema, &mut scratch)
                .map_err(|r| format!("场景 {rel:?} 实例化失败:\n{r}"))?;
            &scratch
        };
        for id in world.query(&["Sprite"]) {
            if let Ok(image) = world.get_field(id, "Sprite.image") {
                if let Some(name) = image.as_str().filter(|s| !s.is_empty()) {
                    if assets.image(name).is_none() {
                        missing.push(format!(
                            "场景 {rel} 的实体 {}{} 引用了不存在的素材 {name:?}",
                            id,
                            world.name_of(id).map(|n| format!("({n})")).unwrap_or_default(),
                        ));
                    }
                }
            }
        }
        // UI Panel.image 引用的精灵都在素材库（和 Sprite.image 同口径）
        for id in world.query(&["Panel"]) {
            if let Ok(image) = world.get_field(id, "Panel.image") {
                if let Some(name) = image.as_str().filter(|s| !s.is_empty()) {
                    if assets.image(name).is_none() {
                        missing.push(format!(
                            "场景 {rel} 的实体 {}{} 的 Panel.image 引用了不存在的素材 {name:?}",
                            id,
                            world.name_of(id).map(|n| format!("({n})")).unwrap_or_default(),
                        ));
                    }
                }
            }
        }
        // 场景里 Anim.clip 引用的片段都已定义
        for id in world.query(&["Anim"]) {
            if let Ok(clip) = world.get_field(id, "Anim.clip") {
                if let Some(name) = clip.as_str().filter(|s| !s.is_empty()) {
                    if !project.animations.contains_key(name) {
                        missing.push(format!(
                            "场景 {rel} 的实体 {}{} 的 Anim.clip 引用了未定义的片段 {name:?}（已定义: [{}]）",
                            id,
                            world.name_of(id).map(|n| format!("({n})")).unwrap_or_default(),
                            project.animations.keys().cloned().collect::<Vec<_>>().join(", "),
                        ));
                    }
                }
            }
        }
        // Button.theme 引用的主题必须在清单 themes 里定义（值级状态/action 校验在
        // vitric-data 的 validate_ui_components；主题名存在性要项目级表，归这里）
        for id in world.query(&["Button"]) {
            if let Ok(theme) = world.get_field(id, "Button.theme") {
                if let Some(name) = theme.as_str().filter(|s| !s.is_empty()) {
                    if !project.themes.contains_key(name) {
                        missing.push(format!(
                            "场景 {rel} 的实体 {}{} 的 Button.theme 引用了未定义的主题 {name:?}（已定义: [{}]）。\
                             提示：主题文件加进 vitric.json 的 themes 数组（themes/<名>.json）",
                            id,
                            world.name_of(id).map(|n| format!("({n})")).unwrap_or_default(),
                            project.themes.keys().cloned().collect::<Vec<_>>().join(", "),
                        ));
                    }
                }
            }
        }
    }
    // 动画：clip 引用的帧图都在素材库
    for (cname, clip) in &project.animations {
        for frame in &clip.frames {
            if assets.image(frame).is_none() {
                missing.push(format!("动画片段 {cname:?} 引用了不存在的帧图 {frame:?}"));
            }
        }
    }
    // 序列：结构/动作字段由 Project::load（vitric-data）已校验；这里补跨文件引用——
    // spawn 的字面贴图、sound 的字面音效、emit "load-scene" 的目标场景都得真存在
    for seq in project.sequences.values() {
        for step in &seq.steps {
            scan_rule_image_refs(&step.action, &seq.file, &assets, &mut missing);
            scan_sound_refs(&step.action, &seq.file, &dir.join("sounds"), &mut missing);
            // sound 动作的字面音效
            if step.kind == "sound" {
                if let Some(name) = step.action.get("sound").and_then(|v| v.as_str()) {
                    if !name.is_empty()
                        && !name.contains("..")
                        && !name.starts_with('/')
                        && !dir.join("sounds").join(name).exists()
                    {
                        missing.push(format!(
                            "序列 {} 引用了不存在的音效 {name:?}（应在项目 sounds/ 目录）",
                            seq.file
                        ));
                    }
                }
            }
            // emit "load-scene" 的目标场景必须在清单 scenes 列表里
            if step.kind == "emit"
                && step.action.get("emit").and_then(|v| v.as_str()) == Some("load-scene")
            {
                if let Some(scene) = step
                    .action
                    .get("data")
                    .and_then(|d| d.get("scene"))
                    .and_then(|v| v.as_str())
                {
                    if !project.scenes.contains_key(scene) {
                        missing.push(format!(
                            "序列 {} emit 的 load-scene 目标场景 {scene:?} 不在清单 scenes 列表里",
                            seq.file
                        ));
                    }
                }
            }
        }
    }
    // 音效：规则里字面引用的 play-sound 音效文件必须存在
    for (file, doc) in &project.rules {
        scan_sound_refs(doc, file, &dir.join("sounds"), &mut missing);
    }
    // 贴图字面引用：脚本源码（ctx.spawn 等）与规则 spawn 动作里写死的 .png
    // 必须在素材仓库——场景之外动态生出来的实体也不许引用不存在的图。
    // 字面量 lint 的局限见各扫描函数的文档（动态拼接扫不到）
    for (file, src) in &project.scripts {
        scan_script_image_refs(src, file, &assets, &mut missing);
    }
    for (file, doc) in &project.rules {
        scan_rule_image_refs(doc, file, &assets, &mut missing);
    }
    // 帧进口产物（vitric assets --frames 出的 *-atlas.json sidecar）：图集存在、
    // 帧表合法、uv/rect 不越界、引用的帧图都在、压缩产物头合法。纯新增——没有
    // 帧进口产物的老项目这里零成本（assets/ 里没有 *-atlas.json）。
    for atlas_rel in discover_atlas_sidecars(&dir.join("assets")) {
        crate::frames::check_atlas_products(dir, &atlas_rel, &mut missing);
    }
    if !missing.is_empty() {
        return Err(format!(
            "素材/动画/音效/贴图引用校验失败:\n  {}\n现有素材: [{}]",
            missing.join("\n  "),
            assets.names().join(", ")
        ));
    }
    Ok(serde_json::json!({
        "project": project.manifest.name,
        "scenes": project.scenes.keys().collect::<Vec<_>>(),
        "sequences": project.sequences.keys().collect::<Vec<_>>(),
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
        // 文字渲染路径可观察：挂了字体报路径，没挂明说是点阵
        "font": project.manifest.font.clone().unwrap_or_else(|| "内嵌 8x8 点阵".to_string()),
        "initial_hash": format!("{:#018x}", sim.world.state_hash()),
    }))
}

/// 找出 assets/ 顶层的帧进口图集 sidecar（`*-atlas.json`），返回相对 assets/ 的名字。
/// 不存在/读不了 = 空（合法：没用过 --frames 的项目）。只扫顶层（产物落在那）。
fn discover_atlas_sidecars(assets_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(assets_dir) else {
        return out;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.ends_with("-atlas.json") {
            out.push(name);
        }
    }
    out.sort(); // 确定的报错顺序
    out
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

    // ---- 脚本/规则字面贴图引用扫描 ----

    fn scan_src(src: &str) -> Vec<String> {
        let mut missing = Vec::new();
        // 空素材仓库：所有字面引用都该被报"不存在"
        scan_script_image_refs(src, "scripts/fx.js", &vitric_render::Assets::empty(), &mut missing);
        missing
    }

    #[test]
    fn script_scan_flags_literal_png_in_spawn() {
        let missing =
            scan_src(r#"vitric.fn("boom", (ctx) => { ctx.spawn({ Sprite: { image: "dust.png", w: 1, h: 1 } }); });"#);
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(missing[0].contains("dust.png"), "报错带贴图名: {}", missing[0]);
        assert!(missing[0].contains("scripts/fx.js"), "报错带来源文件: {}", missing[0]);
        assert!(missing[0].contains("动态拼接"), "局限要写进错误提示: {}", missing[0]);
    }

    #[test]
    fn script_scan_covers_quoted_key_and_single_quotes() {
        // JSON 风格键 + 单引号两种写法都认
        let missing = scan_src(r#"const a = { "image": "a.png" }; const b = { 'image': 'b.png' };"#);
        assert_eq!(missing.len(), 2, "{missing:?}");
        assert!(missing[0].contains("a.png") && missing[1].contains("b.png"));
    }

    #[test]
    fn script_scan_skips_dynamic_and_non_keys() {
        // 动态拼接是文档化局限：不报（不许误报——误报会让 agent 学会无视 check）
        assert!(scan_src(r#"ctx.spawn({ Sprite: { image: "dust_" + i + ".png" } });"#).is_empty());
        // 别的标识符撞上 image 子串 / 属性读取：都不是键
        assert!(scan_src(r#"const bgimage: string = "x.png"; e.Sprite.image = takeFrom(pool);"#).is_empty());
        // 非 .png 字面量不在本 lint 范围
        assert!(scan_src(r#"spawn({ image: "sheet.jpg" })"#).is_empty());
        // 已知过报边界（文本级扫描不解析语法）：注释/字符串里长得像 `image: 'x.png'`
        // 的文本也会被当引用。锁住这个行为——它变了说明扫描器语义动了
        assert_eq!(scan_src(r#"log("an image: 'y.png' inside a string")"#).len(), 1);
    }

    #[test]
    fn rule_scan_flags_spawn_sprite_image_and_skips_refs() {
        let mut missing = Vec::new();
        let doc = json!({"do": [
            {"spawn": {"components": {"Sprite": {"image": "puff.png", "w": 1, "h": 1}}}},
            {"spawn": {"components": {"Sprite": {"image": "event.img", "w": 1, "h": 1}}}},
        ]});
        scan_rule_image_refs(&doc, "rules/fx.json", &vitric_render::Assets::empty(), &mut missing);
        assert_eq!(missing.len(), 1, "只有字面量该被报: {missing:?}");
        assert!(missing[0].contains("puff.png") && missing[0].contains("rules/fx.json"));
    }
}
