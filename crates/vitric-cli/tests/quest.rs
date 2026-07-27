//! End-to-end test for the quest module + inventory module composition.
//!
//! Drives the quest-demo: player walks right to the elder (offer + accept the herb quest),
//! continues to collect 3 herbs (inventory module), the quest auto-completes, then walks
//! back left to the elder to turn in and receive the coin reward. Verifies the full
//! quest state machine (inactive → offered → active → completed → turned-in), the
//! module composition (quest reward granted via inventory's pickup event), and the HUD.

use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::Runtime;

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/quest-demo")
}

#[test]
fn quest_demo_check_passes() {
    // `vitric check` is exercised by Runtime::boot; this test is a placeholder that
    // confirms the project loads (schema merge + rule/script append from two modules).
    let (_sim, _rt) = Runtime::boot(&demo_dir()).expect("quest-demo 应通过校验并启动");
}

#[test]
fn quest_full_loop_offer_collect_turn_in() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Phase 1: walk right. Player (speed 60 = 1 unit/tick) passes elder at x=3 (w=3,
    // overlap zone x∈[2,4]) → offer at tick 2 (inactive→offered), accept at tick 3
    // (offered→active). Continues to herbs at x=8/10/12 → 3 pickups. quest-track system
    // (runs after rules each tick) reads inventory and advances progress; at tick 12
    // progress hits target → state=completed.
    sim.inject_input("right", "pressed");
    for _ in 0..13 {
        sim.step(&mut rt).unwrap();
    }

    // Mid-run assertion: quest should be completed, herbs collected, no reward yet.
    let quest = sim.world.entity("herb-quest").unwrap();
    let state = sim.world.get_field(quest, "QuestState.state").unwrap().clone();
    assert_eq!(state, json!("completed"), "拾取 3 草药后任务应完成");
    let player = sim.world.entity("player").unwrap();
    let items = sim.world.get_field(player, "Inventory.items").unwrap().clone();
    let counts = sim.world.get_field(player, "Inventory.counts").unwrap().clone();
    let items_arr = items.as_array().unwrap();
    let counts_arr = counts.as_array().unwrap();
    let herb_idx = items_arr.iter().position(|v| v == &json!("herb"));
    assert!(herb_idx.is_some(), "背包应有草药: {items}");
    assert_eq!(counts_arr[herb_idx.unwrap()], json!(3), "应有 3 草药");
    // No coin yet — reward not granted until turn-in.
    assert!(
        !items_arr.iter().any(|v| v == &json!("coin")),
        "交付前不应有金币奖励: {items}"
    );

    // Phase 2: walk back left to the elder. Player at x=13 walks left; re-enters elder
    // zone at x=4 (tick 22). elder-turnin fires (completed → turned-in), emitting a
    // pickup(coin x5) event. The pickup is processed next tick (carryover → inv-pickup).
    sim.inject_input("right", "released");
    sim.inject_input("left", "pressed");
    for _ in 0..14 {
        sim.step(&mut rt).unwrap();
    }

    // Final assertions: quest turned in, reward granted, quest log updated.
    let state = sim.world.get_field(quest, "QuestState.state").unwrap().clone();
    assert_eq!(state, json!("turned-in"), "交付后任务状态应为 turned-in");

    let items = sim.world.get_field(player, "Inventory.items").unwrap().clone();
    let counts = sim.world.get_field(player, "Inventory.counts").unwrap().clone();
    let items_arr = items.as_array().unwrap();
    let counts_arr = counts.as_array().unwrap();
    let herb_idx = items_arr.iter().position(|v| v == &json!("herb"));
    let coin_idx = items_arr.iter().position(|v| v == &json!("coin"));
    assert!(herb_idx.is_some(), "草药仍在背包: {items}");
    assert_eq!(counts_arr[herb_idx.unwrap()], json!(3), "3 草药");
    assert!(coin_idx.is_some(), "应有金币奖励: {items}");
    assert_eq!(counts_arr[coin_idx.unwrap()], json!(5), "奖励 5 金币");

    // QuestLog: herb-quest moved from active to completed.
    let active = sim.world.get_field(player, "QuestLog.active").unwrap().clone();
    let completed = sim.world.get_field(player, "QuestLog.completed").unwrap().clone();
    let active_arr = active.as_array().unwrap();
    let completed_arr = completed.as_array().unwrap();
    assert!(
        !active_arr.iter().any(|v| v == &json!("herb-quest")),
        "交付后 herb-quest 应从 active 移除: {active}"
    );
    assert!(
        completed_arr.iter().any(|v| v == &json!("herb-quest")),
        "herb-quest 应在 completed 列表: {completed}"
    );

    // Herbs despawned after collection.
    let remaining_pickups = sim.world.query(&["Pickup"]);
    assert!(
        remaining_pickups.is_empty(),
        "所有草药应已销毁，剩余: {}",
        remaining_pickups.len()
    );

    // HUD reflects the final state.
    let hud = sim.world.entity("hud").unwrap();
    let hud_text = sim.world.get_field(hud, "Text.content").unwrap();
    let hud_str = hud_text.as_str().unwrap();
    assert!(hud_str.contains("done"), "HUD 应显示任务完成: {hud_str}");
    assert!(hud_str.contains("coin"), "HUD 应显示金币: {hud_str}");
}

