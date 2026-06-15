//! 场景视图（Scene View）——引擎为当前世界态自动吐的一份「代理所见」。
//!
//! 三部分（见设计稿一节）：observation（剔除纯装饰后的玩法状态投影）、
//! actions（游戏声明的输入词汇）、done（终止判定）。**纯投影**：只读世界，
//! 绝不改 world、不进哈希、不影响确定性——所以它接收的是 `&World`/`&Engine`。

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use vitric_ecs::World;
use vitric_rules::{Engine, Trigger};

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
    /// 从世界 + 规则引擎 + 终止规格派生一份视图。`done` 始终为 None——
    /// 终止是「事件命中」才知道的，由 session 在 step 后扫事件判，不由静态世界态判。
    /// （静态世界投影不含「刚发生了 game-won」这种瞬时信息。）
    pub fn derive(world: &World, engine: &Engine, _terminal: &TerminalSpec) -> SceneView {
        SceneView {
            observation: project_observation(world),
            actions: derive_actions(engine),
            done: None,
        }
    }
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
