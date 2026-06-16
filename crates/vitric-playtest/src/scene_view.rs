//! 场景视图（Scene View）——引擎为当前世界态自动吐的一份「代理所见」。
//!
//! 三部分（见设计稿一节）：observation（剔除纯装饰后的玩法状态投影）、
//! actions（游戏声明的输入词汇）、done（终止判定）。**纯投影**：只读世界，
//! 绝不改 world、不进哈希、不影响确定性——所以它接收的是 `&World`/`&Engine`。

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use vitric_ecs::World;
use vitric_rules::{Engine, Trigger};

use crate::config::{
    DerivedSpec, DistanceMetric, ObservationConfig, PlaytestConfig, TerminalOverride,
};

/// 一个可注入的动作（输入词汇里的一项）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub action: String,
    /// "pressed" | "released"
    pub phase: String,
}

/// 一局的结局类型。第 1 阶段只产出 Win/Lose/Timeout，软锁/不可达等留给后续阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// 通关（命中胜利类终止事件）。
    Win,
    /// 失败/死亡（命中失败类终止事件）。
    Lose,
    /// 跑满 max_ticks 仍未终止。
    Timeout,
}

/// 哪些事件名算「这局到此为止」。默认：胜利集合 + 失败集合 + 前缀集合
/// （`ending-*` 之类结局事件）。游戏可在后续阶段通过 playtest.json 覆盖。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSpec {
    /// 命中即判 Win 的事件名。
    pub win_events: Vec<String>,
    /// 命中即判 Lose 的事件名。
    pub lose_events: Vec<String>,
    /// 命中即判结局（按前缀，统一归到 Win——结局达成本身算「通到了一个尽头」）。
    pub ending_prefixes: Vec<String>,
}

impl Default for TerminalSpec {
    fn default() -> TerminalSpec {
        // jump 之类小游戏发的是 game-won；通用默认把常见胜负名都收进来，
        // 任何游戏开箱即能判出一个终止，不需要先写 playtest.json。
        TerminalSpec {
            win_events: ["win", "game-won", "victory", "level-complete"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            lose_events: ["lose", "game-over", "death", "dead"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ending_prefixes: vec!["ending".to_string()],
        }
    }
}

impl TerminalSpec {
    /// 套一份 `playtest.json` 的 terminal 覆盖：写了哪个集合就替换哪个，没写的回退到 self
    /// （= 默认集合）。覆盖是「替换」不是「叠加」——游戏声明了自己的胜负名就以它为准，
    /// 不再混进通用默认（否则 game-won 之类会误判）。
    pub fn apply_override(&self, ovr: &TerminalOverride) -> TerminalSpec {
        TerminalSpec {
            win_events: ovr.win_events.clone().unwrap_or_else(|| self.win_events.clone()),
            lose_events: ovr.lose_events.clone().unwrap_or_else(|| self.lose_events.clone()),
            ending_prefixes: ovr
                .ending_prefixes
                .clone()
                .unwrap_or_else(|| self.ending_prefixes.clone()),
        }
    }

    /// 把项目清单 `gates.playthroughs[].must_emit` 声明的通关事件名**追加**进
    /// `win_events`（在默认/已有集合基础上叠加，不替换；去重，原序保留、新名按入参序补到末尾）。
    /// 立场：每个项目其实已经声明了自己的权威通关事件（gate 门禁就用它判通关录像），
    /// 脚本/LLM 游戏（如 echo 的 `run-complete`）的胜利事件不在通用默认集里，
    /// 不并进来就会被误判"谁也通不了"。playtest.json 的 terminal 覆盖仍在本步之前先生效，
    /// 这一步只往 win 集合里**补**清单声明的事件，不动 lose/ending。
    pub fn with_manifest_must_emit<I, S>(&self, events: I) -> TerminalSpec
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut win = self.win_events.clone();
        for ev in events {
            let ev = ev.as_ref();
            if !win.iter().any(|n| n == ev) {
                win.push(ev.to_string());
            }
        }
        TerminalSpec {
            win_events: win,
            lose_events: self.lose_events.clone(),
            ending_prefixes: self.ending_prefixes.clone(),
        }
    }

    /// 一个事件名命中终止？命中返回对应结局，没命中返回 None。
    /// win/ending 都归 Win（达成结局=通到一个尽头），lose 归 Lose。
    pub fn classify(&self, event_name: &str) -> Option<Outcome> {
        if self.win_events.iter().any(|n| n == event_name) {
            return Some(Outcome::Win);
        }
        if self.lose_events.iter().any(|n| n == event_name) {
            return Some(Outcome::Lose);
        }
        if self.ending_prefixes.iter().any(|p| event_name.starts_with(p.as_str())) {
            return Some(Outcome::Win);
        }
        None
    }
}

/// 纯装饰组件：只为画面服务，不是玩法状态，投影时剔除（设计稿一节 render-only）。
/// 第 1 阶段先用一份常量清单兜底；后续阶段可按 schema 标注细化。
const DECORATIVE_COMPONENTS: &[&str] =
    &["Sprite", "Particle", "Emitter", "Bloom", "Ambient", "Anim", "Camera"];

fn is_decorative(component: &str) -> bool {
    DECORATIVE_COMPONENTS.contains(&component)
}

/// 一份「代理所见」。observation 是机器可读 JSON，策略和（后续）LLM 共用同一份。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneView {
    /// 当前相关状态：存活实体投影成 JSON，剔除纯装饰组件。
    pub observation: Value,
    /// 本刻可选的动作清单（游戏声明的输入词汇 × {pressed, released}）。
    pub actions: Vec<Action>,
    /// 终止判定：None=还在进行，Some=本局已到尽头。
    pub done: Option<Outcome>,
}

