//! 单局试玩会话：派生 Scene View → 策略选动作 → 注入 → 步进，直到通关/死亡/超时，
//! 全程录像。一局 = 一段可重放可认证的确定性录像（设计稿四节）。
//!
//! **接口取向说明**：设计稿写的是 `run_session(project_dir, ...)`，但装配运行时的
//! `Runtime::boot` 住在 vitric-cli（cli 依赖 playtest，playtest 不能反向依赖 cli，否则成环）。
//! 所以这里把 boot 留给调用方（CLI 的 cmd_playtest），run_session 接已 boot 好的
//! `(Sim, GameLogic, Engine)`——职责更纯：playtest 只管「喂视图、跑循环、出录像」，
//! 不认识项目目录怎么装配。Engine 单独传是因为派生动作词汇要读规则，而它被 GameLogic
//! 装配体私有持有，拿不到引用。

use std::collections::BTreeMap;

use vitric_sim::{GameLogic, Recording, Sim};
use vitric_rules::Engine;
use serde_json::Value;

use crate::scene_view::{Outcome, SceneView, TerminalSpec};
use crate::strategy::{PlaytestNote, Strategy};

/// 一个数值字段在整局里的轨迹摘要（**增量统计**，不存每 tick 全表）。
///
/// key（在 `numeric_summary` 的 BTreeMap 里）= 数值叶子的路径，形如
/// `hero/Resources.gold`（实体名或 id + 「组件.字段...」）。每 tick 从当前 observation
/// 取所有数值叶子，对各自的 NumericStat 做 O(1) 更新——不保留逐 tick 历史。
///
/// 为什么这么设计：模拟经营找数值崩（设计稿五节「数值崩」）靠的是「这个字段最后跑成多大/
/// 有没有归零/是不是只增不减」这些**摘要**信号，不需要逐 tick 曲线。增量统计把内存压到
/// O(数值字段数) 而非 O(字段数 × tick 数)，几千局也扛得住（设计稿九节性能预算）。
///
/// **不进哈希、不进录像**：和 state_trace/fired_events 一样，是「这局怎么跑的」旁观记录，
/// 是录像的纯函数派生（同录像必出同摘要），自然满足确定性铁律。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NumericStat {
    /// 这个字段第一次被观测到时的值（首帧基线）。
    pub first: f64,
    /// 最后一次观测到的值（末帧——「跑成多大/崩到多小」看它）。
    pub last: f64,
    /// 整局观测到的最小值。
    pub min: f64,
    /// 整局观测到的最大值。
    pub max: f64,
    /// 整局**只增不减**（每次新观测都 ≥ 上一次）——经济跑飞的特征之一。
    pub monotonic_up: bool,
    /// 整局曾触达过 0（资源归零——崩盘软锁的特征之一）。
    pub hit_zero: bool,
    /// 曾观测到非有限值（inf / nan）——数值溢出/除零的硬信号，单独标。
    pub non_finite: bool,
}

impl NumericStat {
    /// 用首次观测值初始化。
    fn start(v: f64) -> NumericStat {
        let finite = v.is_finite();
        NumericStat {
            first: v,
            last: v,
            min: v,
            max: v,
            monotonic_up: true,
            hit_zero: finite && v == 0.0,
            non_finite: !finite,
        }
    }

    /// 增量并入一次新观测（O(1)）：刷新 last/min/max、维护单调/归零/非有限标记。
    fn observe(&mut self, v: f64) {
        if !v.is_finite() {
            // 非有限值单独标，不污染 min/max（NaN 比较全 false 会破坏单调判定）
            self.non_finite = true;
            self.last = v;
            return;
        }
        // 单调：只要某次比上一次小，就不再是「只增不减」
        if v < self.last {
            self.monotonic_up = false;
        }
        self.last = v;
        if v < self.min {
            self.min = v;
        }
        if v > self.max {
            self.max = v;
        }
        if v == 0.0 {
            self.hit_zero = true;
        }
    }
}

