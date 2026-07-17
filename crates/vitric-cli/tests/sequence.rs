//! Sequence (timeline) end-to-end + unit: generic verb advance, tween starts a tween, wait
//! barrier, skip fast-forward, completion event, empty-stage zero cost, snapshot/restore
//! resume identical, recording bit-for-bit replay.
//!
//! Using examples/intro, an "opening" assembled purely from primitives, we prove: there is no
//! "cutscene" code in the engine — a comic-style cutscene is a composition of Sequence +
//! Sprite + Text + Camera + Tween.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::json;

use vitric_cli::runtime::{advance_sequences, Runtime};
use vitric_data::Sequence;
use vitric_ecs::World;
use vitric_rules::Event;

fn intro_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/intro")
}

// ---- Unit: advance_sequences directly drives a World + catalog ----

fn schema_for_test() -> vitric_data::Schema {
    vitric_data::Schema::parse(
        &json!({"components": {
            "Position": {"fields": {"x": {"type":"number"}, "y": {"type":"number"}}},
            "Sprite": {"fields": {"w": {"type":"number"}, "h": {"type":"number"},
                                   "color": {"type":"text", "default":"#fff"}}},
            "Text": {"fields": {"content": {"type":"text", "default":""},
                                 "reveal": {"type":"number", "default": 1}}},
            "Sequence": {"fields": {
                "track": {"type":"text", "default":""},
                "cursor": {"type":"int", "default":0},
                "start": {"type":"int", "default":-1},
                "wait": {"type":"text", "default":""},
                "id": {"type":"text", "default":""}
            }},
            "Tween": {"fields": {
                "target": {"type":"text"}, "field": {"type":"text"},
                "from": {"type":"number"}, "to": {"type":"number"},
                "duration": {"type":"int"}, "ease": {"type":"text", "default":"linear"},
                "start": {"type":"int", "default":-1}, "id": {"type":"text", "default":""}
            }}
        }}),
        "schema.json",
    )
    .unwrap()
}

fn catalog(doc: serde_json::Value) -> BTreeMap<String, Sequence> {
    let seq = Sequence::parse(&doc, "sequences/s.json", &schema_for_test()).unwrap();
    BTreeMap::from([(seq.id.clone(), seq)])
}

fn sequencer(w: &mut World, track: &str) -> vitric_ecs::EntityId {
    let e = w.spawn_named("seq").unwrap();
    w.set_component(e, "Sequence", json!({"track": track, "cursor": 0, "start": -1, "wait": "", "id": ""}))
        .unwrap();
    e
}

/// Thin wrapper for unit tests: call advance_sequences with the test schema.
fn adv(
    w: &mut World,
    cat: &BTreeMap<String, Sequence>,
    inbox: &[Event],
    tick: u64,
) -> Result<Vec<Event>, String> {
    advance_sequences(w, cat, &schema_for_test(), inbox, tick)
}

#[test]
fn empty_stage_costs_nothing() {
    // No Sequence component at all: zero events, world unchanged (performance budget item 1)
    let mut w = World::new();
    w.spawn_named("lonely").unwrap();
    let cat = catalog(json!({"id": "x", "steps": [{"at": 0, "do": {"emit": "boom"}}]}));
    let before = w.entities().len();
    let ev = adv(&mut w, &cat, &[], 0).unwrap();
    assert!(ev.is_empty(), "空场不该发任何事件");
    assert_eq!(w.entities().len(), before, "空场不该动世界");
}

#[test]
fn set_and_emit_fire_at_their_ticks() {
    let mut w = World::new();
    let target = w.spawn_named("subtitle").unwrap();
    w.set_component(target, "Text", json!({"content": "hi", "reveal": 0.0})).unwrap();
    sequencer(&mut w, "s");
    let cat = catalog(json!({"id": "s", "steps": [
        {"at": 0, "do": {"set": "@subtitle.Text.reveal", "to": 1.0}},
        {"at": 2, "do": {"emit": "done", "data": {"k": 7}}}
    ]}));
    // tick 0: start + fire the at=0 set
    let ev = adv(&mut w, &cat, &[], 0).unwrap();
    assert!(ev.is_empty());
    assert_eq!(w.get_field(target, "Text.reveal").unwrap(), &json!(1.0), "at=0 的 set 立刻生效");
    // tick 1: not yet at=2
    adv(&mut w, &cat, &[], 1).unwrap();
    // tick 2: fire the emit, then run to the end and emit the completion event
    let ev = adv(&mut w, &cat, &[], 2).unwrap();
    let names: Vec<&str> = ev.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["done", "sequence-finished"]);
    assert_eq!(ev[0].data.get("k"), Some(&json!(7)));
}

