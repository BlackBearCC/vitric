//! 单局试玩会话：派生 Scene View → 策略选动作 → 注入 → 步进，直到通关/死亡/超时，
//! 全程录像。一局 = 一段可重放可认证的确定性录像（设计稿四节）。
//!
//! **接口取向说明**：设计稿写的是 `run_session(project_dir, ...)`，但装配运行时的
//! `Runtime::boot` 住在 vitric-cli（cli 依赖 playtest，playtest 不能反向依赖 cli，否则成环）。
//! 所以这里把 boot 留给调用方（CLI 的 cmd_playtest），run_session 接已 boot 好的
//! `(Sim, GameLogic, Engine)`——职责更纯：playtest 只管「喂视图、跑循环、出录像」，
//! 不认识项目目录怎么装配。Engine 单独传是因为派生动作词汇要读规则，而它被 GameLogic
//! 装配体私有持有，拿不到引用。

use vitric_sim::{GameLogic, Recording, Sim};
use vitric_rules::Engine;

use crate::scene_view::{Outcome, SceneView, TerminalSpec};
use crate::strategy::Strategy;

/// 一局的配置。
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// 跑满这么多 tick 还没终止就判 Timeout。
    pub max_ticks: u64,
    /// 策略 PCG 的播种（同 seed + 同策略 + 同起点 = 同一局）。
    pub seed: u64,
    /// 哪些事件算终止。
    pub terminal: TerminalSpec,
}

impl Default for SessionConfig {
    fn default() -> SessionConfig {
        SessionConfig { max_ticks: 600, seed: 0, terminal: TerminalSpec::default() }
    }
}

/// 一局的结果：结局 + 用了多少 tick + 这局的录像（可重放）。
#[derive(Debug, Clone)]
pub struct SessionResult {
    pub outcome: Outcome,
    pub ticks: u64,
    pub recording: Recording,
}