/// 从一份 observation（SceneView 投影的 JSON）抽出所有数值叶子，并入 summary（增量）。
///
/// key 路径：`<实体名或id>/<组件>.<字段>[.<子字段>...]`。遍历 observation.entities，
/// 每个实体取 name（无名退化成 id），再钻它的 components 树，遇到数值（含 bool 折成 0/1?
/// 不——只收真数值 number，bool 是 flag 不是数值，不进数值崩分析）就更新对应 NumericStat。
///
/// **确定性**：observation 的实体是按槽位序、组件/字段是 serde_json Map（BTreeMap 序），
/// 遍历序固定；BTreeMap 聚合输出也固定。O(数值叶子数)/tick，不存历史（设计稿九节）。
fn collect_numeric_leaves(observation: &Value, summary: &mut BTreeMap<String, NumericStat>) {
    let Some(entities) = observation.get("entities").and_then(|v| v.as_array()) else {
        return;
    };
    for ent in entities {
        // 实体标识：优先人话名，无名退化成 id（scene_view 保证 id 一定在）
        let label = ent
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| ent.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()));
        let Some(label) = label else { continue };
        let Some(comps) = ent.get("components").and_then(|v| v.as_object()) else {
            continue;
        };
        for (cname, cval) in comps {
            // 路径前缀：实体/组件，字段名在递归里接上
            let prefix = format!("{label}/{cname}");
            walk_numeric(&prefix, cval, summary);
        }
    }
}

/// 递归钻一个组件值，把数值叶子并入 summary。path 是「到这一层」的路径前缀。
fn walk_numeric(path: &str, value: &Value, summary: &mut BTreeMap<String, NumericStat>) {
    match value {
        Value::Number(n) => {
            // 只收真数值；整数也转 f64（数值崩看量级，i64/f64 统一成一个标尺）
            if let Some(f) = n.as_f64() {
                upsert(summary, path, f);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                walk_numeric(&child, v, summary);
            }
        }
        Value::Array(arr) => {
            // 数组按下标编路径（如 inventory.0 / inventory.1），递归同样规则
            for (i, v) in arr.iter().enumerate() {
                let child = format!("{path}.{i}");
                walk_numeric(&child, v, summary);
            }
        }
        // bool/string/null 不是数值，跳过（bool 是 flag，不进数值崩分析）
        _ => {}
    }
}

/// 有则增量并入、无则以首值初始化（增量统计的 upsert）。
fn upsert(summary: &mut BTreeMap<String, NumericStat>, key: &str, v: f64) {
    summary
        .entry(key.to_string())
        .and_modify(|s| s.observe(v))
        .or_insert_with(|| NumericStat::start(v));
}

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

/// 一局的结果：结局 + 用了多少 tick + 这局的录像（可重放）+ 轻量遥测。
///
/// 遥测（state_trace/fired_events）只供聚合器分析，**不进录像、不进哈希**——
/// 它是「这局怎么跑的」的旁观记录，不是「这局是什么」的权威状态。确定性铁律：
/// 同 (策略,seed,起点) 跑出的录像逐字节一致，遥测是录像的纯函数派生，自然也一致。
#[derive(Debug, Clone)]
pub struct SessionResult {
    pub outcome: Outcome,
    pub ticks: u64,
    pub recording: Recording,
    /// 每 tick step 后的世界状态哈希（`world.state_hash()`，已优化过、便宜）。
    /// 长度 = 实际跑的 tick 数；是「状态变没变」的权威信号，软锁聚类靠它判冻结。
    pub state_trace: Vec<u64>,
    /// 整局出现过的事件名（去重，首次出现序）：StepReport.events + logic.drain_observed()。
    /// 用来判「哪些终止/里程碑事件被触发过」「哪些 input 动作引发过规则响应」。
    pub fired_events: Vec<String>,
    /// 数值遥测：每个数值字段路径 → 整局轨迹摘要（增量统计，不存每 tick 全表）。
    /// 给聚合器逮经济跑飞/崩盘/溢出（设计稿五节「数值崩」）。不进哈希/录像。
    pub numeric_summary: BTreeMap<String, NumericStat>,
    /// LLM 档定性 note（清晰度/连续性/选择有效性，设计稿五节「LLM 定性 note」）。
    /// 只有 LLM 策略会产（廉价策略档 drain_notes 默认空）。**不进哈希/录像**——
    /// 它是「LLM 这局看着怎么样」的旁观主观提示，和别的遥测同级，不影响确定性。
    pub notes: Vec<PlaytestNote>,
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

