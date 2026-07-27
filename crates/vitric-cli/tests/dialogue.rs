//! End-to-end test for the dialogue module.
//!
//! Drives the dialogue-demo: player walks into the elder → dialogue starts at node 0,
//! presses "1" three times to advance through the 3-node tree and end the conversation.
//! Verifies the full dialogue state machine (inactive → node 0 → 1 → 2 → ended), the
//! Talked.count increment (composition seam with the quest module), and the HUD.

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/dialogue-demo")
}

#[test]
fn dialogue_demo_check_passes() {
    let (_sim, _rt) = Runtime::boot(&demo_dir()).expect("dialogue-demo 应通过校验并启动");
}

#[test]
fn dialogue_full_tree_advance_and_end() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Phase 1: walk right into the elder. Player (speed 60 = 1 unit/tick) collides at
    // tick 2 (x=2, elder x=3 w=3 → overlap zone x∈[2,4]). talk-on-collision fires
    // (current=-1 < 0) → __dialogue_start → current=0 (entry node).
    sim.inject_input("right", "pressed");
    for _ in 0..3 {
        sim.step(&mut rt).unwrap();
    }

    let player = sim.world.entity("player").unwrap();
    let current = sim.world.get_field(player, "DialogueRunner.current").unwrap().clone();
    assert_eq!(current, json!(0), "碰撞后应进入对话节点 0");
    let active_npc = sim.world.get_field(player, "DialogueRunner.active_npc").unwrap().clone();
    assert!(
        !active_npc.as_str().unwrap_or("").is_empty(),
        "active_npc 应已设置: {active_npc}"
    );

    // Phase 2: press "1" → pick choice 0 at node 0 → node_next[0]="1;-1" → advance to 1.
    sim.inject_input("1", "pressed");
    sim.step(&mut rt).unwrap();
    let current = sim.world.get_field(player, "DialogueRunner.current").unwrap().clone();
    assert_eq!(current, json!(1), "选 0 后应推进到节点 1");

    // Phase 3: press "1" → node 1 → node_next[1]="2;-1" → advance to 2.
    sim.inject_input("1", "pressed");
    sim.step(&mut rt).unwrap();
    let current = sim.world.get_field(player, "DialogueRunner.current").unwrap().clone();
    assert_eq!(current, json!(2), "选 0 后应推进到节点 2");

    // Phase 4: press "1" → node 2 → node_next[2]="-1" → end dialogue.
    sim.inject_input("1", "pressed");
    sim.step(&mut rt).unwrap();
    let current = sim.world.get_field(player, "DialogueRunner.current").unwrap().clone();
    assert_eq!(current, json!(-1), "节点 2 选 0 应结束对话（current 回到 -1）");

    // Talked.count should be 1 — the dialogue-end hook incremented it.
    let elder = sim.world.entity("elder").unwrap();
    let talked = sim.world.get_field(elder, "Talked.count").unwrap().clone();
    assert_eq!(talked, json!(1), "对话结束应使 Talked.count +1（quest 组合接缝）");

    // Step one more tick so the HUD (rendered via a tick rule) catches up to the new
    // state. Within the tick that ended the dialogue, render_dialogue_hud ran before
    // the deferred current=-1 write was visible, so it still showed node 2.
    sim.step(&mut rt).unwrap();

    // HUD should reflect "talked" state.
    let hud = sim.world.entity("hud").unwrap();
    let hud_text = sim.world.get_field(hud, "Text.content").unwrap();
    let hud_str = hud_text.as_str().unwrap();
    assert!(
        hud_str.contains("Talked") || hud_str.contains("talked"),
        "HUD 应显示已对话状态: {hud_str}"
    );
}

#[test]
fn dialogue_choose_while_inactive_is_noop() {
    // Pressing "1" when not in a dialogue should do nothing (no crash, no state change).
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    sim.inject_input("1", "pressed");
    sim.step(&mut rt).unwrap();

    let player = sim.world.entity("player").unwrap();
    let current = sim.world.get_field(player, "DialogueRunner.current").unwrap().clone();
    assert_eq!(current, json!(-1), "不在对话中按 1 不应有任何效果");
    let elder = sim.world.entity("elder").unwrap();
    let talked = sim.world.get_field(elder, "Talked.count").unwrap().clone();
    assert_eq!(talked, json!(0), "不在对话中按 1 不应增加 Talked.count");
}

#[test]
fn dialogue_second_conversation_increments_talked_again() {
    // After ending one conversation, walk into the elder again → a new conversation
    // starts, ends, and Talked.count goes to 2.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // First conversation: walk in, press 1 three times.
    sim.inject_input("right", "pressed");
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();
    // Stop walking so we stay near the elder.
    sim.inject_input("right", "released");
    for _ in 0..3 {
        sim.inject_input("1", "pressed");
        sim.step(&mut rt).unwrap();
    }

    let elder = sim.world.entity("elder").unwrap();
    let talked = sim.world.get_field(elder, "Talked.count").unwrap().clone();
    assert_eq!(talked, json!(1), "第一次对话后 Talked.count 应为 1");

    // The player is still on the elder (stopped at x=3). Walk right a bit to exit,
    // then walk left back in to re-trigger talk (current is -1 now, so it re-starts).
    // Actually, since current=-1, the next collision with the elder re-emits talk.
    // The player is at x=3 (elder zone). Collisions fire every tick → talk re-emits
    // next tick. So just step once and a new dialogue starts.
    sim.step(&mut rt).unwrap();
    let player = sim.world.entity("player").unwrap();
    let current = sim.world.get_field(player, "DialogueRunner.current").unwrap().clone();
    assert_eq!(current, json!(0), "再次碰撞应启动第二次对话（current=-1 时 talk 重新生效）");

    // End the second conversation.
    for _ in 0..3 {
        sim.inject_input("1", "pressed");
        sim.step(&mut rt).unwrap();
    }
    let talked = sim.world.get_field(elder, "Talked.count").unwrap().clone();
    assert_eq!(talked, json!(2), "第二次对话后 Talked.count 应为 2");
}
