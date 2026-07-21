//! Task 11 (Trading & Diplomacy) integration tests.
//!
//! Verifies: (a) faction-tick derives tier_<f> from relations JSON;
//! (b) change_relation applies delta with clamping (via trader-companion-relation rule on
//!     trade-available event);
//! (c) complete_trade executes barter (deducts give, adds receive, +2 relation);
//! (d) onNegotiateReply applies +3 relation with fallback canned line.
//!
//! Test setup follows the combat.rs / companions.rs / research.rs pattern:
//! `Runtime::boot(frontier_dir())` loads the full Frontier scene + logic. Tick 0 is reserved
//! for the `start` event (seed-start rule sets initial inventory: seed/wood/ore/fiber/plank/lamp).
//!
//! Deviations from the brief's literal test code (documented in task-11-report.md):
//! - Faction.relations is a schema `text` field. The brief's literal
//!   `set_field(_, "Faction.relations", json!({...}))` would store a JSON object, which makes
//!   faction-tick's `JSON.parse(c.Faction.relations || "{}")` throw and fall back to `{}`.
//!   The `set_relations` helper serializes to a JSON string first so the script can parse it.
//! - The `complete_trade` test sets wheat/fiber AFTER tick 0 so the seed-start rule (which
//!   overrides fiber to 2) doesn't clobber the test fixture.
//! - The `on_negotiate_reply` test captures the `llm-ask` event's `id` (embedded by the engine
//!   as `"<callback>#<tick>#<idx>"`) and injects it back in the `llm-reply`. The brief's
//!   literal inject lacked the `id` field, which `__onReply` (prelude.js) requires to dispatch.

use std::path::PathBuf;

use serde_json::{json, Value};

use vitric_cli::runtime::Runtime;
use vitric_sim::GameLogic;

fn frontier_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/frontier")
}

/// Helper: set a single field on a named entity.
fn set_field(sim: &mut vitric_sim::Sim, name: &str, path: &str, value: Value) {
    let id = sim.world.entity(name).expect("entity exists");
    sim.world.set_field(id, path, value).expect("set_field ok");
}

/// Helper: read a single field from a named entity as a cloned Value.
fn get_field(sim: &vitric_sim::Sim, name: &str, path: &str) -> Value {
    let id = sim.world.entity(name).expect("entity exists");
    sim.world.get_field(id, path).expect("get_field ok").clone()
}

/// Set Faction.relations on the colony entity from a JSON object.
/// Serializes the object to a JSON string first because the schema field is type "text"
/// (faction-tick parses it via JSON.parse on the JS side).
fn set_relations(sim: &mut vitric_sim::Sim, rels: Value) {
    let s = serde_json::to_string(&rels).expect("serialize relations");
    set_field(sim, "colony", "Faction.relations", Value::String(s));
}

#[test]
fn faction_tick_derives_tier_from_relations() {
    // Set Faction.relations to {"nomads":80,"caravan":50,"remnant":-60} ->
    // tier_nomads="allied" (>=76), tier_caravan="friendly" (>=41), tier_remnant="hostile" (<-49).
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_relations(&mut sim, json!({"nomads":80,"caravan":50,"remnant":-60}));
    sim.step(&mut rt).unwrap(); // faction-tick runs

    assert_eq!(get_field(&sim, "colony", "Faction.tier_nomads").as_str(), Some("allied"));
    assert_eq!(get_field(&sim, "colony", "Faction.tier_caravan").as_str(), Some("friendly"));
    assert_eq!(get_field(&sim, "colony", "Faction.tier_remnant").as_str(), Some("hostile"));
}

#[test]
fn change_relation_clamps_to_100() {
    // Set nomads to 99, inject trade-available (trader-companion-relation rule fires
    // change_relation("nomads", +1)) -> 100. Inject again -> clamp (no change, no event).
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_relations(&mut sim, json!({"nomads":99,"caravan":0,"remnant":-10}));

    sim.inject_reply("trade-available", json!({ "pid": "test", "role": "trader" }));
    sim.step(&mut rt).unwrap(); // +1 -> 100

    let rel_val = get_field(&sim, "colony", "Faction.relations");
    let rel = rel_val.as_str().unwrap();
    assert!(rel.contains("\"nomads\":100"), "expected nomads=100, got {}", rel);

    // Inject again — should clamp (no change, no event).
    sim.inject_reply("trade-available", json!({ "pid": "test", "role": "trader" }));
    sim.step(&mut rt).unwrap();
    let rel2_val = get_field(&sim, "colony", "Faction.relations");
    let rel2 = rel2_val.as_str().unwrap();
    assert!(rel2.contains("\"nomads\":100"), "expected nomads still 100 (clamped), got {}", rel2);
}

