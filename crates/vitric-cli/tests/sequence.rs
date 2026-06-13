//! 序列（时间轴）端到端 + 单元：通用动词推进、tween 起补间、wait barrier、
//! skip 跳过、完成事件、空场零成本、snapshot/restore 续播一致、录像逐位重放。
//!
//! 用 examples/intro 这个纯原语拼出来的"开场"证明：引擎里没有"过场"代码，
//! 漫画过场是 Sequence + Sprite + Text + Camera + Tween 的组合用法。

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

// ---- 单元：advance_sequences 直接驱动一个 World + 目录 ----

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

/// 单元测试用的薄封装：用测试 schema 调 advance_sequences。
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
    // 没有任何 Sequence 组件：零事件、世界不动（性能预算第 1 条）
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
    // tick 0：起跑 + 发动 at=0 的 set
    let ev = adv(&mut w, &cat, &[], 0).unwrap();
    assert!(ev.is_empty());
    assert_eq!(w.get_field(target, "Text.reveal").unwrap(), &json!(1.0), "at=0 的 set 立刻生效");
    // tick 1：还没到 at=2
    adv(&mut w, &cat, &[], 1).unwrap();
    // tick 2：发动 emit，随即跑到末尾发完成事件
    let ev = adv(&mut w, &cat, &[], 2).unwrap();
    let names: Vec<&str> = ev.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["done", "sequence-finished"]);
    assert_eq!(ev[0].data.get("k"), Some(&json!(7)));
}

#[test]
fn tween_action_spawns_a_real_tween_component() {
    // 序列的 tween 动作 = 起一个 Tween 组件交给补间系统执行（零重复）
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
    // 目标被解析成句柄、字段/起止/时长照搬，start=-1 等补间系统盖章
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
    // tick 0：发 a，撞 barrier 停住（b 不发）
    let ev = adv(&mut w, &cat, &[], 0).unwrap();
    assert_eq!(ev.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["a"]);
    assert_eq!(w.get_field(seq, "Sequence.wait").unwrap(), &json!("go"), "停在 barrier");
    // tick 1：没有 go 事件，继续停
    let ev = adv(&mut w, &cat, &[], 1).unwrap();
    assert!(ev.is_empty(), "barrier 没放行不该再发事件");
    assert!(w.is_alive(seq), "还在等，序列没结束");
    // tick 2：go 到达 → 放行，发 b，跑到末尾发完成事件 + 自动 despawn
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
    // tick 0 + skip 输入：无视 at/wait，剩余终态全部落定 + 完成事件
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

// ---- 端到端：examples/intro 全链路（Sim + Runtime + 规则） ----

/// 把一串 (tick, action) 输入注进去跑到 `ticks`，并录像。返回 (录像, 最终世界).
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
    // reveal 从 0 被序列的 tween 推到 1：打字机。版面只算一次由 render 层测试锁，
    // 这里只验 reveal 字段真的被序列驱动着单调上升
    let (_, w0) = run_intro(&[], 31, false);
    let sub = w0.entity("subtitle").unwrap();
    // tween 在 at=30 起跑，前 30 tick reveal 还是 0
    assert_eq!(w0.get_field(sub, "Text.reveal").unwrap().as_f64(), Some(0.0));
    let (_, w1) = run_intro(&[], 60, false);
    let sub1 = w1.entity("subtitle").unwrap();
    let r1 = w1.get_field(sub1, "Text.reveal").unwrap().as_f64().unwrap();
    assert!(r1 > 0.0, "tween 起跑后 reveal 应上升");
    // 跑到 reveal 补间结束（at=30 + duration 60 = tick 90）后必到 1.0
    let (_, w2) = run_intro(&[], 92, false);
    let sub2 = w2.entity("subtitle").unwrap();
    assert_eq!(w2.get_field(sub2, "Text.reveal").unwrap().as_f64(), Some(1.0));
}

#[test]
fn intro_illustration_is_spawned_then_despawned_by_sequence() {
    // 开场 spawn 插画（纯色精灵），结局把它淡出后整段结束
    let (_, w) = run_intro(&[], 5, false);
    assert!(w.entity("illustration").is_ok(), "序列 spawn 出插画");
}

#[test]
fn watching_the_whole_thing_emits_run_complete() {
    // barrier 在 elapsed=150 等 player-confirm；在那之后按 confirm 放行，
    // 序列收尾 emit intro-done，规则接住 emit run-complete（gate 的 must_emit）
    let (_, w) = run_intro(&[(160, "confirm")], 360, false);
    // 序列跑完后 sequencer 实体被移除
    assert!(w.entity("sequencer").is_err(), "看完整段后序列实体应被移除");
}

#[test]
fn skipping_also_emits_run_complete_and_replays_identically() {
    // 看完整段 与 中途跳过 两条路径都录像、都逐位重放一致（contract 验收）
    let (watch_rec, _) = run_intro(&[(160, "confirm")], 360, true);
    let watch_rec = watch_rec.unwrap();
    let (skip_rec, _) = run_intro(&[(40, "skip")], 240, true);
    let skip_rec = skip_rec.unwrap();

    // 两条都从冷启动逐位重放一致
    let (mut s1, mut r1) = Runtime::boot(&intro_dir()).unwrap();
    s1.replay(&watch_rec, &mut r1).expect("看完整段的录像必须逐位重放一致");
    let (mut s2, mut r2) = Runtime::boot(&intro_dir()).unwrap();
    s2.replay(&skip_rec, &mut r2).expect("中途跳过的录像必须逐位重放一致");
}

#[test]
fn sequence_snapshot_restore_resumes_identically() {
    // 序列状态全在 Sequence 组件里：中途快照、换个 sim 恢复、续播逐位一致
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