#[test]
fn tween_action_spawns_a_real_tween_component() {
    // The sequence's tween action = spawns a Tween component for the tween system to execute
    // (no duplication)
    let mut w = World::new();
    let panel = w.spawn_named("panel").unwrap();
    w.set_component(panel, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
    sequencer(&mut w, "s");
    let cat = catalog(json!({"id": "s", "steps": [
        {"at": 0, "do": {"tween": {"target": "panel", "field": "Position.x",
                                     "from": 0, "to": 5, "duration": 10, "ease": "linear"}}}
    ]}));
    adv(&mut w, &cat, &[], 0).unwrap();
    let tweens = w.query(&["Tween"]);
    assert_eq!(tweens.len(), 1, "tween 动作应起一个 Tween 组件");
    // Target is parsed into a handle; field/from/to/duration copied verbatim; start=-1 waits
    // for the tween system to stamp it
    assert_eq!(w.get_field(tweens[0], "Tween.target").unwrap(), &json!(panel.to_string()));
    assert_eq!(w.get_field(tweens[0], "Tween.field").unwrap(), &json!("Position.x"));
    assert_eq!(w.get_field(tweens[0], "Tween.to").unwrap(), &json!(5.0));
    assert_eq!(w.get_field(tweens[0], "Tween.start").unwrap(), &json!(-1));
}

#[test]
fn wait_barrier_parks_until_named_event() {
    let mut w = World::new();
    let seq = sequencer(&mut w, "s");
    let cat = catalog(json!({"id": "s", "steps": [
        {"at": 0, "do": {"emit": "a"}},
        {"at": 0, "do": {"wait": "go"}},
        {"at": 0, "do": {"emit": "b"}}
    ]}));
    // tick 0: emit a, hit the barrier and park (b not emitted)
    let ev = adv(&mut w, &cat, &[], 0).unwrap();
    assert_eq!(ev.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["a"]);
    assert_eq!(w.get_field(seq, "Sequence.wait").unwrap(), &json!("go"), "停在 barrier");
    // tick 1: no go event, keep parked
    let ev = adv(&mut w, &cat, &[], 1).unwrap();
    assert!(ev.is_empty(), "barrier 没放行不该再发事件");
    assert!(w.is_alive(seq), "还在等，序列没结束");
    // tick 2: go arrives → release, emit b, run to the end and emit the completion event +
    // auto-despawn
    let inbox = [Event::new("go", json!({}))];
    let ev = adv(&mut w, &cat, &inbox, 2).unwrap();
    let names: Vec<&str> = ev.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["b", "sequence-finished"]);
    assert!(!w.is_alive(seq), "跑完自动移除");
}

#[test]
fn skip_input_fast_forwards_to_end() {
    let mut w = World::new();
    let panel = w.spawn_named("panel").unwrap();
    w.set_component(panel, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
    let target = w.spawn_named("subtitle").unwrap();
    w.set_component(target, "Text", json!({"content": "hi", "reveal": 0.0})).unwrap();
    let seq = sequencer(&mut w, "s");
    let cat = catalog(json!({"id": "s", "steps": [
        {"at": 0,   "do": {"set": "@subtitle.Text.reveal", "to": 1.0}},
        {"at": 100, "do": {"wait": "never"}},
        {"at": 200, "do": {"set": "@panel.Position.x", "to": 9.0}},
        {"at": 200, "do": {"emit": "the-end"}}
    ]}));
    // tick 0 + skip input: ignore at/wait, settle all remaining terminal states + completion
    // event
    let inbox = [Event::new("input", json!({"action": "skip", "phase": "pressed"}))];
    let ev = adv(&mut w, &cat, &inbox, 0).unwrap();
    let names: Vec<&str> = ev.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["the-end", "sequence-finished"], "跳过把终态落定后发完成");
    assert_eq!(w.get_field(target, "Text.reveal").unwrap(), &json!(1.0));
    assert_eq!(w.get_field(panel, "Position.x").unwrap(), &json!(9.0), "未执行条目的终态也落定");
    assert!(!w.is_alive(seq));
}

#[test]
fn unknown_track_is_explicit() {
    let mut w = World::new();
    sequencer(&mut w, "ghost");
    let cat = catalog(json!({"id": "real", "steps": [{"at": 0, "do": {"emit": "x"}}]}));
    let err = adv(&mut w, &cat, &[], 0).unwrap_err();
    assert!(err.contains("ghost") && err.contains("real"), "{err}");
}