impl SceneView {
    /// 从世界 + 规则引擎 + 终止规格派生一份视图（**自动推**，无 config 覆盖）。
    /// `done` 始终为 None——终止是「事件命中」才知道的，由 session 在 step 后扫事件判，
    /// 不由静态世界态判。（静态世界投影不含「刚发生了 game-won」这种瞬时信息。）
    ///
    /// 等价于 `derive_with_config(.., &PlaytestConfig::default())`——默认 config 逐字节
    /// 还原本函数的输出（向后兼容由测试 `default_config_derive_byte_identical_to_plain_derive` 锁）。
    pub fn derive(world: &World, engine: &Engine, _terminal: &TerminalSpec) -> SceneView {
        SceneView {
            observation: project_observation(world),
            actions: derive_actions(engine),
            done: None,
        }
    }

    /// 带 `playtest.json` 覆盖的派生（设计稿一节「自动推 + 可选覆盖」、十一节第 6 条）。
    /// 仍是**纯投影**：只读世界/规则，不改 world、不进哈希、不影响确定性。
    ///
    /// observation 按 config 调整：先按 include/exclude 选组件、按 relabel 改人话名，
    /// 再把声明的派生量算进一个 `"derived"` 子对象（默认 config 不注入 derived 键——
    /// 保证向后兼容逐字节一致）。actions/terminal 的覆盖由调用方（session）用 config 配的
    /// TerminalSpec 处理，这里只管观测。
    pub fn derive_with_config(
        world: &World,
        engine: &Engine,
        _terminal: &TerminalSpec,
        config: &PlaytestConfig,
    ) -> SceneView {
        let mut observation = project_observation_with_config(world, &config.observation);
        // 派生量非空才注入 derived 键（空时不写——默认 config 的输出必须和老 derive 逐字节一致）
        if !config.observation.derived.is_empty() {
            let derived = compute_derived(world, &config.observation.derived);
            if let Some(obj) = observation.as_object_mut() {
                obj.insert("derived".to_string(), Value::Object(derived));
            }
        }
        SceneView { observation, actions: derive_actions(engine), done: None }
    }
}

