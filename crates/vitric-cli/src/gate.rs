//! `vitric gate` — 交付门禁。
//!
//! 立场：游戏是 agent（AI）做的，"做完了"不能靠 agent 自述——引擎必须**机械地**
//! 验证交付质量。门禁的核心是确定性录像：一份能逐校验点逐位重放、且重放过程中
//! 真的触发了终局事件（默认 game-won）的录像，就是一张**不可伪造的通关证书**——
//! 想伪造任何一帧，状态哈希必然在下一个校验点跑偏。
//!
//! 四道门（清单 `gates` 字段声明，见 vitric-data 的 [`vitric_data::Gates`]）：
//! 1. check 门：完整项目校验（vitric check 同款），任何错误 = FAIL；
//! 2. 通关录像门：每条录像独立重放，校验点一致 + must_emit 事件出现 + 长度 ≤ max_ticks；
//! 3. 断言门（可选）：重放过程中每个 tick 全量求值断言集，任何一刻违反 = FAIL；
//! 4. playtest 门（可选，声明 `gates.playtest` 才跑）：真跑一遍 playtest swarm（确定可复现），
//!    聚合出报告，再逐条核对清单声明的契约（能不能通关、软锁数、不可达结局、惰性动作、
//!    数值崩）——把"自动清地板"变成交付契约，任一条不达标 = FAIL。
//!
//! 没有声明 gates 的项目直接拒绝——无门禁项目不出证书，空门禁放行就是后门。

use std::path::Path;

use serde_json::{json, Value};

use vitric_data::{Gates, PlaytestGate, Project};
use vitric_playtest::{
    aggregate_with_endings_and_declared, perturb_plan, run_seed_swarm, run_session_lookahead,
    run_swarm_with_config, LabeledResult, LookaheadConfig, PlaytestConfig, Report, SessionConfig,
    SessionSpec, StrategyKind, TerminalSpec,
};
use vitric_rules::{Engine, RuleSet};
use vitric_sim::Recording;

use crate::runtime::{self, Runtime};

/// 一条断言：id + 条件三元组列表（全部成立 = 健康）。格式同控制面 assert/add。
type Assertion = (String, Vec<(String, String, Value)>);