    // 遥测累加器：state_trace 每 tick 一条，fired_events 去重（用 set 判重 + vec 保序），
    // numeric_summary 增量统计（每 tick 从 observation 取数值叶子更新，不存历史）
    let mut state_trace: Vec<u64> = Vec::new();
    let mut numeric_summary: BTreeMap<String, NumericStat> = BTreeMap::new();
    let mut fired_events: Vec<String> = Vec::new();
    let mut seen_events: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut note_events = |names: &mut dyn Iterator<Item = &str>| {
        for name in names {
            if seen_events.insert(name.to_string()) {
                fired_events.push(name.to_string());
            }
        }
    };

    // LLM 档定性 note 累加器（每 tick 决策后从策略收，非 LLM 策略 drain 永远空）。
    // 不进哈希/录像——它是旁观主观提示，确定性铁律不约束它（LLM 局本就不要求跨次复现）。
    let mut notes: Vec<PlaytestNote> = Vec::new();

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
        // 决策后收 note：LLM 策略在 choose 里可能产了 note，每 tick 取走累积
        // （取走即清空，避免重复）。非 LLM 策略默认返空，零开销。
        notes.append(&mut strategy.drain_notes());

        let report = sim.step(logic).map_err(|e| e.to_string())?;

        // 遥测（step 后采）：state_hash 是状态指纹（已优化、便宜，不自己序列化整世界），
        // 事件名累进去重集。遥测只读、不回写世界，不影响录像/哈希。
        state_trace.push(sim.world.state_hash());
        // 数值遥测：step 后对当前世界投影一份观测，抽数值叶子增量并入摘要。
        // 用 SceneView::derive 的同款投影（剔装饰、按槽位序），保证 key 路径与策略所见一致；
        // 只取 observation（不用 actions），增量更新 O(数值叶子数)，不存逐 tick 历史。
        let post = SceneView::derive(&sim.world, engine, &cfg.terminal);
        collect_numeric_leaves(&post.observation, &mut numeric_summary);
        note_events(&mut report.events.iter().map(|e| e.name.as_str()));

        // 扫本 tick 发给逻辑层的事件 + 逻辑层 emit 的事件，命中终止就收
        if let Some(o) = scan_terminal(report.events.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
        let emitted = logic.drain_observed();
        note_events(&mut emitted.iter().map(|e| e.name.as_str()));
        if let Some(o) = scan_terminal(emitted.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
    }

    // 退出循环（done/Timeout）后再收一次 note：LLM 可能在最后一个决策 tick 吐了
    // 还没被 drain 的 note（如「这关到这儿就卡住了，看不懂下一步」）。
    notes.append(&mut strategy.drain_notes());

    let recording = sim.stop_recording().expect("刚 start_recording 过");
    Ok(SessionResult {
        outcome,
        ticks: sim.tick,
        recording,
        state_trace,
        fired_events,
        numeric_summary,
        notes,
    })
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

    /// 一个每 tick 吐一条 note 的假策略（不碰 LLM，只验 session 的 note 收集通道）。
    struct NotingStrategy {
        tick: u64,
        pending: Vec<crate::strategy::PlaytestNote>,
    }
    impl crate::strategy::Strategy for NotingStrategy {
        fn choose(&mut self, _view: &SceneView) -> Option<Action> {
            // 每个决策 tick 攒一条 note，模拟 LLM 策略在 choose 里产 note
            self.pending.push(crate::strategy::PlaytestNote {
                tick: self.tick,
                kind: "clarity".to_string(),
                text: format!("第 {} tick 看不懂", self.tick),
            });
            self.tick += 1;
            None
        }
        fn drain_notes(&mut self) -> Vec<crate::strategy::PlaytestNote> {
            std::mem::take(&mut self.pending)
        }
    }

    #[test]
    fn run_session_collects_notes_from_strategy() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = NotingStrategy { tick: 0, pending: vec![] };
        let cfg = SessionConfig { max_ticks: 5, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        // 5 个决策 tick 各产一条 note，全被 session 收进 notes
        assert_eq!(res.notes.len(), 5, "每 tick 一条 note 应全部收齐: {:?}", res.notes);
        assert_eq!(res.notes[0].tick, 0);
        assert_eq!(res.notes[4].tick, 4);
        assert!(res.notes[0].text.contains("看不懂"));
    }

    #[test]
    fn run_session_notes_empty_for_non_noting_strategy() {
        // 普通策略（random）不产 note → notes 为空（note 通道是 LLM 档专属）
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 10, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert!(res.notes.is_empty(), "非 LLM 策略不产 note");
    }

    #[test]
    fn run_session_collects_state_trace_one_per_tick() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 40, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        // 每跑一个 tick 采一条 state_hash，长度必须等于实际 tick 数
        assert_eq!(res.state_trace.len(), res.ticks as usize);
        assert_eq!(res.state_trace.len(), 40);
    }

