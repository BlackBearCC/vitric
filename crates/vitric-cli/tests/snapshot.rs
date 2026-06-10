//! 快照完整性：在"脏时刻"打快照——carryover（脚本上一 tick 发的事件）非空、
//! pending_inputs（已注入未消化的输入）非空——restore 后继续跑必须和原轨迹逐位一致。
//! 干净 tick 边界的快照测试抓不到这两个口子，这里专门堵。

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;

/// 最小项目：脚本每 tick emit 一个 pulse 事件（必然滞留 carryover 跨 tick），
/// 规则把 pulse 和 input 都计入计数器（两者丢失都会改变状态哈希）。
fn write_project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitric-snap-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    for sub in ["scenes", "rules", "scripts"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        dir.join("vitric.json"),
        json!({
            "name": "snap-test",
            "schema": "schema.json",
            "entry": "scenes/main.json",
            "scenes": ["scenes/main.json"],
            "rules": ["rules/game.json"],
            "scripts": ["scripts/systems.js"],
            "seed": 7
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("schema.json"),
        json!({"components": {
            "Pulse": {"fields": {"count": {"type": "int", "default": 0}}}
        }})
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("scenes/main.json"),
        json!({"entities": [{"name": "counter", "components": {"Pulse": {}}}]}).to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("rules/game.json"),
        json!({"rules": [
            {"id": "count-pulse", "on": {"event": "pulse"},
             "do": [{"add": "@counter.Pulse.count", "by": 1}]},
            {"id": "count-input", "on": {"event": "input", "filter": {"action": "right", "phase": "pressed"}},
             "do": [{"add": "@counter.Pulse.count", "by": 100}]}
        ]})
        .to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("scripts/systems.js"),
        r#"vitric.system("pulser", { query: ["Pulse"], writes: [] }, (entities, ctx) => {
            ctx.emit("pulse", {});
        });"#,
    )
    .unwrap();
    dir
}

#[test]
fn dirty_moment_snapshot_restores_identical_trajectory() {
    let dir = write_project("dirty");
    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();
    for _ in 0..30 {
        sim.step(&mut rt).unwrap();
    }
    // 制造脏时刻：有未消化的输入，carryover 里有上一 tick 的 pulse
    sim.inject_input("right", "pressed");
    let snap = sim.snapshot(&rt);

    // 原轨迹继续跑 30 tick
    for _ in 0..30 {
        sim.step(&mut rt).unwrap();
    }
    let h_direct = sim.world.state_hash();
    let count_direct = {
        let c = sim.world.entity("counter").unwrap();
        sim.world.get_field(c, "Pulse.count").unwrap().clone()
    };

    // 全新进程语义：重 boot 再 restore，再跑 30 tick——必须逐位一致
    let (mut sim2, mut rt2) = Runtime::boot(&dir).unwrap();
    sim2.restore(&snap, &mut rt2).unwrap();
    for _ in 0..30 {
        sim2.step(&mut rt2).unwrap();
    }
    assert_eq!(sim2.world.state_hash(), h_direct, "restore 后轨迹分歧（快照漏状态）");
    let c2 = sim2.world.entity("counter").unwrap();
    assert_eq!(sim2.world.get_field(c2, "Pulse.count").unwrap(), &count_direct);
    // 注入的输入确实生效了（+100 在两边都发生）
    assert!(count_direct.as_i64().unwrap() >= 100, "pending input 在快照里丢了");

    let _ = fs::remove_dir_all(&dir);
}