/// 跑全部门禁。返回 (JSON 报告, 是否全过)；Err 只用于"门禁本身无法成立"的情况
/// （目录没有清单 / 清单没声明 gates）——这些必须是显式硬错误，不是一份 pass=false 的报告。
pub fn run(dir: &Path) -> Result<(Value, bool), String> {
    let project = Project::load(dir).map_err(|r| r.to_string())?;
    // 约束：没有门禁声明就没有机器可验的交付标准，gate 拒绝出证书。
    // 空 playthroughs 同理——证书的本体就是通关录像，没有录像的门禁是空门。
    let Some(gates) = project.manifest.gates.clone() else {
        return Err(
            "清单未声明 gates——无门禁项目不出证书。\
             提示：在 vitric.json 加 \"gates\": {\"playthroughs\": \
             [{\"recording\": \"qa/clear.json\", \"must_emit\": \"game-won\"}]}，\
             录像用 vitric run <项目目录> --record qa/clear.json 录制"
                .to_string(),
        );
    };
    if gates.playthroughs.is_empty() {
        return Err(
            "gates.playthroughs 为空——通关录像是交付证书的本体，没有录像就没有证书。\
             提示：vitric run <项目目录> --record qa/clear.json 录一局通关，再挂进 gates.playthroughs"
                .to_string(),
        );
    }

    let mut results: Vec<Value> = Vec::new();

    // ---- 门 1：check（完整项目校验，vitric check 同款内核）----
    if gates.check {
        match runtime::check(dir) {
            Ok(report) => results.push(json!({
                "name": "check", "status": "pass",
                "detail": {"entities": report["entities"], "initial_hash": report["initial_hash"]},
            })),
            Err(e) => results.push(json!({"name": "check", "status": "fail", "detail": e})),
        }
    }

    // ---- 断言集加载（求值发生在重放过程中）----
    let mut assertions: Vec<Assertion> = Vec::new();
    let mut assertions_gate_error: Option<String> = None;
    if let Some(rel) = &gates.assertions {
        match load_assertions(dir, rel) {
            Ok(list) => assertions = list,
            // 声明了断言文件但读不出来：这是断言门自身的失败，不能静默当作"没有断言"
            Err(e) => assertions_gate_error = Some(e),
        }
    }
    // 断言条件求值复用规则引擎的 Engine::check（空规则集 + 项目 schema，和控制面同款）
    let checker = Engine::new(RuleSet::default(), project.schema.clone());
    let mut violations: Vec<Value> = Vec::new();

    // ---- 门 2：通关录像（每条独立重放验证）----
    for entry in &gates.playthroughs {
        let name = format!("playthrough:{}", entry.recording);
        match run_playthrough(dir, entry, &gates, &checker, &assertions, &mut violations) {
            Ok(detail) => results.push(json!({"name": name, "status": "pass", "detail": detail})),
            Err(e) => results.push(json!({"name": name, "status": "fail", "detail": e})),
        }
    }

    // ---- 门 3：断言（违反明细在重放中收集，这里汇总裁决）----
    if gates.assertions.is_some() {
        if let Some(e) = assertions_gate_error {
            results.push(json!({"name": "assertions", "status": "fail", "detail": e}));
        } else if violations.is_empty() {
            results.push(json!({
                "name": "assertions", "status": "pass",
                "detail": format!("{} 条断言在全部重放的每个 tick 求值，零违反", assertions.len()),
            }));
        } else {
            results.push(json!({
                "name": "assertions", "status": "fail",
                "detail": {"message": "重放过程中有断言被违反（id + 首次违反的 tick 如下）",
                           "violations": violations},
            }));
        }
    }

    // ---- 门 4：playtest（可选，声明 gates.playtest 才跑）----
    // 真跑一遍 playtest swarm（确定可复现）→ 聚合出报告 → 逐条核对清单声明的契约。
    // 没声明就跳过——现有 gate 行为完全不变（向后兼容）。
    if let Some(pt) = &gates.playtest {
        results.push(run_playtest_gate(dir, pt));
    }

    let pass = results.iter().all(|g| g["status"] == json!("pass"));
    let report = json!({
        "pass": pass,
        "project": project.manifest.name,
        "gates": results,
    });
    Ok((report, pass))
}

/// 重放一条通关录像：文件在、能解析、长度合规、逐校验点一致、must_emit 事件出现。
/// 同时在每个 tick 上对断言集全量求值，违反记进 `violations`（按 id 去抖：
/// 从健康翻到违反那一刻记一条，持续违反不刷屏）。
fn run_playthrough(
    dir: &Path,
    entry: &vitric_data::PlaythroughGate,
    gates: &Gates,
    checker: &Engine,
    assertions: &[Assertion],
    violations: &mut Vec<Value>,
) -> Result<Value, String> {
    let rec_path = dir.join(&entry.recording);
    let text = std::fs::read_to_string(&rec_path).map_err(|e| {
        format!(
            "录像文件 {} 读取失败: {e}。提示：录像来自 vitric run <项目目录> --record {}（QA/导演真打一局通关）",
            entry.recording, entry.recording
        )
    })?;
    let rec: Recording =
        serde_json::from_str(&text).map_err(|e| format!("录像 {} 解析失败: {e}", entry.recording))?;

    // 长度上限：防"挂机注水"——无限长的录像不是有效率的通关证明
    if let Some(max) = gates.max_ticks {
        if rec.ticks > max {
            return Err(format!(
                "录像 {} 长 {} tick，超过 gates.max_ticks 上限 {max}。\
                 提示：录一局更短的通关，或上调清单里的 max_ticks",
                entry.recording, rec.ticks
            ));
        }
    }

    // 全新世界重放：证书必须从项目数据冷启动可复现，不依赖任何现场状态
    let (mut sim, mut rt) = Runtime::boot(dir)?;
    let mut emitted = false;
    let mut failing = std::collections::BTreeSet::new();
    sim.replay_observed(&rec, &mut rt, |tick, world, step_events, observed| {
        // 事件收集：step 喂进逻辑的（输入/碰撞/start）+ 规则脚本 emit 的，两路都算观测
        emitted = emitted
            || step_events.iter().any(|e| e.name == entry.must_emit)
            || observed.iter().any(|e| e.name == entry.must_emit);
        for (id, conds) in assertions {
            match checker.check(world, conds) {
                Ok(true) => {
                    failing.remove(id);
                }
                Ok(false) => {
                    if failing.insert(id.clone()) {
                        violations.push(json!({
                            "id": id, "tick": tick, "kind": "violated",
                            "recording": entry.recording,
                        }));
                    }
                }
                // 求值失败（引用的实体没了等）也算违反，但要说清原因
                Err(e) => {
                    if failing.insert(id.clone()) {
                        violations.push(json!({
                            "id": id, "tick": tick, "kind": "eval-error",
                            "recording": entry.recording, "detail": e.to_string(),
                        }));
                    }
                }
            }
        }
    })
    // 跑偏 = 录像被篡改或逻辑混入非确定性，原样透出 SimError 的定位信息
    .map_err(|e| e.to_string())?;

    if !emitted {
        return Err(format!(
            "重放逐位一致，但全程未观测到事件 {:?}——这份录像不是一局通关局。\
             提示：录制时要真打到触发 {:?} 再停（事件名可在 gates.playthroughs[].must_emit 改）",
            entry.must_emit, entry.must_emit
        ));
    }
    Ok(json!({
        "ticks": rec.ticks,
        "final_hash": format!("{:#018x}", rec.final_hash),
        "must_emit": entry.must_emit,
        "verified": true,
    }))
}

