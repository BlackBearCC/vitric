//! `vitric gate` — 交付门禁。
//!
//! 立场：游戏是 agent（AI）做的，"做完了"不能靠 agent 自述——引擎必须**机械地**
//! 验证交付质量。门禁的核心是确定性录像：一份能逐校验点逐位重放、且重放过程中
//! 真的触发了终局事件（默认 game-won）的录像，就是一张**不可伪造的通关证书**——
//! 想伪造任何一帧，状态哈希必然在下一个校验点跑偏。
//!
//! 三道门（清单 `gates` 字段声明，见 vitric-data 的 [`vitric_data::Gates`]）：
//! 1. check 门：完整项目校验（vitric check 同款），任何错误 = FAIL；
//! 2. 通关录像门：每条录像独立重放，校验点一致 + must_emit 事件出现 + 长度 ≤ max_ticks；
//! 3. 断言门（可选）：重放过程中每个 tick 全量求值断言集，任何一刻违反 = FAIL。
//!
//! 没有声明 gates 的项目直接拒绝——无门禁项目不出证书，空门禁放行就是后门。

use std::path::Path;

use serde_json::{json, Value};

use vitric_data::{Gates, Project};
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