    /// 每 tick 都 emit 同一个非终止事件——用来验证 fired_events 去重。
    struct EmitEveryTick {
        event: String,
        pending: Vec<Event>,
    }
    impl GameLogic for EmitEveryTick {
        fn on_tick(
            &mut self,
            _: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            self.pending.push(Event::new(&self.event, json!({})));
            Ok(())
        }
        fn drain_observed(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }
    }

    #[test]
    fn run_session_collects_fired_events_deduped() {
        let mut sim = Sim::new(1);
        // 每 tick 发同名事件 20 次，fired_events 应只收一次（去重）
        let mut logic = EmitEveryTick { event: "milestone".to_string(), pending: vec![] };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        // milestone 不是终止事件 → 不会停，跑满
        let cfg = SessionConfig { max_ticks: 20, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert!(res.fired_events.contains(&"milestone".to_string()), "{:?}", res.fired_events);
        // 去重：发了 20 次，fired_events 里只一份
        assert_eq!(res.fired_events.iter().filter(|n| *n == "milestone").count(), 1);
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

    // ---- 数值遥测（NumericStat / numeric_summary）单元测试 ----

    #[test]
    fn numeric_stat_observe_tracks_min_max_last_monotonic_zero() {
        // 100 → 50 → 200 → 0：min=0 max=200 last=0；中途下降过 → 非单调；触达过 0
        let mut s = NumericStat::start(100.0);
        s.observe(50.0);
        s.observe(200.0);
        s.observe(0.0);
        assert_eq!(s.first, 100.0);
        assert_eq!(s.last, 0.0);
        assert_eq!(s.min, 0.0);
        assert_eq!(s.max, 200.0);
        assert!(!s.monotonic_up, "中途降过就不是只增不减");
        assert!(s.hit_zero, "触达过 0");
        assert!(!s.non_finite);
    }

    #[test]
    fn numeric_stat_monotonic_up_stays_true_when_only_grows() {
        let mut s = NumericStat::start(1.0);
        for v in [2.0, 4.0, 8.0, 16.0] {
            s.observe(v);
        }
        assert!(s.monotonic_up, "只增不减");
        assert_eq!(s.max, 16.0);
        assert!(!s.hit_zero);
    }

    #[test]
    fn numeric_stat_flags_non_finite() {
        let mut s = NumericStat::start(1.0);
        s.observe(f64::INFINITY);
        assert!(s.non_finite, "inf 必须标 non_finite");
        // 非有限值不污染 min/max（仍是有限观测的范围）
        assert_eq!(s.max, 1.0);
    }

    #[test]
    fn collect_numeric_leaves_extracts_nested_paths() {
        // observation 仿 SceneView 投影结构：entities[].{name,components.{Comp.{field}}}
        let obs = json!({"entities": [
            {"name": "hero", "id": "0v0", "components": {
                "Resources": {"gold": 12.0, "wood": 3},
                "Stats": {"hp": 100}
            }},
            {"id": "1v0", "components": {"Tally": {"n": 7}}}  // 无名 → 用 id 当 label
        ]});
        let mut sum: BTreeMap<String, NumericStat> = BTreeMap::new();
        collect_numeric_leaves(&obs, &mut sum);
        assert!(sum.contains_key("hero/Resources.gold"), "{:?}", sum.keys().collect::<Vec<_>>());
        assert!(sum.contains_key("hero/Resources.wood"));
        assert!(sum.contains_key("hero/Stats.hp"));
        assert!(sum.contains_key("1v0/Tally.n"), "无名实体用 id 当 label");
        assert_eq!(sum["hero/Resources.gold"].first, 12.0);
    }

    #[test]
    fn collect_numeric_leaves_skips_non_numeric() {
        // bool/string 不进数值摘要（flag 不是数值）
        let obs = json!({"entities": [
            {"name": "w", "components": {"State": {"sealed": true, "label": "x", "count": 5}}}
        ]});
        let mut sum: BTreeMap<String, NumericStat> = BTreeMap::new();
        collect_numeric_leaves(&obs, &mut sum);
        assert!(sum.contains_key("w/State.count"));
        assert!(!sum.contains_key("w/State.sealed"), "bool 不收");
        assert!(!sum.contains_key("w/State.label"), "string 不收");
    }

    /// 一个每 tick 把某实体字段 ×2 的逻辑（直接改 world，模拟经济跑飞）。
    struct DoublerLogic {
        ent: String,
    }
    impl GameLogic for DoublerLogic {
        fn on_tick(
            &mut self,
            world: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            // 找到命名实体，把 Bank.gold ×2（float，避免 i64 checked_add 溢出报错）
            let id = world.entity_names().find(|(n, _)| *n == self.ent).map(|(_, id)| id);
            if let Some(id) = id {
                if let Ok(v) = world.get_component(id, "Bank") {
                    let cur = v.get("gold").and_then(|g| g.as_f64()).unwrap_or(0.0);
                    let _ = world.set_component(id, "Bank", json!({"gold": cur * 2.0}));
                }
            }
            Ok(())
        }
    }

    fn bank_world_sim() -> Sim {
        let mut sim = Sim::new(1);
        let id = sim.world.spawn_named("hero").unwrap();
        sim.world.set_component(id, "Bank", json!({"gold": 1.0})).unwrap();
        sim
    }

    #[test]
    fn run_session_numeric_summary_catches_runaway_growth() {
        let mut sim = bank_world_sim();
        let mut logic = DoublerLogic { ent: "hero".to_string() };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 30, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        let stat = res.numeric_summary.get("hero/Bank.gold").expect("应采到 hero/Bank.gold");
        assert!(stat.monotonic_up, "只 ×2 → 只增不减");
        // 30 tick 翻倍：远超初值的千倍（2^30 ≈ 1e9）
        assert!(stat.max > 1e6, "翻倍 30 次应跑飞到 >1e6，实际 {}", stat.max);
        assert!(stat.last > stat.first * 1000.0, "末值 ≫ 首值");
    }

    #[test]
    fn run_session_numeric_summary_is_incremental_not_full_history() {
        // 摘要只存每个字段一条 NumericStat，与 tick 数无关（增量，不存历史）
        let mut sim = bank_world_sim();
        let mut logic = DoublerLogic { ent: "hero".to_string() };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 20, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        // 只一个数值字段 → 摘要里恰好一条，不随 tick 膨胀
        assert_eq!(res.numeric_summary.len(), 1);
    }

    #[test]
    fn run_session_numeric_summary_is_deterministic() {
        let run = || {
            let mut sim = bank_world_sim();
            let mut logic = DoublerLogic { ent: "hero".to_string() };
            let eng = empty_engine();
            let mut strat = RandomStrategy::new(9);
            let cfg = SessionConfig { max_ticks: 15, seed: 9, ..Default::default() };
            run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap().numeric_summary
        };
        assert_eq!(run(), run(), "同输入两次跑数值摘要必须逐项一致");
    }
}