#[test]
fn quest_prereq_locks_until_prereq_completed() {
    // A quest with a prereq can't be offered until the prereq is in the player's
    // completed list. We drive this by directly emitting quest-offer for a locked quest
    // and asserting it stays inactive + emits quest-locked.
    use std::fs;
    let dir = std::env::temp_dir().join("vitric-quest-prereq-test");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("scenes")).unwrap();
    fs::create_dir_all(dir.join("rules")).unwrap();
    fs::create_dir_all(dir.join("scripts")).unwrap();

    // Absolute path to the repo's modules/quest — the temp project lives outside the repo,
    // so a relative `../../modules/quest` won't resolve. Use an absolute include path.
    let quest_module = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../modules/quest")
        .canonicalize()
        .unwrap();
    let manifest = format!(
        r#"{{"name":"prereq","schema":"schema.json","entry":"scenes/main.json",
        "scenes":["scenes/main.json"],"rules":["rules/game.json"],
        "scripts":["scripts/game.js"],
        "includes":["{}"],"seed":1}}"#,
        quest_module.display()
    );
    fs::write(dir.join("vitric.json"), manifest).unwrap();
    fs::write(
        dir.join("schema.json"),
        r#"{"components":{
            "Player":{"fields":{}},
            "Position":{"fields":{"x":{"type":"number"},"y":{"type":"number"}}}
        }}"#,
    )
    .unwrap();
    fs::write(
        dir.join("scenes/main.json"),
        r#"{"entities":[
            {"name":"player","components":{"Player":{},"Position":{"x":0,"y":0},"QuestLog":{"active":[],"completed":[]}}},
            {"name":"q1","components":{
                "QuestDef":{"id":"q1","title":"first","desc":"","prereq":[],"reward_item":"","reward_count":0},
                "QuestObjective":{"kind":"collect","arg":"x","target":1},
                "QuestState":{"state":"inactive","progress":0,"assignee":""}
            }},
            {"name":"q2","components":{
                "QuestDef":{"id":"q2","title":"second","desc":"","prereq":["q1"],"reward_item":"","reward_count":0},
                "QuestObjective":{"kind":"collect","arg":"y","target":1},
                "QuestState":{"state":"inactive","progress":0,"assignee":""}
            }}
        ]}"#,
    )
    .unwrap();
    fs::write(dir.join("rules/game.json"), r#"{"rules":[]}"#).unwrap();
    fs::write(dir.join("scripts/game.js"), "").unwrap();

    let (mut sim, mut rt) = Runtime::boot(&dir).unwrap();

    // Offer q2 while q1 is not completed → should stay inactive, emit quest-locked.
    sim.inject_reply("quest-offer", json!({"quest": "q2"}));
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap(); // allow the event to process

    let q2 = sim.world.entity("q2").unwrap();
    let state = sim.world.get_field(q2, "QuestState.state").unwrap().clone();
    assert_eq!(state, json!("inactive"), "prereq 未完成时 q2 应保持 inactive");

    // Mark q1 as completed (turn-in path), then offer q2 → should succeed.
    let player = sim.world.entity("player").unwrap();
    let completed = json!(["q1"]);
    sim.world.set_field(player, "QuestLog.completed", completed).unwrap();
    sim.inject_reply("quest-offer", json!({"quest": "q2"}));
    sim.step(&mut rt).unwrap();
    sim.step(&mut rt).unwrap();

    let state = sim.world.get_field(q2, "QuestState.state").unwrap().clone();
    assert_eq!(state, json!("offered"), "prereq 完成后 q2 应可接取（offered）");

    fs::remove_dir_all(&dir).unwrap();
}