/// playtest 门：按清单 `gates.playtest` 配置真跑一遍 playtest（swarm / lookahead / 种子探索），
/// 聚合出报告，再逐条核对声明的断言。返回一条 `{"name":"playtest","status":..,"detail":..}`。
///
/// 复用 cmd_playtest 同款 boot/工厂/聚合写法（不重写 swarm）：每条 spec 在调用线程内 boot 一份
/// 全新运行时，结果可复现（确定性铁律）。pass 时 detail 给关键指标；fail 时 detail 列出违反的
/// 断言名 + 实际值（带可对账的代表 strategy/seed），不内联整坨录像。
///
/// 任何一步出错（boot / 跑批 / 读种子录像）都按门失败处理（detail 给错误原因），不 panic。
fn run_playtest_gate(dir: &Path, pt: &PlaytestGate) -> Value {
    match playtest_report(dir, pt) {
        Ok(report) => judge_playtest(pt, &report),
        Err(e) => json!({"name": "playtest", "status": "fail", "detail": {"error": e}}),
    }
}

/// 按 pt 配置跑 playtest 出报告（boot/工厂/聚合照搬 cmd_playtest）。
fn playtest_report(dir: &Path, pt: &PlaytestGate) -> Result<Report, String> {
    // 项目根 playtest.json（存在即用，否则默认配置=自动推视图）——和 cmd_playtest 同口径。
    let config = PlaytestConfig::load(dir)?.unwrap_or_default();
    // 清单声明的权威通关事件（gates.playthroughs[].must_emit）：脚本/LLM 游戏的胜利事件
    // 不在通用默认 TerminalSpec 里，靠它并进 win 集合，否则会被误判"谁也通不了"。
    let manifest_must_emit: Vec<String> = match Project::load(dir) {
        Ok(project) => project
            .manifest
            .gates
            .as_ref()
            .map(|g| g.playthroughs.iter().map(|p| p.must_emit.clone()).collect())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let terminal = match &config.terminal {
        Some(ovr) => TerminalSpec::default().apply_override(ovr),
        None => TerminalSpec::default(),
    }
    .with_manifest_must_emit(&manifest_must_emit);

    // 工厂闭包：每个工作线程在自己线程内 boot 一份全新运行时（QuickJS 非 Send，运行时不跨线程）。
    let factory = || -> Result<(_, _, _), String> {
        let (sim, rt) = Runtime::boot(dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);

    // 跑法分流：种子录像 > lookahead > 默认策略组 swarm。
    let results = if let Some(seed_rel) = &pt.seed_recording {
        // 种子式探索：以这条录像为基线，扰动出 sessions 条变异并行跑。
        let seed_path = dir.join(seed_rel);
        let rec_text = std::fs::read_to_string(&seed_path)
            .map_err(|e| format!("读取种子录像 {seed_rel} 失败: {e}"))?;
        let seed_rec: Recording =
            serde_json::from_str(&rec_text).map_err(|e| format!("种子录像 {seed_rel} 解析失败: {e}"))?;
        let plan = perturb_plan(&seed_rec, pt.sessions, 0);
        run_seed_swarm(
            factory,
            &plan,
            &seed_rec.replies,
            pt.max_ticks,
            terminal.clone(),
            1, // explore_seed：截断脚本的 random 发散播种（与扰动 PCG 错开）
            threads,
        )?
    } else if pt.strategy.as_deref() == Some("lookahead") {
        // lookahead：跑 sessions 局前瞻搜索（每局 seed 递增）。lookahead 贵，不进 swarm 轮换，
        // 这里按声明显式跑——每局自己 boot，串行（前瞻本身已是重计算）。
        let mut out = Vec::with_capacity(pt.sessions);
        for k in 0..pt.sessions {
            let (mut sim, mut rt) = Runtime::boot(dir)?;
            let engine = rt.rules.clone();
            let cfg = SessionConfig {
                max_ticks: pt.max_ticks,
                seed: k as u64,
                terminal: terminal.clone(),
                playtest: config.clone(),
                ..Default::default()
            };
            let result = run_session_lookahead(
                &mut sim,
                &mut rt,
                &engine,
                &cfg,
                &LookaheadConfig { horizon: pt.horizon },
            )?;
            // lookahead 局贴 Coverage 标签只是占位（spec 仅用于聚合分组/对账，不影响结果）。
            let spec =
                SessionSpec::new(StrategyKind::Coverage, k as u64, pt.max_ticks);
            out.push(LabeledResult { spec, result });
        }
        out
    } else {
        // 默认策略组 swarm：四策略轮换 × 递增 seed 凑够 sessions 局。
        let mut plan: Vec<SessionSpec> = Vec::with_capacity(pt.sessions);
        for k in 0..pt.sessions {
            let kind = StrategyKind::ALL[k % StrategyKind::ALL.len()];
            plan.push(SessionSpec {
                strategy_kind: kind,
                seed: k as u64,
                max_ticks: pt.max_ticks,
                terminal: terminal.clone(),
            });
        }
        run_swarm_with_config(factory, &plan, &config, threads)?
    };

    // 结局覆盖要扫规则声明的结局集合，单独 boot 一份只读 Engine 喂聚合器（同 cmd_playtest）。
    let (_, rt) = Runtime::boot(dir)?;
    Ok(aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &manifest_must_emit))
}

/// 逐条核对声明的断言：每条只有在 pt 里填了才查（没填的维度不参与裁决）。
/// 全过 push pass（detail 给关键指标）；任一不达标 push fail（detail 列违反项 + 实际值）。
fn judge_playtest(pt: &PlaytestGate, report: &Report) -> Value {
    let dist = &report.outcome_distribution;
    let unreachable = report
        .ending_coverage
        .as_ref()
        .map(|ec| ec.unreachable_endings.len())
        .unwrap_or(0);
    let nb = &report.numeric_breakage;

    let mut violations: Vec<Value> = Vec::new();

    // 能通关：通关率必须 > 0
    if pt.require_clearable == Some(true) && dist.win == 0 {
        violations.push(json!({
            "assertion": "require_clearable",
            "expected": "win_rate > 0（swarm 至少通关一次）",
            "actual": {"win": dist.win, "win_rate": dist.win_rate},
        }));
    }
    // 通关率下限
    if let Some(min) = pt.min_clear_rate {
        if dist.win_rate < min {
            violations.push(json!({
                "assertion": "min_clear_rate",
                "expected": min, "actual": dist.win_rate,
            }));
        }
    }
    // 软锁簇数上限
    if let Some(max) = pt.max_soft_locks {
        let n = report.stuck_clusters.len();
        if n > max {
            violations.push(json!({
                "assertion": "max_soft_locks",
                "expected": max, "actual": n,
            }));
        }
    }
    // 不可达结局数上限
    if let Some(max) = pt.max_unreachable_endings {
        if unreachable > max {
            violations.push(json!({
                "assertion": "max_unreachable_endings",
                "expected": max, "actual": unreachable,
                "endings": report.ending_coverage.as_ref().map(|ec| ec.unreachable_endings.clone()),
            }));
        }
    }
    // 惰性动作数上限
    if let Some(max) = pt.max_inert_actions {
        let n = report.inert_actions.len();
        if n > max {
            violations.push(json!({
                "assertion": "max_inert_actions",
                "expected": max, "actual": n, "actions": report.inert_actions,
            }));
        }
    }
    // 数值崩必须全空
    if pt.forbid_numeric_breakage == Some(true) {
        let total = nb.runaway.len() + nb.collapse.len() + nb.non_finite.len();
        if total > 0 {
            violations.push(json!({
                "assertion": "forbid_numeric_breakage",
                "expected": "runaway/collapse/non_finite 全空",
                "actual": {
                    "runaway": nb.runaway.len(),
                    "collapse": nb.collapse.len(),
                    "non_finite": nb.non_finite.len(),
                },
            }));
        }
    }

    // 关键指标摘要（pass/fail 都带，便于对账与复现）。
    let metrics = json!({
        "sessions": report.sessions,
        "win_rate": dist.win_rate,
        "wins": dist.win,
        "soft_locks": report.stuck_clusters.len(),
        "unreachable_endings": unreachable,
        "inert_actions": report.inert_actions.len(),
        "numeric_breakage": {
            "runaway": nb.runaway.len(),
            "collapse": nb.collapse.len(),
            "non_finite": nb.non_finite.len(),
        },
    });

    if violations.is_empty() {
        json!({"name": "playtest", "status": "pass", "detail": metrics})
    } else {
        json!({
            "name": "playtest", "status": "fail",
            "detail": {"message": "playtest 门有声明的契约未达标", "violations": violations, "metrics": metrics},
        })
    }
}

/// 读断言集文件：`[{"id": "...", "if": [[左, op, 右], ...]}, ...]`。
/// 格式错误显式报到条目级，不静默丢弃——丢一条断言 = 门禁悄悄变松。
fn load_assertions(dir: &Path, rel: &str) -> Result<Vec<Assertion>, String> {
    let text = std::fs::read_to_string(dir.join(rel))
        .map_err(|e| format!("断言集 {rel} 读取失败: {e}。提示：gates.assertions 路径相对项目根目录"))?;
    let doc: Value =
        serde_json::from_str(&text).map_err(|e| format!("断言集 {rel} JSON 解析失败: {e}"))?;
    let arr = doc
        .as_array()
        .ok_or_else(|| format!("断言集 {rel} 顶层必须是数组: [{{\"id\", \"if\": [[左,op,右]...]}}]"))?;
    let mut out = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("{rel}#/{i} 缺少 id 字符串"))?;
        let conds = item.get("if").ok_or_else(|| format!("{rel}#/{i} 缺少 if 条件数组"))?;
        let conds = parse_conditions(conds).map_err(|e| format!("{rel}#/{i}: {e}"))?;
        out.push((id.to_string(), conds));
    }
    Ok(out)
}

/// 条件数组解析（同控制面 assert/add：[[左, 操作符, 右], ...]，exists/!exists 可两元）。
fn parse_conditions(v: &Value) -> Result<Vec<(String, String, Value)>, String> {
    let arr = v
        .as_array()
        .ok_or("if 必须是条件数组，如 [[\"@player.Health.hp\", \">=\", 0]]")?;
    let mut out = Vec::new();
    for (i, cond) in arr.iter().enumerate() {
        let parts = cond.as_array().filter(|p| p.len() == 2 || p.len() == 3);
        let parts = parts.ok_or_else(|| {
            format!("if[{i}] 必须是 [路径, 操作符, 值] 三元组（exists/!exists 可两元）")
        })?;
        let left = parts[0].as_str().ok_or_else(|| format!("if[{i}][0] 必须是路径字符串"))?;
        let op = parts[1].as_str().ok_or_else(|| format!("if[{i}][1] 必须是操作符字符串"))?;
        out.push((left.to_string(), op.to_string(), parts.get(2).cloned().unwrap_or(Value::Null)));
    }
    Ok(out)
}