/// 跑一局。`sim`/`logic` 必须是刚 boot 出来、还在 tick 0 的全新一对（要录可重放的录像，
/// 必须从冷启动起录——和 vitric run 的 --record 同一条约束）。`engine` 用来派生动作词汇。
///
/// 循环（每 tick）：派生 Scene View → 若已 done 跳出 → 策略选动作 → inject_input →
/// step → 扫本 tick 事件命中终止则记 outcome 跳出 → 到 max_ticks 记 Timeout。
pub fn run_session(
    sim: &mut Sim,
    logic: &mut dyn GameLogic,
    engine: &Engine,
    strategy: &mut dyn Strategy,
    cfg: &SessionConfig,
) -> Result<SessionResult, String> {
    if sim.is_recording() {
        return Err("run_session 要自己开录：传进来的 sim 不该已在录像中".to_string());
    }
    sim.start_recording();

    let mut outcome = Outcome::Timeout;
    while sim.tick < cfg.max_ticks {
        // Scene View 是纯投影：只读世界/规则，绝不改 world、不进哈希
        let view = SceneView::derive(&sim.world, engine, &cfg.terminal);
        if let Some(done) = view.done {
            // 理论上 done 由 step 后扫事件判（见 SceneView::derive 注释），
            // 这里保留分支以防后续阶段让 derive 也能判静态终止。
            outcome = done;
            break;
        }

        // 策略选动作 → 注入（None=本 tick 不操作）
        if let Some(action) = strategy.choose(&view) {
            sim.inject_input(&action.action, &action.phase);
        }

        let report = sim.step(logic).map_err(|e| e.to_string())?;

        // 扫本 tick 发给逻辑层的事件 + 逻辑层 emit 的事件，命中终止就收
        if let Some(o) = scan_terminal(report.events.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
        let emitted = logic.drain_observed();
        if let Some(o) = scan_terminal(emitted.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
    }

    let recording = sim.stop_recording().expect("刚 start_recording 过");
    Ok(SessionResult { outcome, ticks: sim.tick, recording })
}

/// 一组事件里第一个命中终止的结局（按事件序，确定性）。
fn scan_terminal<'a>(
    events: impl Iterator<Item = &'a vitric_rules::Event>,
    terminal: &TerminalSpec,
) -> Option<Outcome> {
    for e in events {
        if let Some(o) = terminal.classify(&e.name) {
            return Some(o);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vitric_data::Schema;
    use vitric_rules::{Event, RuleSet};
    use vitric_sim::Pcg32;

    use crate::scene_view::Action;
    use crate::strategy::RandomStrategy;

    fn empty_engine() -> Engine {
        let schema = Schema::parse(&json!({"components": {}}), "s.json").unwrap();
        Engine::new(RuleSet::parse(&json!({"rules": []}), "r.json").unwrap(), schema)
    }

    /// 在第 N tick 发一个终止事件的最小逻辑（drain_observed 把它交给会话扫描）。
    struct EmitAt {
        at: u64,
        event: String,
        pending: Vec<Event>,
    }
    impl GameLogic for EmitAt {
        fn on_tick(
            &mut self,
            _world: &mut vitric_ecs::World,
            _events: Vec<Event>,
            _rng: &mut Pcg32,
            tick: u64,
        ) -> Result<(), String> {
            // step 在 on_tick 后才 +1，所以这里的 tick 是「即将完成的那一帧」编号
            if tick == self.at {
                self.pending.push(Event::new(&self.event, json!({})));
            }
            Ok(())
        }
        fn drain_observed(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }
    }

    /// 永不终止的逻辑：用来验证 Timeout。
    struct NeverEnds;
    impl GameLogic for NeverEnds {
        fn on_tick(
            &mut self,
            _: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn run_session_collects_win_on_terminal_event() {
        let mut sim = Sim::new(1);
        let mut logic = EmitAt { at: 3, event: "game-won".to_string(), pending: vec![] };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 100, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Win);
        assert_eq!(res.ticks, 4, "tick 3 那帧发的 game-won，step 后 tick=4 时收到");
    }

    #[test]
    fn run_session_collects_lose_on_terminal_event() {
        let mut sim = Sim::new(1);
        let mut logic = EmitAt { at: 2, event: "game-over".to_string(), pending: vec![] };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 100, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Lose);
    }

    #[test]
    fn run_session_times_out_and_records() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 50, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Timeout);
        assert_eq!(res.ticks, 50);
        assert_eq!(res.recording.ticks, 50);
        // 空 engine 没有动作词汇，策略选不出动作 → 录像无输入，但 checkpoint 仍在
        assert!(!res.recording.checkpoints.is_empty());
    }

    #[test]
    fn run_session_is_deterministic_byte_for_byte() {
        let run = || {
            let mut sim = Sim::new(1);
            let mut logic = NeverEnds;
            let eng = action_engine();
            let mut strat = RandomStrategy::new(123);
            let cfg = SessionConfig { max_ticks: 80, seed: 123, ..Default::default() };
            run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap()
        };
        let a = run();
        let b = run();
        assert_eq!(a.outcome, b.outcome);
        assert_eq!(a.ticks, b.ticks);
        // 录像逐字节一致
        let ja = serde_json::to_string(&a.recording).unwrap();
        let jb = serde_json::to_string(&b.recording).unwrap();
        assert_eq!(ja, jb, "同 (策略,seed,起点) 两次跑录像必须逐字节一致");
    }

    /// 带 input 词汇的引擎：让随机策略真能注入动作，录像里有 inputs。
    fn action_engine() -> Engine {
        let schema = Schema::parse(
            &json!({"components": {"Velocity": {"fields": {"x": {"type": "number"}}}}}),
            "s.json",
        )
        .unwrap();
        let rules = RuleSet::parse(
            &json!({"rules": [
                {"id": "left", "on": {"event": "input", "filter": {"action": "left", "phase": "pressed"}},
                 "do": [{"emit": "noop", "data": {}}]},
                {"id": "right", "on": {"event": "input", "filter": {"action": "right", "phase": "pressed"}},
                 "do": [{"emit": "noop", "data": {}}]}
            ]}),
            "r.json",
        )
        .unwrap();
        Engine::new(rules, schema)
    }

    #[test]
    fn run_session_with_actions_records_inputs() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = action_engine();
        let mut strat = RandomStrategy::new(5);
        let cfg = SessionConfig { max_ticks: 200, seed: 5, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Timeout);
        assert!(!res.recording.inputs.is_empty(), "随机策略应注入过一些输入");
        // 注入的动作都在词汇表里
        for inp in &res.recording.inputs {
            assert!(["left", "right"].contains(&inp.action.as_str()), "意外动作 {inp:?}");
        }
        let _ = Action { action: "x".into(), phase: "pressed".into() }; // 引用 Action 类型
    }
}