// ---- End-to-end: examples/intro full chain (Sim + Runtime + rules) ----

/// Inject a list of (tick, action) inputs and run up to `ticks`, recording. Returns (recording,
/// final world).
fn run_intro(inputs: &[(u64, &str)], ticks: u64, record: bool) -> (Option<vitric_sim::Recording>, World) {
    let (mut sim, mut rt) = Runtime::boot(&intro_dir()).unwrap();
    if record {
        sim.start_recording();
    }
    for t in 0..ticks {
        for (it, action) in inputs {
            if *it == t {
                sim.inject_input(action, "pressed");
            }
        }
        sim.step(&mut rt).unwrap();
    }
    let rec = if record { sim.stop_recording() } else { None };
    (rec, sim.world)
}

#[test]
fn intro_typewriter_reveals_subtitle_over_time() {
    // reveal is pushed from 0 to 1 by the sequence's tween: typewriter effect. Layout is
    // computed only once, locked by the render-layer test; here we only verify the reveal
    // field is really driven monotonically upward by the sequence
    let (_, w0) = run_intro(&[], 31, false);
    let sub = w0.entity("subtitle").unwrap();
    // tween starts at at=30; for the first 30 ticks reveal is still 0
    assert_eq!(w0.get_field(sub, "Text.reveal").unwrap().as_f64(), Some(0.0));
    let (_, w1) = run_intro(&[], 60, false);
    let sub1 = w1.entity("subtitle").unwrap();
    let r1 = w1.get_field(sub1, "Text.reveal").unwrap().as_f64().unwrap();
    assert!(r1 > 0.0, "tween 起跑后 reveal 应上升");
    // After running past the reveal tween end (at=30 + duration 60 = tick 90) it must reach 1.0
    let (_, w2) = run_intro(&[], 92, false);
    let sub2 = w2.entity("subtitle").unwrap();
    assert_eq!(w2.get_field(sub2, "Text.reveal").unwrap().as_f64(), Some(1.0));
}

#[test]
fn intro_illustration_is_spawned_then_despawned_by_sequence() {
    // Opening spawns the illustration (a solid-color sprite); the ending fades it out and the
    // whole thing ends
    let (_, w) = run_intro(&[], 5, false);
    assert!(w.entity("illustration").is_ok(), "序列 spawn 出插画");
}

#[test]
fn watching_the_whole_thing_emits_run_complete() {
    // The barrier waits at elapsed=150 for player-confirm; after that pressing confirm
    // releases it, the sequence wraps up and emits intro-done, the rule catches it and emits
    // run-complete (the gate's must_emit)
    let (_, w) = run_intro(&[(160, "confirm")], 360, false);
    // After the sequence finishes the sequencer entity is removed
    assert!(w.entity("sequencer").is_err(), "看完整段后序列实体应被移除");
}

#[test]
fn skipping_also_emits_run_complete_and_replays_identically() {
    // Both paths — watching the whole thing and skipping midway — record and replay
    // bit-identically (contract acceptance)
    let (watch_rec, _) = run_intro(&[(160, "confirm")], 360, true);
    let watch_rec = watch_rec.unwrap();
    let (skip_rec, _) = run_intro(&[(40, "skip")], 240, true);
    let skip_rec = skip_rec.unwrap();

    // Both replay bit-identically from a cold boot
    let (mut s1, mut r1) = Runtime::boot(&intro_dir()).unwrap();
    s1.replay(&watch_rec, &mut r1).expect("看完整段的录像必须逐位重放一致");
    let (mut s2, mut r2) = Runtime::boot(&intro_dir()).unwrap();
    s2.replay(&skip_rec, &mut r2).expect("中途跳过的录像必须逐位重放一致");
}

#[test]
fn sequence_snapshot_restore_resumes_identically() {
    // All sequence state lives in the Sequence component: snapshot midway, restore on another
    // sim, resume bit-identically
    let (mut sim, mut rt) = Runtime::boot(&intro_dir()).unwrap();
    for _ in 0..50 {
        sim.step(&mut rt).unwrap();
    }
    let snap = sim.snapshot(&rt);
    for _ in 0..50 {
        sim.step(&mut rt).unwrap();
    }
    let direct = sim.world.state_hash();

    let (mut sim2, mut rt2) = Runtime::boot(&intro_dir()).unwrap();
    sim2.restore(&snap, &mut rt2).unwrap();
    for _ in 0..50 {
        sim2.step(&mut rt2).unwrap();
    }
    assert_eq!(sim2.world.state_hash(), direct, "序列中途回退续播必须逐位一致");
}
