//! `vitric gate` — delivery gate.
//!
//! Stance: games are made by agents (AI), so "done" cannot rely on agent self-report — the engine must **mechanically**
//! verify delivery quality. The core of the gate is deterministic recording: a recording that can be replayed bit-by-bit
//! across checkpoint points, and during replay actually triggered the terminal event (default game-won), is an
//! **unforgeable clear certificate** — to forge any single frame, the state hash must diverge at the next checkpoint.
//!
//! Four gates (declared in manifest `gates` field, see [`vitric_data::Gates`] in vitric-data):
//! 1. check gate: full project verification (same as vitric check), any error = FAIL;
//! 2. clear recording gate: each recording is independently replayed, checkpoints consistent + must_emit event occurred + length ≤ max_ticks;
//! 3. assertion gate (optional): assertion set fully evaluated every tick during replay, any moment of violation = FAIL;
//! 4. playtest gate (optional, runs only when `gates.playtest` is declared): actually runs a playtest swarm (deterministic & reproducible),
//!    aggregates a report, then checks each contract declared in the manifest (clearable or not, soft-locks, unreachable endings, inert actions,
//!    numeric collapse) — turns "auto-clearing the floor" into a delivery contract, any failure = FAIL.
//!
//! Projects that don't declare gates are rejected outright — no gate, no certificate; an empty gate is a backdoor.

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

/// An assertion: id + list of condition triples (all hold = healthy). Format same as control-plane assert/add.
type Assertion = (String, Vec<(String, String, Value)>);

/// Run all gates. Returns (JSON report, whether all passed); Err is used only when "the gate itself cannot be established"
/// (directory has no manifest / manifest doesn't declare gates) — these must be explicit hard errors, not a pass=false report.
pub fn run(dir: &Path) -> Result<(Value, bool), String> {
    let project = Project::load(dir).map_err(|r| r.to_string())?;
    // Constraint: without gate declarations there's no machine-verifiable delivery standard, gate refuses to issue a certificate.
    // Empty playthroughs likewise — the certificate body is the clear recording; a gate without recordings is an empty gate.
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

    // ---- Gate 1: check (full project verification, same kernel as vitric check) ----
    if gates.check {
        match runtime::check(dir) {
            Ok(report) => results.push(json!({
                "name": "check", "status": "pass",
                "detail": {"entities": report["entities"], "initial_hash": report["initial_hash"]},
            })),
            Err(e) => results.push(json!({"name": "check", "status": "fail", "detail": e})),
        }
    }

    // ---- Assertion set loading (evaluation happens during replay) ----
    let mut assertions: Vec<Assertion> = Vec::new();
    let mut assertions_gate_error: Option<String> = None;
    if let Some(rel) = &gates.assertions {
        match load_assertions(dir, rel) {
            Ok(list) => assertions = list,
            // Assertion file declared but unreadable: this is the assertion gate's own failure, cannot silently treat as "no assertions"
            Err(e) => assertions_gate_error = Some(e),
        }
    }
    // Assertion condition evaluation reuses the rule engine's Engine::check (empty rule set + project schema, same as control plane)
    let checker = Engine::new(RuleSet::default(), project.schema.clone());
    let mut violations: Vec<Value> = Vec::new();

    // ---- Gate 2: clear recording (each independently replayed and verified) ----
    for entry in &gates.playthroughs {
        let name = format!("playthrough:{}", entry.recording);
        match run_playthrough(dir, entry, &gates, &checker, &assertions, &mut violations) {
            Ok(detail) => results.push(json!({"name": name, "status": "pass", "detail": detail})),
            Err(e) => results.push(json!({"name": name, "status": "fail", "detail": e})),
        }
    }

    // ---- Gate 3: assertions (violation details collected during replay, aggregated here for verdict) ----
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

    // ---- Gate 4: playtest (optional, runs only when gates.playtest is declared) ----
    // Actually runs a playtest swarm (deterministic & reproducible) → aggregates a report → checks each contract declared in the manifest.
    // Skipped if not declared — existing gate behavior unchanged (backward compatible).
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

/// Replay a clear recording: file exists, parseable, length in range, checkpoints consistent, must_emit event occurred.
/// Also fully evaluates the assertion set on every tick, recording violations into `violations` (debounced by id:
/// records one entry at the moment of healthy→violated transition, continuous violations don't flood the log).
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

    // Length upper bound: prevent "AFK inflation" — infinitely long recordings aren't an efficient clear proof
    if let Some(max) = gates.max_ticks {
        if rec.ticks > max {
            return Err(format!(
                "录像 {} 长 {} tick，超过 gates.max_ticks 上限 {max}。\
                 提示：录一局更短的通关，或上调清单里的 max_ticks",
                entry.recording, rec.ticks
            ));
        }
    }

    // Replay in a fresh world: the certificate must be reproducible from a cold start of project data, not depending on any live state
    let (mut sim, mut rt) = Runtime::boot(dir)?;
    let mut emitted = false;
    let mut failing = std::collections::BTreeSet::new();
    sim.replay_observed(&rec, &mut rt, |tick, world, step_events, observed| {
        // Event collection: step-fed (input/collision/start) + rule-script-emitted, both paths count as observed
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
                // Evaluation failure (referenced entity gone, etc.) also counts as a violation, but the cause must be clear
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
    // Divergence = recording tampered or non-determinism leaked into logic; pass through SimError's localization info as-is
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

/// playtest gate: per manifest `gates.playtest` config, actually runs a playtest (swarm / lookahead / seed exploration),
/// aggregates a report, then checks each declared assertion. Returns one `{"name":"playtest","status":..,"detail":..}` entry.
///
/// Reuses the same boot/factory/aggregation code as cmd_playtest (no swarm rewrite): each spec boots a fresh
/// runtime in the calling thread, results are reproducible (determinism hard rule). On pass, detail gives key metrics; on fail, detail lists violated
/// assertion names + actual values (with representative strategy/seed for reconciliation), without inlining the entire recording blob.
///
/// Any step error (boot / batch run / seed recording read) is treated as gate failure (detail gives the error cause), no panic.
fn run_playtest_gate(dir: &Path, pt: &PlaytestGate) -> Value {
    match playtest_report(dir, pt) {
        Ok(report) => judge_playtest(pt, &report),
        Err(e) => json!({"name": "playtest", "status": "fail", "detail": {"error": e}}),
    }
}

/// Run a playtest per pt config and produce a report (boot/factory/aggregation copied from cmd_playtest).
fn playtest_report(dir: &Path, pt: &PlaytestGate) -> Result<Report, String> {
    // Project-root playtest.json (used if present, else default config = auto-inferred view) — same caliber as cmd_playtest.
    let config = PlaytestConfig::load(dir)?.unwrap_or_default();
    // Manifest-declared authoritative clear event (gates.playthroughs[].must_emit): win events of scripted/LLM games
    // are not in the generic default TerminalSpec, so they're merged into the win set, otherwise it's misjudged as "nobody can clear".
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

    // Factory closure: each worker thread boots a fresh runtime in its own thread (QuickJS is not Send, runtime does not cross threads).
    let factory = || -> Result<(_, _, _), String> {
        let (sim, rt) = Runtime::boot(dir)?;
        let engine = rt.rules.clone();
        Ok((sim, rt, engine))
    };
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);

    // Run-mode branching: seed recording > lookahead > default strategy-group swarm.
    let results = if let Some(seed_rel) = &pt.seed_recording {
        // Seed-based exploration: uses this recording as baseline, perturbs `sessions` variants and runs in parallel.
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
            1, // explore_seed: seeding for the truncated script's random divergence (offset from perturbation PCG)
            threads,
        )?
    } else if pt.strategy.as_deref() == Some("lookahead") {
        // lookahead: runs `sessions` game-tree search planners (seed increments per game). Lookahead is expensive, not in swarm rotation,
        // explicitly runs per declaration here — each game boots itself, serial (tree search is already heavy compute).
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
                // Manifest horizon field semantics is already "search depth", beam is beam-width (both backward compatible with defaults).
                &LookaheadConfig { depth: pt.horizon, beam_width: pt.beam },
            )?;
            // lookahead games tag Coverage just as a placeholder (spec is only for aggregation grouping/reconciliation, doesn't affect results).
            let spec =
                SessionSpec::new(StrategyKind::Coverage, k as u64, pt.max_ticks);
            out.push(LabeledResult { spec, result });
        }
        out
    } else {
        // Default strategy-group swarm: four-strategy rotation × incrementing seed to fill `sessions` games.
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

    // Ending coverage needs to scan the rule-declared ending set, separately boot a read-only Engine to feed the aggregator (same as cmd_playtest).
    let (_, rt) = Runtime::boot(dir)?;
    Ok(aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &manifest_must_emit))
}