/// 带 config 的观测投影：在自动剔装饰的基础上叠加 include/exclude/relabel。
/// config.observation 全空时与 [`project_observation`] 逐字节一致（向后兼容）。
fn project_observation_with_config(world: &World, cfg: &ObservationConfig) -> Value {
    let use_include = !cfg.include.is_empty();
    let mut entities = Vec::new();
    for id in world.entities() {
        let ent_label = world.name_of(id).map(|s| s.to_string());
        let mut comps = Map::new();
        for cname in world.components_of(id) {
            // 组件取舍：白名单优先（启用时只留白名单内）；否则剔默认装饰 + 用户 exclude
            let keep = if use_include {
                cfg.include.iter().any(|c| c == &cname)
            } else {
                !is_decorative(&cname) && !cfg.exclude.iter().any(|c| c == &cname)
            };
            if !keep {
                continue;
            }
            if let Ok(v) = world.get_component(id, &cname) {
                let mut cval = v.clone();
                // relabel：把命中 `<实体>/<组件>.<字段>` 的叶子键换成人话名（不改值/层级）
                if let Some(label) = &ent_label {
                    apply_relabel(&mut cval, &cname, label, cfg);
                }
                comps.insert(cname, cval);
            }
        }
        let mut e = Map::new();
        e.insert("id".to_string(), json!(id.to_string()));
        if let Some(name) = &ent_label {
            e.insert("name".to_string(), json!(name));
        }
        e.insert("components".to_string(), Value::Object(comps));
        entities.push(Value::Object(e));
    }
    json!({ "entities": entities })
}

/// 对一个组件值套 relabel：遍历 cfg.relabel，凡 path 形如 `<实体>/<本组件>.<字段路径>` 的，
/// 把那个叶子的最末键换成人话名。只支持顶层字段重命名（`组件.字段`）——够覆盖派生量配套用法。
fn apply_relabel(cval: &mut Value, cname: &str, ent_label: &str, cfg: &ObservationConfig) {
    let prefix = format!("{ent_label}/{cname}.");
    if let Some(obj) = cval.as_object_mut() {
        for r in &cfg.relabel {
            if let Some(field) = r.path.strip_prefix(&prefix) {
                // 只处理顶层字段（field 不含再下一层 '.'）；嵌套字段保持原样（够用即可）
                if !field.contains('.') {
                    if let Some(v) = obj.remove(field) {
                        obj.insert(r.name.clone(), v);
                    }
                }
            }
        }
    }
}

/// 算所有派生量，归到一个 `derived` 子对象（键=派生量名，确定序由声明序保证）。
fn compute_derived(world: &World, specs: &[DerivedSpec]) -> Map<String, Value> {
    let mut out = Map::new();
    for spec in specs {
        let v = match spec {
            DerivedSpec::Distance { from, to, metric, .. } => {
                derived_distance(world, from, to, *metric)
            }
            DerivedSpec::Alias { path, .. } => derived_alias(world, path),
            DerivedSpec::Count { component, .. } => derived_count(world, component),
        };
        out.insert(spec.name().to_string(), v);
    }
    out
}

/// 两个命名实体的 Position 距离。任一实体不存在/无 Position/坐标非数 → Null。
fn derived_distance(world: &World, from: &str, to: &str, metric: DistanceMetric) -> Value {
    let Some((ax, ay)) = entity_position(world, from) else { return Value::Null };
    let Some((bx, by)) = entity_position(world, to) else { return Value::Null };
    let d = match metric {
        DistanceMetric::Manhattan => (ax - bx).abs() + (ay - by).abs(),
        DistanceMetric::Euclidean => ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt(),
    };
    json!(d)
}

/// 读一个命名实体的 Position.{x,y}（都得是数才算）。
fn entity_position(world: &World, name: &str) -> Option<(f64, f64)> {
    let (_, id) = world.entity_names().find(|(n, _)| *n == name)?;
    let pos = world.get_component(id, "Position").ok()?;
    let x = pos.get("x").and_then(|v| v.as_f64())?;
    let y = pos.get("y").and_then(|v| v.as_f64())?;
    Some((x, y))
}

/// 字段别名：把 observation 路径 `<实体>/<组件>.<字段路径>` 指向的值原样镜像出来。
/// 取不到（实体/组件/字段不存在）→ Null。
fn derived_alias(world: &World, path: &str) -> Value {
    let Some((ent, rest)) = path.split_once('/') else { return Value::Null };
    let Some((comp, field_path)) = rest.split_once('.') else { return Value::Null };
    let Some((_, id)) = world.entity_names().find(|(n, _)| *n == ent) else {
        return Value::Null;
    };
    let Ok(cval) = world.get_component(id, comp) else { return Value::Null };
    // 沿字段路径逐层钻（支持 a.b.c 嵌套；数组下标也走对象/数组通用 step）
    let mut cur = cval;
    for seg in field_path.split('.') {
        cur = match cur {
            Value::Object(m) => match m.get(seg) {
                Some(v) => v,
                None => return Value::Null,
            },
            Value::Array(a) => match seg.parse::<usize>().ok().and_then(|i| a.get(i)) {
                Some(v) => v,
                None => return Value::Null,
            },
            _ => return Value::Null,
        };
    }
    cur.clone()
}

