//! Runtime — assembles project data, the rule engine, and the script engine into a runnable game.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use vitric_data::{Clip, Project, Scene, Schema, SeqStep, Sequence};
use vitric_ecs::{EntityId, World};
use vitric_rules::{Engine, Event, RuleSet, ScriptCall};
use vitric_script::ScriptEngine;
use vitric_sim::{GameLogic, Pcg32, Sim};

/// UI layout's reference viewport size (pixels) in the simulation state. Layout is relative ratios + pixel offsets,
/// and the solved result is written into components (entering hash and save state), so it must be decoupled from the concrete window resolution — otherwise the same session
/// would diverge in state hash across machines with different resolutions. Conventional 1920×1080: cross-machine deterministic. At render time CPU/GPU
/// each re-solve with the real window resolution (the same solve_layout pure function), UI anchors to the viewport and scales naturally.
pub const UI_REFERENCE_VIEWPORT: (u32, u32) = (1920, 1080);

/// Game logic assembly: rules are the front door, scripts handle complex logic.
///
/// Per-tick execution order (fixed, part of determinism):
/// 1. Rules consume this tick's events (input / collision / events emitted by scripts last tick);
/// 2. Each `call` produced by rules invokes a script function;
/// 3. Script systems each run once in registration order;
/// 4. Events emitted by scripts go into the next tick's inbox.
pub struct Runtime {
    pub rules: Engine,
    pub scripts: ScriptEngine,
    /// Animation clip definitions.
    pub animations: BTreeMap<String, Clip>,
    /// Sequence (timeline) static track definitions. Runtime Sequence components reference them by name,
    /// static tracks do not enter per-instance snapshots (components only store minimal playback state).
    pub sequences: BTreeMap<String, Sequence>,
    /// Themes (style volumes). Assembly-time constants, do not enter world state; UI controls reference them by name to fetch styles.
    /// Each tick resolves the theme background color corresponding to Button.state into Panel.color (render reads Panel.color,
    /// the render layer does not depend on the theme table — code only hands off data).
    pub themes: BTreeMap<String, vitric_data::Theme>,
    /// schema (used to instantiate new scenes on scene switches; the same definition held by rules/scripts).
    schema: Schema,
    /// All scenes in the manifest (immutable copies preloaded at assembly time). Scene switches fetch data from here
    /// rather than reading disk at switch time — scene files modified on disk at runtime do not affect this process's
    /// switch results, replay and the original session load the same in-memory data, determinism is not broken by hot edits.
    scenes: BTreeMap<String, Scene>,
    /// Project root directory (hot reload re-reads disk from here).
    root: Option<std::path::PathBuf>,
    /// Events emitted by scripts last tick, handed to rules this tick.
    carryover: Vec<Event>,
    /// Copies of all events emitted by rules/scripts this tick, taken by the main loop and fed into the control plane event log.
    observed: Vec<Event>,
}