/// Check each declared assertion one by one: each is checked only if filled in pt (unfilled dimensions don't participate in the verdict).
/// All pass → push pass (detail gives key metrics); any failure → push fail (detail lists violation items + actual values).
fn judge_playtest(pt: &PlaytestGate, report: &Report) -> Value {
    let dist = &report.outcome_distribution;
    let unreachable = report
        .ending_coverage
        .as_ref()
        .map(|ec| ec.unreachable_endings.len())
        .unwrap_or(0);
    let nb = &report.numeric_breakage;

    let mut violations: Vec<Value> = Vec::new();

    // Clearable: clear rate must be > 0
    if pt.require_clearable == Some(true) && dist.win == 0 {
        violations.push(json!({
            "assertion": "require_clearable",
            "expected": "win_rate > 0（swarm 至少通关一次）",
            "actual": {"win": dist.win, "win_rate": dist.win_rate},
        }));
    }
    // Clear rate lower bound
    if let Some(min) = pt.min_clear_rate {
        if dist.win_rate < min {
            violations.push(json!({
                "assertion": "min_clear_rate",
                "expected": min, "actual": dist.win_rate,
            }));
        }
    }
    // Upper bound on soft-lock cluster count
    if let Some(max) = pt.max_soft_locks {
        let n = report.stuck_clusters.len();
        if n > max {
            violations.push(json!({
                "assertion": "max_soft_locks",
                "expected": max, "actual": n,
            }));
        }
    }
    // Upper bound on unreachable ending count
    if let Some(max) = pt.max_unreachable_endings {
        if unreachable > max {
            violations.push(json!({
                "assertion": "max_unreachable_endings",
                "expected": max, "actual": unreachable,
                "endings": report.ending_coverage.as_ref().map(|ec| ec.unreachable_endings.clone()),
            }));
        }
    }
    // Upper bound on inert action count
    if let Some(max) = pt.max_inert_actions {
        let n = report.inert_actions.len();
        if n > max {
            violations.push(json!({
                "assertion": "max_inert_actions",
                "expected": max, "actual": n, "actions": report.inert_actions,
            }));
        }
    }
    // Numeric breakage must all be empty
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

    // Key metrics summary (included for both pass/fail, for reconciliation and reproducibility).
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

/// Read assertion set file: `[{"id": "...", "if": [[left, op, right], ...]}, ...]`.
/// Format errors are explicitly reported at the entry level, never silently dropped —
/// dropping one assertion = silently loosening the gate.
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

/// Parse condition array (same as control plane assert/add: [[left, operator, right], ...],
/// exists/!exists may be two-element).
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