/// 带某组件的存活实体数。
fn derived_count(world: &World, component: &str) -> Value {
    let n = world
        .entities()
        .into_iter()
        .filter(|&id| world.components_of(id).iter().any(|c| c == component))
        .count();
    json!(n)
}

/// 观测投影：遍历存活实体（槽位序=确定性），每个实体投影成
/// `{"id":..,"name":..,"components":{玩法组件...}}`，剔除纯装饰组件。
/// 全是装饰的实体（如纯相机/纯背景 Sprite）components 为空，仍保留——
/// 它的存在本身是状态（实体在不在）。
fn project_observation(world: &World) -> Value {
    let mut entities = Vec::new();
    for id in world.entities() {
        let mut comps = Map::new();
        for cname in world.components_of(id) {
            if is_decorative(&cname) {
                continue;
            }
            if let Ok(v) = world.get_component(id, &cname) {
                comps.insert(cname, v.clone());
            }
        }
        let mut e = Map::new();
        e.insert("id".to_string(), json!(id.to_string()));
        if let Some(name) = world.name_of(id) {
            e.insert("name".to_string(), json!(name));
        }
        e.insert("components".to_string(), Value::Object(comps));
        entities.push(Value::Object(e));
    }
    json!({ "entities": entities })
}

/// 动作派生：枚举规则里所有 input 触发器的 filter.action，去重，
/// 每个动作配 {pressed, released}（pressed 在前=策略主力，released 也列全）。
/// 顺序确定：先按规则在规则集里的出现序收集 distinct action，再 ×{pressed,released}。
fn derive_actions(engine: &Engine) -> Vec<Action> {
    let mut seen: Vec<String> = Vec::new();
    for rule in &engine.rules.rules {
        if let Trigger::Event { name, filter, .. } = &rule.trigger {
            if name != "input" {
                continue;
            }
            // 输入动作名由 filter 的 action 字段声明（见 jump/game.json）
            if let Some(action) = filter.get("action").and_then(|v| v.as_str()) {
                if !seen.iter().any(|a| a == action) {
                    seen.push(action.to_string());
                }
            }
        }
    }
    let mut actions = Vec::with_capacity(seen.len() * 2);
    for a in seen {
        actions.push(Action { action: a.clone(), phase: "pressed".to_string() });
        actions.push(Action { action: a, phase: "released".to_string() });
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vitric_data::Schema;
    use vitric_rules::RuleSet;

    fn engine(rules: Value, schema: Value) -> Engine {
        let schema = Schema::parse(&schema, "schema.json").unwrap();
        Engine::new(RuleSet::parse(&rules, "rules.json").unwrap(), schema)
    }

    fn jump_like_engine() -> Engine {
        engine(
            json!({"rules": [
                {"id": "left", "on": {"event": "input", "filter": {"action": "left", "phase": "pressed"}},
                 "do": [{"set": "@hero.Velocity.x", "to": -8}]},
                {"id": "left-stop", "on": {"event": "input", "filter": {"action": "left", "phase": "released"}},
                 "do": [{"set": "@hero.Velocity.x", "to": 0}]},
                {"id": "right", "on": {"event": "input", "filter": {"action": "right", "phase": "pressed"}},
                 "do": [{"set": "@hero.Velocity.x", "to": 8}]},
                {"id": "jump", "on": {"event": "input", "filter": {"action": "space", "phase": "pressed"}},
                 "do": [{"set": "@hero.Velocity.y", "to": 14}]},
                // 非 input 触发的规则不该贡献动作词汇
                {"id": "tickrule", "on": "tick", "do": [{"emit": "noop", "data": {}}]}
            ]}),
            json!({"components": {
                "Velocity": {"fields": {"x": {"type": "number"}, "y": {"type": "number"}}}
            }}),
        )
    }

    #[test]
    fn actions_come_from_distinct_input_rule_actions() {
        let eng = jump_like_engine();
        let view = SceneView::derive(&World::new(), &eng, &TerminalSpec::default());
        // distinct action 集合 = {left, right, space}，每个 ×{pressed,released}
        let names: Vec<&str> = view.actions.iter().map(|a| a.action.as_str()).collect();
        assert_eq!(view.actions.len(), 6, "3 个动作 ×2 phase: {:?}", view.actions);
        // left 只出现一次（即便有 left 的 pressed 和 released 两条规则也只算一个动作）
        assert_eq!(names.iter().filter(|n| **n == "left").count(), 2);
        assert!(names.contains(&"left") && names.contains(&"right") && names.contains(&"space"));
        // pressed 排在对应 released 前
        let left_pressed = view.actions.iter().position(|a| a.action == "left" && a.phase == "pressed");
        let left_released = view.actions.iter().position(|a| a.action == "left" && a.phase == "released");
        assert!(left_pressed < left_released);
    }

    #[test]
    fn actions_derivation_is_deterministic() {
        let eng = jump_like_engine();
        let a = SceneView::derive(&World::new(), &eng, &TerminalSpec::default());
        let b = SceneView::derive(&World::new(), &eng, &TerminalSpec::default());
        assert_eq!(a.actions, b.actions);
    }

    #[test]
    fn observation_drops_decorative_keeps_gameplay() {
        let eng = jump_like_engine();
        let mut w = World::new();
        let hero = w.spawn_named("hero").unwrap();
        w.set_component(hero, "Velocity", json!({"x": 1.0, "y": 2.0})).unwrap();
        // Sprite 是纯装饰，应被剔除
        let schema = Schema::parse(
            &json!({"components": {"Sprite": {"fields": {"w": {"type": "number"}}}}}),
            "s.json",
        )
        .unwrap();
        let _ = schema; // Sprite 不在本测试 schema 校验范围，直接塞 raw 值即可
        w.set_component(hero, "Sprite", json!({"w": 1.0})).unwrap();

        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let ents = view.observation.get("entities").unwrap().as_array().unwrap();
        assert_eq!(ents.len(), 1);
        let comps = ents[0].get("components").unwrap().as_object().unwrap();
        assert!(comps.contains_key("Velocity"), "玩法组件保留: {comps:?}");
        assert!(!comps.contains_key("Sprite"), "装饰组件剔除: {comps:?}");
        assert_eq!(ents[0].get("name").unwrap(), &json!("hero"));
    }

    // ---- 第 6 阶段：config 覆盖（include/exclude/relabel/derived/terminal） ----

    use crate::config::{
        DerivedSpec, DistanceMetric, ObservationConfig, PlaytestConfig, Relabel, TerminalOverride,
    };

    /// 造一个带 Position 的世界（hero 在原点，flag 在 (3,4)）+ 一些装饰组件。
    fn pos_world() -> World {
        let mut w = World::new();
        let hero = w.spawn_named("hero").unwrap();
        w.set_component(hero, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(hero, "Velocity", json!({"x": 1.0, "y": 0.0})).unwrap();
        w.set_component(hero, "Sprite", json!({"w": 1.0})).unwrap();
        let flag = w.spawn_named("flag").unwrap();
        w.set_component(flag, "Position", json!({"x": 3.0, "y": 4.0})).unwrap();
        w.set_component(flag, "Goal", json!({})).unwrap();
        w
    }

    #[test]
    fn default_config_derive_byte_identical_to_plain_derive() {
        // 向后兼容铁律：默认 config 的 derive_with_config 必须和老 derive 逐字节一致
        let eng = jump_like_engine();
        let w = pos_world();
        let a = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let b = SceneView::derive_with_config(
            &w,
            &eng,
            &TerminalSpec::default(),
            &PlaytestConfig::default(),
        );
        let ja = serde_json::to_string(&a).unwrap();
        let jb = serde_json::to_string(&b).unwrap();
        assert_eq!(ja, jb, "默认 config 必须和老行为逐字节一致");
    }

    #[test]
    fn config_include_whitelist_keeps_only_listed() {
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                include: vec!["Position".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let ents = view.observation.get("entities").unwrap().as_array().unwrap();
        let hero = ents.iter().find(|e| e.get("name") == Some(&json!("hero"))).unwrap();
        let comps = hero.get("components").unwrap().as_object().unwrap();
        assert!(comps.contains_key("Position"), "白名单内保留: {comps:?}");
        assert!(!comps.contains_key("Velocity"), "白名单外剔除: {comps:?}");
    }

    #[test]
    fn config_exclude_drops_extra_components() {
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                exclude: vec!["Velocity".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let ents = view.observation.get("entities").unwrap().as_array().unwrap();
        let hero = ents.iter().find(|e| e.get("name") == Some(&json!("hero"))).unwrap();
        let comps = hero.get("components").unwrap().as_object().unwrap();
        assert!(comps.contains_key("Position"), "未排除的留: {comps:?}");
        assert!(!comps.contains_key("Velocity"), "exclude 的额外剔除: {comps:?}");
    }

    #[test]
    fn config_relabel_renames_leaf_key() {
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                relabel: vec![Relabel {
                    path: "hero/Position.x".to_string(),
                    name: "横坐标".to_string(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let ents = view.observation.get("entities").unwrap().as_array().unwrap();
        let hero = ents.iter().find(|e| e.get("name") == Some(&json!("hero"))).unwrap();
        let pos = hero.get("components").unwrap().get("Position").unwrap().as_object().unwrap();
        assert!(pos.contains_key("横坐标"), "x 被改人话名: {pos:?}");
        assert!(!pos.contains_key("x"), "原键被改掉: {pos:?}");
        assert!(pos.contains_key("y"), "没动的字段保留");
    }

    #[test]
    fn config_derived_distance_manhattan() {
        let eng = jump_like_engine();
        let w = pos_world(); // hero(0,0) flag(3,4)
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                derived: vec![DerivedSpec::Distance {
                    name: "to_exit".to_string(),
                    from: "hero".to_string(),
                    to: "flag".to_string(),
                    metric: DistanceMetric::Manhattan,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let derived = view.observation.get("derived").unwrap();
        assert_eq!(derived.get("to_exit").unwrap().as_f64().unwrap(), 7.0, "|3|+|4|=7");
    }

    #[test]
    fn config_derived_distance_euclidean() {
        let eng = jump_like_engine();
        let w = pos_world(); // hero(0,0) flag(3,4)
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                derived: vec![DerivedSpec::Distance {
                    name: "d".to_string(),
                    from: "hero".to_string(),
                    to: "flag".to_string(),
                    metric: DistanceMetric::Euclidean,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let d = view.observation.get("derived").unwrap().get("d").unwrap().as_f64().unwrap();
        assert!((d - 5.0).abs() < 1e-9, "sqrt(9+16)=5，实际 {d}");
    }

    #[test]
    fn config_derived_distance_null_when_entity_missing() {
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                derived: vec![DerivedSpec::Distance {
                    name: "d".to_string(),
                    from: "hero".to_string(),
                    to: "nonexistent".to_string(),
                    metric: DistanceMetric::Manhattan,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let d = view.observation.get("derived").unwrap().get("d").unwrap();
        assert!(d.is_null(), "缺实体 → null: {d:?}");
    }

    #[test]
    fn config_derived_alias_mirrors_value() {
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                derived: vec![DerivedSpec::Alias {
                    name: "vx".to_string(),
                    path: "hero/Velocity.x".to_string(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let vx = view.observation.get("derived").unwrap().get("vx").unwrap().as_f64().unwrap();
        assert_eq!(vx, 1.0, "alias 镜像 hero/Velocity.x=1.0");
    }

    #[test]
    fn config_derived_count_entities_with_component() {
        let eng = jump_like_engine();
        let w = pos_world(); // hero+flag 都有 Position
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                derived: vec![
                    DerivedSpec::Count {
                        name: "positioned".to_string(),
                        component: "Position".to_string(),
                    },
                    DerivedSpec::Count {
                        name: "goals".to_string(),
                        component: "Goal".to_string(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let derived = view.observation.get("derived").unwrap();
        assert_eq!(derived.get("positioned").unwrap().as_u64().unwrap(), 2);
        assert_eq!(derived.get("goals").unwrap().as_u64().unwrap(), 1);
    }

    #[test]
    fn config_alias_reads_relabeled_or_original_path() {
        // alias 的 path 用原始（未 relabel）路径——派生量在投影后追加，引用原始字段名
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                relabel: vec![Relabel {
                    path: "hero/Velocity.x".to_string(),
                    name: "速度".to_string(),
                }],
                derived: vec![DerivedSpec::Alias {
                    name: "vx".to_string(),
                    path: "hero/Velocity.x".to_string(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        // relabel 改了观测里的键，但 alias 仍按原始 path 取到值（派生在原始世界上算）
        let vx = view.observation.get("derived").unwrap().get("vx").unwrap().as_f64().unwrap();
        assert_eq!(vx, 1.0);
    }

    #[test]
    fn terminal_override_applies_custom_events() {
        // TerminalSpec::apply_override 把自定义 win/lose 名套进去
        let ovr = TerminalOverride {
            win_events: Some(vec!["reached-exit".to_string()]),
            lose_events: Some(vec!["fell".to_string()]),
            ending_prefixes: None,
        };
        let spec = TerminalSpec::default().apply_override(&ovr);
        assert_eq!(spec.classify("reached-exit"), Some(Outcome::Win));
        assert_eq!(spec.classify("fell"), Some(Outcome::Lose));
        // 没覆盖的 ending 前缀回退默认
        assert_eq!(spec.classify("ending-x"), Some(Outcome::Win));
        // 老的默认 win 名被覆盖掉了（不再认 game-won）
        assert_eq!(spec.classify("game-won"), None);
    }

    #[test]
    fn with_manifest_must_emit_appends_and_dedups() {
        // 清单声明的 must_emit 并进 win 集合：新名认得出，老默认名仍在（追加不替换）
        let spec = TerminalSpec::default().with_manifest_must_emit(["run-complete"]);
        assert_eq!(spec.classify("run-complete"), Some(Outcome::Win));
        assert_eq!(spec.classify("game-won"), Some(Outcome::Win), "默认 win 名仍保留");
        // 已存在的名（game-won）不会重复进集合
        let spec2 = TerminalSpec::default().with_manifest_must_emit(["game-won", "run-complete"]);
        assert_eq!(spec2.win_events.iter().filter(|n| *n == "game-won").count(), 1, "去重");
        assert!(spec2.win_events.iter().any(|n| n == "run-complete"));
    }

    #[test]
    fn with_manifest_must_emit_empty_is_unchanged() {
        // 空清单（没声明 gates）退化为默认集合——向后兼容铁律
        let base = TerminalSpec::default();
        let merged = base.with_manifest_must_emit(Vec::<String>::new());
        assert_eq!(merged, base, "无 must_emit 时必须和默认逐字段一致");
    }

    #[test]
    fn with_manifest_must_emit_stacks_on_override() {
        // playtest.json 覆盖先生效，再叠清单 must_emit：两路都认
        let ovr = TerminalOverride {
            win_events: Some(vec!["reached-exit".to_string()]),
            lose_events: None,
            ending_prefixes: None,
        };
        let spec = TerminalSpec::default()
            .apply_override(&ovr)
            .with_manifest_must_emit(["quest-done"]);
        assert_eq!(spec.classify("reached-exit"), Some(Outcome::Win), "覆盖的 win 名认得");
        assert_eq!(spec.classify("quest-done"), Some(Outcome::Win), "清单 must_emit 也认得");
        assert_eq!(spec.classify("game-won"), None, "覆盖替换掉了默认 win 名");
    }

    #[test]
    fn terminal_classifies_win_lose_and_ending_prefix() {
        let spec = TerminalSpec::default();
        assert_eq!(spec.classify("game-won"), Some(Outcome::Win));
        assert_eq!(spec.classify("win"), Some(Outcome::Win));
        assert_eq!(spec.classify("game-over"), Some(Outcome::Lose));
        assert_eq!(spec.classify("ending-true"), Some(Outcome::Win));
        assert_eq!(spec.classify("ending-bad"), Some(Outcome::Win));
        assert_eq!(spec.classify("input"), None);
        assert_eq!(spec.classify("collision"), None);
    }
}