impl Runtime {
    /// Assemble the runtime from an already-loaded project (rule semantic validation and script evaluation happen here).
    pub fn build(project: &Project) -> Result<Runtime, String> {
        // Rules: multiple files merged into one rule set
        let mut all = RuleSet::default();
        for (file, doc) in &project.rules {
            let set = RuleSet::parse(doc, file).map_err(|r| r.to_string())?;
            all.rules.extend(set.rules);
        }
        let rules = Engine::new(all, project.schema.clone());

        // Scripts (.ts is transpiled to JS by esbuild before entering QuickJS)
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

    /// Load project + assemble + instantiate the entry scene, returns a ready-to-run (Sim, Runtime).
    pub fn boot(dir: &Path) -> Result<(Sim, Runtime), String> {
        let project = Project::load(dir).map_err(|r| r.to_string())?;
        let mut runtime = Runtime::build(&project)?;
        runtime.root = Some(dir.to_path_buf());
        let mut sim = Sim::new(project.manifest.seed);
        vitric_data::instantiate_scene(project.entry_scene(), &project.schema, &mut sim.world)
            .map_err(|r| r.to_string())?;
        Ok((sim, runtime))
    }

    /// Scene switch (the execution side of the convention event `load-scene {"scene": "scenes/xxx.json"}`).
    ///
    /// Timing constraint: the switch happens at the tail of on_tick, still inside sim.step's deterministic pipeline —
    /// the load-scene event that triggers it is itself produced deterministically by rules/scripts, so replaying the same
    /// recording will arrive here at the same tick and assemble the same world, the checkpoint hash still matches.
    ///
    /// Semantics: by default the whole world is torn down and rebuilt (clear_entities goes through proper despawn, all old handles
    /// are invalidated); entities that want to survive across scenes attach a `Persist` marker component — all its components are
    /// moved as-is into the new world (rebuilt with the same name, in slot order). The new scene's initialization hook is the next tick's
    /// `scene-loaded` event; `start` is emitted only once at tick 0 of the whole session, never re-emitted.
    fn switch_scene(&mut self, world: &mut World, scene_rel: &str) -> Result<(), String> {
        let scene = self.scenes.get(scene_rel).ok_or_else(|| {
            format!(
                "load-scene 引用的场景 {scene_rel:?} 不在清单 scenes 列表里。\
                 可用场景: [{}]。提示：新场景文件要先加进 vitric.json 的 scenes 数组",
                self.scenes.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;

        // Persist survivors: snapshot first (name + all components), errors all surface before touching the world
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

        // Tear down → rebuild. Note that events already emitted this tick in carryover (including animation events) are not cleared:
        // events are pure data and are delivered to the next tick as usual — the same convention as "script-emitted events are delivered across ticks".
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

        // The new scene's "start": the next tick's scene-loaded event (added to observed so the control plane can see it)
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
        // The sequence wait barrier needs to see this tick's events: input (including skip), the carryover emitted by scripts last tick,
        // and the named events emitted by rules/scripts this tick (player-confirm and similar barrier-releasing events).
        // Events emitted by rules are consumed in process_tick's cascade and do not enter carryover, so the sequence must **additionally** receive a copy of all events emitted this tick here.
        let mut seq_inbox = inbox.clone();

        // load-scene (the scene-switch convention event) emitted by rules/scripts this tick. The switch is deferred to
        // a unified execution at the tail of the pipeline — after the emitter emits, the rest of this tick's logic still faces the old world,
        // the ordering is clear; we only look at "our own" emitted events, external injection that wants to switch scenes also has to go through the rules front door.
        let mut loads: Vec<Event> = Vec::new();
        let collect_loads = |events: &[Event], loads: &mut Vec<Event>| {
            loads.extend(events.iter().filter(|e| e.name == "load-scene").cloned());
        };

        // 1. Rules
        let out = self.rules.process_tick(world, inbox).map_err(|e| e.to_string())?;
        collect_loads(&out.emitted, &mut loads);
        seq_inbox.extend(out.emitted.iter().cloned());
        self.observed.extend(out.emitted);

        // 2. Rules -> script function calls
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

        // 3. Script systems
        let so = self.scripts.run_systems(world, rng, tick).map_err(|e| e.to_string())?;
        collect_loads(&so.events, &mut loads);
        seq_inbox.extend(so.events.iter().cloned());
        self.observed.extend(so.events.iter().cloned());
        self.carryover.extend(so.events);

        // 4. Animation frame advance (the engine exclusively owns the write access to Sprite.image — animation is never "interrupted" by other logic,
        //    the only proper way to switch animation is to change Anim.clip)
        let anim_events = advance_animations(world, &self.animations)?;
        self.observed.extend(anim_events.iter().cloned());
        self.carryover.extend(anim_events);

        // 4.5 Sequence advance (generic timeline): an engine system at the same level as animation, state fully in the Sequence
        //     component (snapshot/replay safe), drives generic verbs by the relative start tick. Sequence-emitted
        //     load-scene/play-sound go through the same tail pipeline as rules, so this runs before scene switches.
        let seq_events = advance_sequences(world, &self.sequences, &self.schema, &seq_inbox, tick)?;
        collect_loads(&seq_events, &mut loads);
        self.observed.extend(seq_events.iter().cloned());
        self.carryover.extend(seq_events);

        // 4.6 UI layout (generic controls): an engine system at the same level as animation/sequence. Runs after sequence advance —
        //     sequences may spawn/modify UI nodes, layout needs to see this tick's final UI tree. The dirty flag guarantees
        //     zero recompute for static UI; the solved result is written back to Ui.rx/ry/rw/rh (entering hash and save, snapshot safe).
        //     The reference viewport is fixed ([`UI_REFERENCE_VIEWPORT`]) — layout state is decoupled from render resolution,
        //     cross-machine deterministic; at render time CPU/GPU each re-solve with the real resolution (pure function, same logic).
        advance_ui_layout(world, UI_REFERENCE_VIEWPORT)?;

        // 4.7 UI interaction (focus navigation + click activation, 1.2): runs after layout — picking/focus geometry needs to read
        //     this tick's solved Ui.rx/ry/rw/rh (reference frame 1920×1080). Consumes this tick's
        //     ui-up/down/left/right/confirm input and ui-click replies (coordinates already normalized to the reference frame),
        //     updates Button.state / UiRoot.focus / Button.press_t (all in components = snapshot/recording safe),
        //     activated buttons emit `ui-activate {id, action}` for rules/sequences to handle (UI does not bake in domain actions).
        //     ui-activate is not load-scene, enters carryover this tick, rules continue the chain next tick —
        //     the same cross-tick convention as sequence emit, deterministic and replay-consistent.
        let ui_events = advance_ui_interaction(world, &seq_inbox, UI_REFERENCE_VIEWPORT)?;
        collect_loads(&ui_events, &mut loads); // ui-activate will not be load-scene, but unified contract
        self.observed.extend(ui_events.iter().cloned());
        self.carryover.extend(ui_events);

        // 4.8 Theme application: resolve the theme background color corresponding to Button.state into Panel.color — render only reads
        //     Panel.color, the render layer does not depend on the theme table (code only hands off data). Panel.color is a deterministic
        //     state write (entering hash), not a render decoration. The press-feedback scale/modulate is the render-side
        //     pure-function decoration that reads Button.press_t (does not enter state, does not modify layout).
        apply_ui_theme(world, &self.themes)?;

        // 5. Scene switch (must execute inside the deterministic pipeline, so replay reproduces it at the same tick)
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

    /// Take away the events emitted by rules/scripts this tick (for control plane observation).
    fn drain_observed(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.observed)
    }

    /// Input action vocabulary: introspected from the rule set (the same `input_actions` as the playtest SceneView).
    /// Runtime is rule-like logic and can reach `self.rules`, so returns non-empty — the dispatcher's
    /// `dyn GameLogic` cannot reach the rule engine, this hook hands the action vocabulary out and merges it into describe.
    fn available_actions(&self) -> Vec<(String, Vec<String>)> {
        vitric_rules::input_actions(&self.rules.rules)
            .into_iter()
            .map(|ia| (ia.action, ia.phases))
            .collect()
    }

    /// Hot reload: re-read rules + scripts from disk, rebuild as a whole and atomically replace;
    /// any step failure keeps the old logic untouched (no half-dead state).
    /// Note: schema/scene changes are not in hot reload's scope (they define the world's shape, changes require restart).
    fn reload(&mut self) -> Result<serde_json::Value, String> {
        let root = self.root.clone().ok_or("该运行时没有项目目录，无法热重载")?;
        let project = Project::load(&root).map_err(|r| r.to_string())?;
        let fresh = Runtime::build(&project)?;
        self.rules = fresh.rules;
        self.scripts = fresh.scripts;
        // carryover holds pure-data events, safe across reloads, retained
        Ok(serde_json::json!({
            "reloaded": ["rules", "scripts"],
            "note": "schema/场景的改动不走热重载，需要重启进程",
            "rules": self.rules.rules.rules.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            "systems": self.scripts.systems.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            "fns": self.scripts.fns.clone(),
        }))
    }

    /// carryover (events emitted by scripts last tick that have not yet entered rules) is cross-tick state,
    /// if not snapshotted, the first tick after restore would have a different event stream than the original trajectory.
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

/// Animation system: advance frames each tick. State is fully in the Anim component (snapshot/replay safe):
/// `clip` current clip (empty string = not playing), `prev` used by the engine to detect switches, `t` tick count within the clip,
/// `done` whether a non-looping clip has finished playing (an `anim-finished` event is emitted once when it finishes).
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
            continue; // empty string = no animation, Sprite.image is returned to the user
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
            // Switch clip: play from the start
            t = 0;
            done = false;
            world.set_field(id, "Anim.prev", json!(clip_name)).map_err(|e| e.to_string())?;
        } else {
            t += 1;
        }

        // Integer arithmetic preserves determinism: tick t corresponds to frame t*fps/60
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

/// Sequence system: a generic timeline primitive, advances active `Sequence` components each tick.
///
/// State is fully in the `Sequence` component (snapshot/replay safe): `track` (which static sequence is referenced),
/// `cursor` (the index of the next entry to drive), `start` (start tick, -1 = not started yet),
/// `wait` (the named event being waited on, empty string = not waiting), `id` (the id carried by the completion event, empty = use the track name).
/// Static tracks (entry arrays) live in `catalog`, do not enter components, do not enter per-instance snapshots.
///
/// Semantic contract:
/// - **Empty-scene zero cost**: early-return each tick when there are no Sequence components;
/// - The first tick processed stamps `start` from -1 to the current tick (elapsed=0);
/// - Each tick drives all entries with `at ≤ elapsed` and index ≥ cursor (in index order),
///   the cursor advances until it hits `wait` (barrier) or there are no due entries;
/// - `wait`: the cursor stops at the barrier, until that named event appears in `inbox`;
/// - **skip**: a `skip` input in the inbox → finalize the terminal state of all remaining entries (ignoring at
///   and wait), then emit the completion event. skip is an input, enters the recording, replay-consistent;
/// - When reaching the end, emits a `sequence-finished {id, track}` event, the sequence entity auto-despawns;
/// - `tween` action = spawn a Tween component and hand it to sim's advance_tweens to execute (no duplication).
///
/// Sequences use `emit` (including `sound`→play-sound) to decouple from "scenes": switching a scene is just emitting load-scene,
/// the project's rules pick up load-scene; the sequence itself knows nothing about "scene" / "level" / "cutscene".
pub fn advance_sequences(
    world: &mut World,
    catalog: &BTreeMap<String, Sequence>,
    schema: &Schema,
    inbox: &[Event],
    tick: u64,
) -> Result<Vec<Event>, String> {
    let ids = world.query(&["Sequence"]);
    if ids.is_empty() {
        return Ok(Vec::new()); // Empty-scene zero cost: no sequences playing, zero allocation zero traversal
    }
    // skip is an input: it is a {"action":"skip","phase":"pressed"} input event in the recording
    let skip = inbox
        .iter()
        .any(|e| e.name == "input" && e.data.get("action").and_then(|v| v.as_str()) == Some("skip"));

    let mut events = Vec::new();
    for id in ids {
        if !world.is_alive(id) {
            continue; // A previous sequence's action may have despawned it
        }
        let track_name = world
            .get_field(id, "Sequence.track")
            .map_err(|e| e.to_string())?
            .as_str()
            .ok_or_else(|| format!("实体 {id} 的 Sequence.track 必须是文本（序列名）"))?
            .to_string();
        if track_name.is_empty() {
            continue; // empty track = no sequence mounted, skip
        }
        let seq = catalog.get(&track_name).ok_or_else(|| {
            format!(
                "实体 {id} 的 Sequence.track {track_name:?} 没有定义。已定义序列: [{}]。\
                 提示：序列在清单 sequences 列表的文件里定义",
                catalog.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?;

        // Start stamp: stamp start from -1 to the current tick (entering component = entering hash and save)
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

        // barrier: waiting for some event, only released when it appears in this tick's inbox (skip also releases)
        if !waiting.is_empty() {
            let released = skip || inbox.iter().any(|e| e.name == waiting);
            if released {
                waiting.clear();
            } else {
                // Still waiting: state unchanged, continue to the next sequence
                continue;
            }
        }

        // Drive entries. Under skip, ignores at/wait and finalizes all remaining terminal states in one pass.
        let finished = loop {
            if cursor >= seq.steps.len() {
                break true; // reached the end
            }
            let step = &seq.steps[cursor];
            if !skip && step.at > elapsed {
                break false; // not due yet, wait for the next tick
            }
            if step.kind == "wait" && !skip {
                // Hit a barrier: record the event name to wait for, advance the cursor past it, stop here this tick
                let name = step.action.get("wait").and_then(|v| v.as_str()).unwrap_or("");
                waiting = name.to_string();
                cursor += 1;
                break false;
            }
            // Execute the action (wait is a no-op under skip, just skip over)
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
            // Write back the minimal playback state (cursor + barrier flag)
            world.set_field(id, "Sequence.cursor", json!(cursor as i64)).map_err(|e| e.to_string())?;
            world.set_field(id, "Sequence.wait", json!(waiting)).map_err(|e| e.to_string())?;
        }
    }
    Ok(events)
}

/// UI layout system (each tick, an engine system at the same level as animation/sequence). **Dirty flag + one tree traversal**:
/// only truly solves when the UI tree's structure/size (or viewport size) has changed, writing each Ui node's solved rectangle
/// back to `Ui.rx/ry/rw/rh` (entering components = entering hash and save, snapshot/recording safe).
///
/// Dirty check uses `UiRoot.layout_hash`: the current input hash ([`vitric_render::layout_input_hash`],
/// excluding the rx/ry/rw/rh outputs themselves) equals the last stored value = static = skip recompute (the
/// "static UI plays N ticks, layout recomputes 0 times" outcome). Only when the hash changes does it solve + write back + stamp a new hash.
///
/// Performance: no UI in the scene (no UiRoot) = first line zero-cost early-return (zero allocation zero traversal).
/// `viewport`: the viewport size (pixels) the layout references. Window/screenshot use the real resolution, headless logic tests use
/// a conventional reference resolution — layout is relative ratios + pixel offsets, the reference size is hashed and tracked too.
pub fn advance_ui_layout(world: &mut World, viewport: (u32, u32)) -> Result<(), String> {
    let roots = world.query(&["UiRoot"]);
    if roots.is_empty() {
        return Ok(()); // Empty UI zero cost: no UiRoot, zero allocation zero traversal
    }
    let (vw, vh) = viewport;
    let want = vitric_render::layout_input_hash(world, vw, vh);
    // Last stamped hash (taken from the first UiRoot's layout_hash field, missing/different = dirty)
    let root = roots[0];
    let have = world
        .get_field(root, "UiRoot.layout_hash")
        .ok()
        .and_then(|v| v.as_str())
        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok());
    if have == Some(want) {
        return Ok(()); // Static: input unchanged, the layout result is what was last written back, skip recompute
    }
    // Dirty: do a real solve pass, write the rectangles back to each node
    let layout = vitric_render::solve_layout(world, vw, vh)?;
    for (id, r) in &layout {
        world.set_field(*id, "Ui.rx", json!(r.x)).map_err(|e| e.to_string())?;
        world.set_field(*id, "Ui.ry", json!(r.y)).map_err(|e| e.to_string())?;
        world.set_field(*id, "Ui.rw", json!(r.w)).map_err(|e| e.to_string())?;
        world.set_field(*id, "Ui.rh", json!(r.h)).map_err(|e| e.to_string())?;
    }
    // Stamp a new hash (only when UiRoot has a layout_hash field — if the schema does not declare it, do not write,
    // degrading to "solve every tick", still correct, just does not save that one pass; declared gets the dirty flag)
    if world.has_component(root, "UiRoot") && world.get_field(root, "UiRoot.layout_hash").is_ok() {
        world
            .set_field(root, "UiRoot.layout_hash", json!(format!("0x{want:016x}")))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// UI interaction system (focus navigation + click activation, 1.2). An engine system at the same level as animation/sequence/layout,
/// **runs after layout** — focus geometry and click picking both read this tick's solved `Ui.rx/ry/rw/rh`
/// (reference frame 1920×1080).
///
/// All state enters components (snapshot/recording safe):
/// - `UiRoot.focus`: the entity name of the currently focused button ("" = no focus, the engine lands on the first focusable button on first interaction);
/// - `Button.state`: normal/focused/pressed/disabled;
/// - `Button.press_t`: press feedback timer (-1 = not in feedback, 0..PRESS_TICKS incrementing; analytical, not accumulative).
///
/// Consumes this tick's `inbox`:
/// - `input {action: "ui-up"|"ui-down"|"ui-left"|"ui-right"}` → move focus by layout adjacency;
/// - `input {action: "ui-confirm"}` → activate the currently focused button;
/// - `ui-click {nx, ny, button}` → screen normalized coordinates (0..1) converted to the reference frame (×1920/×1080)
///   then check which button rectangle it falls into, hit = activate (also moves focus to it).
/// - `ui-click-by-name {name, button}` → activate a Button by its scene name (fail-fast on
///   missing name / no Button / Disabled). Layout-independent alternative to `ui-click`.
///
/// **Coordinate conversion chain (key wiring)**: layout output rx/ry/rw/rh are pixel rectangles in the 1920×1080 reference frame
/// (entering hash, decoupled from render resolution); the click source is physical screen/window pixels. The window/RPC injection side first
/// normalizes the click by viewport size to 0..1 ([`vitric_control::inject_ui_click`]), this system multiplies it back by the
/// 1920×1080 reference frame — so no matter the real resolution, hit testing always targets the same reference-frame rectangles,
/// replay (the recording stores normalized coordinates) is bit-for-bit consistent. Do **not** compare world coordinates to UI rectangles (those are two different frames).
///
/// Activate = button set to pressed + press_t=0 + emit `ui-activate {id, action}` (handled by rules/sequences).
/// disabled buttons are not focusable and do not respond to clicks/confirm (contract section 4).
///
/// Performance: focus ring = one pass over query Button (O(button count), not a full-table scan); empty UI (no UiRoot)
/// first line zero-cost early-return.
pub fn advance_ui_interaction(
    world: &mut World,
    inbox: &[Event],
    viewport: (u32, u32),
) -> Result<Vec<Event>, String> {
    if world.query(&["UiRoot"]).is_empty() {
        return Ok(Vec::new()); // Empty UI zero cost: no UiRoot, zero allocation zero traversal
    }
    let root = world.query(&["UiRoot"])[0];
    let mut events = Vec::new();

    // Focus ring: all entities with Button (query slot order = deterministic), read state + rectangle.
    // disabled does not enter the focusable set (focus navigation skips it), but stays in the table (clicking it must be explicitly ignored).
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

    // 1) Press feedback timer advance (analytical, not accumulative: press_t is a tick count, scale/modulate is computed from it in one step).
    //    When due (press_t ≥ PRESS_TICKS), fall back to normal (focus state is uniformly reset in step 3, here only the feedback is cleared).
    for b in &btns {
        let pt = world
            .get_field(b.id, "Button.press_t")
            .ok()
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);
        if pt >= 0 {
            let next = pt + 1;
            if next as u64 >= vitric_render::PRESS_TICKS {
                // Feedback ended: clear timer + back to normal (if it's the focused button, step 3 will set it back to focused)
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

    // Current focus: UiRoot.focus stores the entity name. Empty/invalid → land on the first focusable button (deterministic fallback).
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
    // Index in btns of the current focus (found by name). Empty name/not found = None.
    let mut focus_idx: Option<usize> = if focus_name.is_empty() {
        None
    } else {
        btns.iter().position(|b| world.name_of(b.id) == Some(focus_name.as_str()))
    };
    // Focus points to disabled / nonexistent → fall back to the first focusable
    if focus_idx.is_none_or(|i| btns[i].state == vitric_render::ButtonState::Disabled) {
        focus_idx = focusable.first().copied();
    }

    // 2) Direction input: move focus (by layout adjacency, only within the focusable set).
    //    Multiple direction inputs in the same tick: applied one by one in arrival order (deterministic, inbox is already in fixed order).
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
            continue; // only move focus on the press edge (release does nothing)
        }
        let Some(dir_name) = action.strip_prefix("ui-") else { continue };
        let Some(dir) = vitric_render::Dir::parse(dir_name) else { continue };
        let Some(cur) = focus_idx.and_then(|fi| focusable.iter().position(|&x| x == fi)) else {
            // No focus yet: direction key lands on the first focusable first (standard menu feel)
            focus_idx = focusable.first().copied();
            continue;
        };
        let next_in_ring = vitric_render::navigate(&focus_geom, cur, dir);
        focus_idx = Some(focusable[next_in_ring]);
    }

    // 3) Reset focus state: among focusable buttons, the focused one is set to focused, the rest non-pressed ones to normal.
    //    pressed (in feedback) is not overwritten by the focus reset — only falls back to normal when feedback ends (step 1).
    let focus_id = focus_idx.map(|i| btns[i].id);
    for b in &btns {
        if b.state == vitric_render::ButtonState::Disabled {
            continue; // disabled state is fixed, not touched by focus logic
        }
        // Real state in the current component (step 1 may have just modified press_t/state, re-read)
        let cur = world
            .get_field(b.id, "Button.state")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "normal".to_string());
        if cur == "pressed" {
            continue; // in feedback, do not overwrite
        }
        let want = if Some(b.id) == focus_id { "focused" } else { "normal" };
        if cur != want {
            world.set_field(b.id, "Button.state", json!(want)).map_err(|e| e.to_string())?;
        }
    }
    // Write the focus name back to UiRoot (enters hash and save)
    let new_focus_name = focus_id.and_then(|id| world.name_of(id)).unwrap_or("").to_string();
    if new_focus_name != focus_name {
        world.set_field(root, "UiRoot.focus", json!(new_focus_name)).map_err(|e| e.to_string())?;
    }

    // 4) Confirm key: activate the currently focused button.
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

    // 5) Click: screen normalized coordinates → reference frame 1920×1080 → hit button rect → activate.
    let (vw, vh) = viewport;
    for e in inbox {
        if e.name != "ui-click" {
            continue;
        }
        let nx = e.data.get("nx").and_then(|v| v.as_f64());
        let ny = e.data.get("ny").and_then(|v| v.as_f64());
        let (Some(nx), Some(ny)) = (nx, ny) else { continue };
        // Normalized → reference frame pixels (same coordinate system as rx/ry/rw/rh)
        let px = nx * vw as f64;
        let py = ny * vh as f64;
        // Hit test: query in reverse order (later-drawn paint on top, prioritize hit), disabled does not respond.
        let hit = btns.iter().rev().find(|b| {
            b.state != vitric_render::ButtonState::Disabled
                && px >= b.rect.x
                && px < b.rect.x + b.rect.w
                && py >= b.rect.y
                && py < b.rect.y + b.rect.h
        });
        if let Some(b) = hit {
            // A click hit also moves focus to it (unify point and focus), then activate
            let name = world.name_of(b.id).unwrap_or("").to_string();
            world.set_field(root, "UiRoot.focus", json!(name)).map_err(|e| e.to_string())?;
            activate_button(world, b.id, &b.action, &mut events)?;
        }
    }

    // 6) Click by name: activate a Button by its scene name. Stricter than coordinate clicks —
    //    fail-fast on missing name / no Button component / Disabled. By-name is an explicit
    //    semantic request, so silent miss would hide script bugs. Records store the name
    //    (deterministic), replays inject the same name at the same tick.
    for e in inbox {
        if e.name != "ui-click-by-name" {
            continue;
        }
        let Some(name) = e.data.get("name").and_then(|v| v.as_str()) else { continue };
        let id = world.entity(name).map_err(|err| err.to_string())?;
        // Must have a Button component — otherwise a script could "click" arbitrary entities.
        if !world.has_component(id, "Button") {
            return Err(format!(
                "ui-click-by-name: 实体 {name:?} 没有 Button 组件（不能被点击）"
            ));
        }
        // Disabled buttons refuse clicks (parity with coordinate clicks' disabled filter).
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
        if state == vitric_render::ButtonState::Disabled {
            return Err(format!(
                "ui-click-by-name: 按钮 {name:?} 当前是 Disabled，不能点击"
            ));
        }
        let action = world
            .get_field(id, "Button.action")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        // Move focus to the named button (unify with coordinate clicks), then activate.
        world.set_field(root, "UiRoot.focus", json!(name)).map_err(|err| err.to_string())?;
        activate_button(world, id, &action, &mut events)?;
    }

    Ok(events)
}

/// Activate a button: set pressed + start press feedback timer + emit `ui-activate {id, action}`.
/// An empty action is still emitted (check already rejects empty actions, runtime does not double-reject — explicit behavior).
fn activate_button(
    world: &mut World,
    id: EntityId,
    action: &str,
    events: &mut Vec<Event>,
) -> Result<(), String> {
    world.set_field(id, "Button.state", json!("pressed")).map_err(|e| e.to_string())?;
    // press_t=0: this tick is feedback frame 0 (the analytical press_scale(0)=1, starts scaling from next tick)
    world.set_field(id, "Button.press_t", json!(0)).map_err(|e| e.to_string())?;
    let name = world.name_of(id).map(String::from).unwrap_or_else(|| id.to_string());
    events.push(Event::new("ui-activate", json!({"id": name, "action": action})));
    Ok(())
}

/// Theme application system: writes each Button's `state`-corresponding theme background color into its `Panel.color`.
/// The render layer only reads Panel.color (does not know the theme table) — theme is at assemble time, state is in components, solving happens here,
/// render only draws the solved color (code just hands off data).
///
/// `Panel.color` is **deterministic state** (enters hash and save), not render decoration — same state same color,
/// replay consistent. The press-feedback scale/modulate is the pure-function decoration that the render side reads from `Button.press_t`.
///
/// No theme (Button has no theme field or references empty) = skip this button (keep the Panel.color hardcoded in the scene,
/// do not impose a theme). Referencing a non-existent theme is already a red light at check time; here the runtime explicitly errors out as a fallback.
pub fn apply_ui_theme(
    world: &mut World,
    themes: &BTreeMap<String, vitric_data::Theme>,
) -> Result<(), String> {
    let buttons = world.query(&["Button", "Panel"]);
    if buttons.is_empty() {
        return Ok(()); // no buttons = zero cost
    }
    for id in buttons {
        let theme_name = world
            .get_field(id, "Button.theme")
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        if theme_name.is_empty() {
            continue; // no theme referenced: keep the scene-hardcoded Panel.color
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
        // Only write when changed (avoids pointless "dirty" — though writing the same value does not change the hash, one less write is cleaner)
        let cur = world.get_field(id, "Panel.color").ok().and_then(|v| v.as_str().map(String::from));
        if cur.as_deref() != Some(style.bg.as_str()) {
            world.set_field(id, "Panel.color", json!(style.bg)).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Execute one action in a sequence (the action set has already been type/field-validated by vitric-data).
/// All mirror existing engine generic verbs, no new semantics invented. Component values uniformly go through schema normalization
/// (number always stored in float form) — representation is unique, state hash is unaffected by the writer (same convention as scenes/rules).
fn exec_seq_action(
    world: &mut World,
    schema: &Schema,
    step: &SeqStep,
    events: &mut Vec<Event>,
) -> Result<(), String> {
    let obj = step.action.as_object().expect("校验层已确保动作是对象");
    match step.kind.as_str() {
        // tween: start a Tween component, hand off to sim's advance_tweens to execute (zero duplication)
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
            comp.insert("start".into(), json!(-1)); // stamped by the tween system at start time
            comp.insert("id".into(), spec.get("id").cloned().unwrap_or_else(|| json!("")));
            let tw = world.spawn();
            let value = normalize_component(schema, "Tween", Value::Object(comp))?;
            world.set_component(tw, "Tween", value).map_err(|e| e.to_string())?;
        }
        // set: instant field set (mirrors rule set)
        "set" => {
            let target = obj["set"].as_str().expect("校验过");
            let to = obj.get("to").expect("校验过").clone();
            let (id, path) = resolve_field(world, target)?;
            world.set_field(id, &path, to).map_err(|e| e.to_string())?;
        }
        // spawn: spawn entity (mirrors rule spawn)
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
        // despawn: destroy entity (mirrors rule despawn)
        "despawn" => {
            let target = obj["despawn"].as_str().expect("校验过");
            let id = resolve_entity(world, target)?;
            world.despawn(id).map_err(|e| e.to_string())?;
        }
        // emit: emit an event for rules to chain on (the front door decoupled from the scene)
        "emit" => {
            let name = obj["emit"].as_str().expect("校验过");
            let data = obj.get("data").cloned().unwrap_or_else(|| json!({}));
            events.push(Event::new(name, data));
        }
        // sound: play a sound (mirrors audio, translated into a play-sound event — same audio channel as rules)
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

/// Normalize a component value by schema (number→float, fill defaults). Unknown component name is an explicit error.
/// Components spawned/tweened by sequences go through the same normalization as scenes/rules, so state hashes match.
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

/// Parse an entity reference: "@name" / "name" / "e3v1" handle. Entity references in sequence actions go through this.
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

/// Parse "entity.field path" (e.g. "@subtitle.Text.content") into (handle, field path).
fn resolve_field(world: &World, target: &str) -> Result<(EntityId, String), String> {
    let (ent, path) = target.split_once('.').ok_or_else(|| {
        format!("目标 {target:?} 缺少字段路径，写法 \"@实体名.组件.字段\"")
    })?;
    Ok((resolve_entity(world, ent)?, path.to_string()))
}

/// TypeScript → JavaScript (esbuild subprocess, only strips types, no bundling).
/// esbuild lookup order: env var ESBUILD_BIN → esbuild on PATH.
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

/// Recursively scan rule documents for `{"emit": "play-sound"|"play-music", "data": {"sound": "literal"}}`
/// sound/music references (the two share the same literal-name rules, including path-escape validation).
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
                    // The reference value may also be a runtime path like "event.xxx", only validate the literal filename
                    let is_ref = sound.starts_with("self.")
                        || sound.starts_with("other.")
                        || sound.starts_with("event.")
                        || sound.starts_with('@');
                    if !is_ref {
                        // Same rule as runtime: must not escape the sounds/ directory
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

/// Scan the script **source text** for literal .png texture references; each reference must exist in the asset store.
///
/// Recognized forms (real-world incident prototype: a script ctx.spawn used a non-existent "dust.png", check was green,
/// the game crashed hard mid-render):
/// - JS object literal: `image: "dust.png"` / `image: 'dust.png'`
/// - JSON-style quoted key: `"image": "dust.png"` / `'image': 'dust.png'`
///
/// Honest limitation — this is a literal lint, not data-flow analysis:
/// dynamic concatenation (`"dust_" + i + ".png"`), indirect variable references cannot be scanned; passing check does not mean
/// the image definitely exists at runtime. So the error message advises "use literal names whenever possible" so the lint can hold.
/// What is scanned is the on-disk original (.ts is scanned directly without transpilation — esbuild only strips types, does not touch string literals).
fn scan_script_image_refs(src: &str, file: &str, assets: &vitric_render::Assets, missing: &mut Vec<String>) {
    let bytes = src.as_bytes();
    let mut from = 0;
    while let Some(pos) = src[from..].find("image") {
        let start = from + pos;
        let mut i = start + "image".len();
        from = i; // regardless of whether this match holds, next round continues after "image"
        // Left boundary of the key: the previous char must not be an identifier character (rule out bgimage / my_image / e.image)
        let prev = src[..start].chars().next_back();
        if prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '.') {
            continue;
        }
        // Quoted key ("image" / 'image'): the closing quote must match the opening quote and immediately follow the key name
        if let Some(q @ ('"' | '\'')) = prev {
            if bytes.get(i) != Some(&(q as u8)) {
                continue; // ordinary string content like "image arts", not a key
            }
            i += 1;
        }
        // Colon (whitespace allowed on both sides)
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
        // The value must be a single-segment string literal closed by the same quote (concatenation/newline mid-way means it is not a literal reference)
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

/// Recursively scan rule documents for `{"spawn": {"components": {"Sprite": {"image": "literal"}}}}`
/// texture references (same approach as [`scan_sound_refs`]). Runtime references (self./other./event./@)
/// are not statically validated — same exemption rule as sound scanning.
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

/// `vitric check`: only validates data, does not run. Returns a human/AI-readable complete report.
pub fn check(dir: &Path) -> Result<Value, String> {
    let project = Project::load(dir).map_err(|r| r.to_string())?;
    let runtime = Runtime::build(&project)?;
    // Instantiate the entry scene into a throwaway world to surface landing-phase errors too
    let mut sim = Sim::new(project.manifest.seed);
    vitric_data::instantiate_scene(project.entry_scene(), &project.schema, &mut sim.world)
        .map_err(|r| r.to_string())?;
    // Assets: validate on load (bad image / oversized), then check that all images referenced by scenes exist
    let mut assets = vitric_render::Assets::load_dir(&dir.join("assets"))?;
    // Font: if the manifest declares a font, actually parse it at check time (existence is already checked by Project::load,
    // here we catch "file exists but is not a valid TTF")
    if let Some(font_rel) = &project.manifest.font {
        assets.load_font(&dir.join(font_rel))?;
    }
    let mut missing = Vec::new();
    // Instantiate **every** scene in the manifest + check references — load-scene can switch to it at any time,
    // a bad reference in a non-entry scene not caught at check time would only blow up at switch time
    for (rel, scene) in &project.scenes {
        let mut scratch;
        let world: &World = if rel == &project.manifest.entry {
            &sim.world // entry already instantiated (entities/initial_hash in the report use it)
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
        // UI Panel.image referenced sprites must all be in the asset store (same convention as Sprite.image)
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
        // All clips referenced by Anim.clip in the scene must be defined
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
        // Button.theme referenced themes must be defined in the manifest themes (value-level state/action validation is in
        // vitric-data's validate_ui_components; theme-name existence needs a project-level table, lives here)
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
    // Animations: all frame images referenced by clips must be in the asset store
    for (cname, clip) in &project.animations {
        for frame in &clip.frames {
            if assets.image(frame).is_none() {
                missing.push(format!("动画片段 {cname:?} 引用了不存在的帧图 {frame:?}"));
            }
        }
    }
    // Sequences: structure/action fields are already validated by Project::load (vitric-data); here we add cross-file references —
    // spawn's literal textures, sound's literal sound effects, emit "load-scene" target scenes must all really exist
    for seq in project.sequences.values() {
        for step in &seq.steps {
            scan_rule_image_refs(&step.action, &seq.file, &assets, &mut missing);
            scan_sound_refs(&step.action, &seq.file, &dir.join("sounds"), &mut missing);
            // literal sound effect of the sound action
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
            // emit "load-scene" target scene must be in the manifest scenes list
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
    // Sounds: literal play-sound effect files referenced in rules must exist
    for (file, doc) in &project.rules {
        scan_sound_refs(doc, file, &dir.join("sounds"), &mut missing);
    }
    // Texture literal references: .png hardcoded in script source (ctx.spawn etc.) and in rule spawn actions
    // must be in the asset store — entities dynamically spawned outside the scene are also not allowed to reference non-existent images.
    // Limitations of the literal lint are documented in each scan function (dynamic concatenation cannot be scanned)
    for (file, src) in &project.scripts {
        scan_script_image_refs(src, file, &assets, &mut missing);
    }
    for (file, doc) in &project.rules {
        scan_rule_image_refs(doc, file, &assets, &mut missing);
    }
    // Frame-import products (vitric assets --frames produces *-atlas.json sidecars): atlas exists,
    // frame table valid, uv/rect in-bounds, referenced frame images exist, compression product header valid. Pure addition —
    // old projects without frame-import products pay zero cost here (no *-atlas.json in assets/).
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
        // Text-rendering path observable: with a font reports the path, without one explicitly says bitmap
        "font": project.manifest.font.clone().unwrap_or_else(|| "内嵌 8x8 点阵".to_string()),
        "initial_hash": format!("{:#018x}", sim.world.state_hash()),
    }))
}

/// Find frame-import atlas sidecars (`*-atlas.json`) at the top level of assets/, returns names relative to assets/.
/// Non-existent / unreadable = empty (legal: projects that have never used --frames). Only scans the top level (products land there).
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
    out.sort(); // deterministic error order
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scan(doc: Value) -> Vec<String> {
        let mut missing = Vec::new();
        // Point at a definitely-nonexistent directory: all literal references should report "does not exist"
        scan_sound_refs(&doc, "rules/test.json", Path::new("/nonexistent/sounds"), &mut missing);
        missing
    }

    #[test]
    fn available_actions_surfaces_rule_input_actions() {
        // Actually boot a project with input rules (jump: left/right/space/up four actions, no scripts so no esbuild needed),
        // assert GameLogic::available_actions returns the input actions introspected from rules — this is exactly the vocabulary
        // merged into the output when describe cannot reach the rules.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/jump");
        let (_sim, rt) = Runtime::boot(&dir).expect("jump 示例应能 boot");
        let actions = rt.available_actions();
        let names: Vec<&str> = actions.iter().map(|(a, _)| a.as_str()).collect();
        for want in ["left", "right", "space", "up"] {
            assert!(names.contains(&want), "应含动作 {want}: {names:?}");
        }
        // distinct: each action appears only once (even if there are pressed+released rules)
        assert_eq!(
            names.iter().filter(|n| **n == "left").count(),
            1,
            "left 去重只一次: {names:?}"
        );
        // left has pressed+released rules → phases collect both phases
        let left = actions.iter().find(|(a, _)| a == "left").unwrap();
        assert!(
            left.1.contains(&"pressed".to_string()) && left.1.contains(&"released".to_string()),
            "left 应收到 pressed+released 两 phase: {:?}",
            left.1
        );
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
        // Path escape is an explicit "illegal" error, not "file does not exist"
        let missing =
            scan(json!({"emit": "play-music", "data": {"sound": "../secret.ogg"}}));
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("不合法"), "要点明不合法而非不存在: {}", missing[0]);
    }

    #[test]
    fn scan_still_covers_play_sound_and_skips_runtime_refs() {
        // play-sound old rule works as before; runtime references (event.* etc.) are not statically validated
        let missing = scan(json!([
            {"emit": "play-sound", "data": {"sound": "coin.wav"}},
            {"emit": "play-music", "data": {"sound": "event.bgm"}},
            {"emit": "stop-music", "data": {}},
        ]));
        assert_eq!(missing.len(), 1, "只有 coin.wav 该被报: {missing:?}");
        assert!(missing[0].contains("coin.wav"));
    }

    // ---- Script/rule literal texture reference scanning ----

    fn scan_src(src: &str) -> Vec<String> {
        let mut missing = Vec::new();
        // Empty asset store: all literal references should be reported as "does not exist"
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
        // Both JSON-style key + single-quote forms are recognized
        let missing = scan_src(r#"const a = { "image": "a.png" }; const b = { 'image': 'b.png' };"#);
        assert_eq!(missing.len(), 2, "{missing:?}");
        assert!(missing[0].contains("a.png") && missing[1].contains("b.png"));
    }

    #[test]
    fn script_scan_skips_dynamic_and_non_keys() {
        // Dynamic concatenation is a documented limitation: not reported (no false positives allowed — false positives would teach the agent to ignore check)
        assert!(scan_src(r#"ctx.spawn({ Sprite: { image: "dust_" + i + ".png" } });"#).is_empty());
        // Other identifiers colliding with the image substring / property reads: none are keys
        assert!(scan_src(r#"const bgimage: string = "x.png"; e.Sprite.image = takeFrom(pool);"#).is_empty());
        // Non-.png literals are out of this lint's scope
        assert!(scan_src(r#"spawn({ image: "sheet.jpg" })"#).is_empty());
        // Known over-report boundary (text-level scanning does not parse syntax): text that looks like `image: 'x.png'`
        // inside comments/strings is also treated as a reference. Lock this behavior — if it changes, the scanner semantics changed
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