#[test]
fn complete_trade_barters_items_and_adds_relation() {
    // Player has 3 wheat. Trigger trade-nomads (give 3 wheat -> receive 2 fiber base, neutral mult 1.0).
    // After: player.Inventory.wheat=0, player.Inventory.fiber=2, nomads relation +2.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_relations(&mut sim, json!({"nomads":30,"caravan":0,"remnant":-10}));
    sim.step(&mut rt).unwrap(); // tick 0: settle (start event -> seed-start writes fiber=2; faction-tick derives tier_nomads=neutral)
    // Set wheat/fiber AFTER seed-start so the rule's start-event fixture doesn't override them.
    set_field(&mut sim, "player", "Inventory.wheat", json!(3));
    set_field(&mut sim, "player", "Inventory.fiber", json!(0));

    sim.inject_reply("ui-activate", json!({ "action": "trade-nomads" }));
    sim.step(&mut rt).unwrap(); // tick 1: complete_trade runs, emits inv-set, +2 relation (inline)
    sim.step(&mut rt).unwrap(); // tick 2: inv-apply rule writes back inventory

    let wheat = get_field(&sim, "player", "Inventory.wheat").as_i64().unwrap();
    let fiber = get_field(&sim, "player", "Inventory.fiber").as_i64().unwrap();
    assert_eq!(wheat, 0, "wheat should be 0 after trade");
    assert_eq!(fiber, 2, "fiber should be 2 after trade");

    let rel_val = get_field(&sim, "colony", "Faction.relations");
    let rel = rel_val.as_str().unwrap();
    assert!(rel.contains("\"nomads\":32"), "nomads relation should be 30+2=32, got {}", rel);
}

#[test]
fn on_negotiate_reply_applies_plus_3_relation() {
    // Call negotiate(nomads) -> ctx.ask("llm") stashes target="nomads" + emits llm-ask with
    // id "onNegotiateReply#<tick>#<idx>". Engine routes llm-reply -> __onReply -> onNegotiateReply.
    // For test: capture the llm-ask id, inject llm-reply with fallback text + id -> onNegotiateReply
    // runs -> detects fallback -> substitutes canned line -> +3 relation.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    set_relations(&mut sim, json!({"nomads":30,"caravan":0,"remnant":-10}));
    sim.step(&mut rt).unwrap(); // tick 0: settle (start event + faction-tick derives tier_nomads=neutral)

    sim.inject_reply("ui-activate", json!({ "action": "negotiate-nomads" }));
    sim.step(&mut rt).unwrap(); // tick 1: negotiate fn runs, stashes target="nomads", emits llm-ask

    // Capture the llm-ask event's id (engine embeds callback name as id prefix).
    let observed = rt.drain_observed();
    let llm_ask = observed.iter().find(|e| e.name == "llm-ask").expect("llm-ask event emitted");
    let ask_id = llm_ask.data.get("id").and_then(|v| v.as_str()).expect("llm-ask has id");

    // Inject LLM reply with the captured id + fallback text -> triggers onNegotiateReply.
    sim.inject_reply("llm-reply", json!({ "id": ask_id, "text": "（旅人沉默片刻,点了点头）" }));
    sim.step(&mut rt).unwrap(); // tick 2: onNegotiateReply runs, detects fallback, substitutes canned line, +3 relation

    let rel_val = get_field(&sim, "colony", "Faction.relations");
    let rel = rel_val.as_str().unwrap();
    assert!(rel.contains("\"nomads\":33"), "nomads relation should be 30+3=33, got {}", rel);

    // Stash should be cleared.
    let stash_val = get_field(&sim, "colony", "Colony._negotiate_target");
    let stash = stash_val.as_str().unwrap();
    assert_eq!(stash, "", "negotiate target stash should be cleared after reply");
}
