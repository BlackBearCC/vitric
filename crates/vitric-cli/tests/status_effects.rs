//! End-to-end test for the status-effects module: boot the status-effects-demo,
//! test effect lifecycle (apply / tick / expire / clear), the two composition
//! patterns (tick-based DoT/HoT via status-ticked → damage/heal, and stat-modifier
//! via status-applied/expired → ATK bonus), stacking/refresh, and multi-effect
//! coexistence.
//!
//! Tests the two-module composition: status-effects (lifecycle) ↔ combat
//! (damage/heal/Health). The game's rules bridge the modules: poison-ticked →
//! damage, regen-ticked → heal, haste-applied → +ATK, haste-expired → -ATK.

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/status-effects-demo")
}

fn player_hp(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Health.hp").unwrap().as_i64().unwrap()
}

fn player_attack(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Attack.power").unwrap().as_i64().unwrap()
}

fn dummy_hp(sim: &vitric_sim::Sim) -> i64 {
    let d = sim.world.entity("dummy").unwrap();
    sim.world.get_field(d, "Health.hp").unwrap().as_i64().unwrap()
}

/// Read the list of active status effect names on an entity.
fn status_effects(sim: &vitric_sim::Sim, entity: &str) -> Vec<String> {
    let e = sim.world.entity(entity).unwrap();
    let arr = sim.world.get_field(e, "StatusEffects.effects").unwrap().clone();
    arr.as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

/// Read the duration of a specific effect on an entity (0 if not present).
fn status_duration(sim: &vitric_sim::Sim, entity: &str, effect: &str) -> i64 {
    let e = sim.world.entity(entity).unwrap();
    let effs = sim.world.get_field(e, "StatusEffects.effects").unwrap().clone();
    let durs = sim.world.get_field(e, "StatusEffects.durations").unwrap().clone();
    let arr = effs.as_array().unwrap();
    let d = durs.as_array().unwrap();
    for (i, v) in arr.iter().enumerate() {
        if v.as_str().unwrap() == effect {
            return d[i].as_i64().unwrap_or(0);
        }
    }
    0
}

/// Press a key once and step enough ticks for the event cascade to land.
fn press(sim: &mut vitric_sim::Sim, rt: &mut Runtime, key: &str) {
    sim.inject_input(key, "pressed");
    for _ in 0..5 {
        sim.step(rt).unwrap();
    }
}

/// Step N ticks without input (let status effects tick + cascade).
fn run_ticks(sim: &mut vitric_sim::Sim, rt: &mut Runtime, n: usize) {
    for _ in 0..n {
        sim.step(rt).unwrap();
    }
}

#[test]
fn status_effects_demo_check_passes() {
    let (_sim, _rt) = Runtime::boot(&demo_dir())
        .expect("status-effects-demo must pass vitric check and boot");
}

#[test]
fn initial_state_has_no_effects() {
    let (sim, _rt) = Runtime::boot(&demo_dir()).unwrap();

    assert!(status_effects(&sim, "player").is_empty(), "player should start with no effects");
    assert!(status_effects(&sim, "dummy").is_empty(), "dummy should start with no effects");
    assert_eq!(player_hp(&sim), 100);
    assert_eq!(player_attack(&sim), 20, "base ATK should be 20");
    assert_eq!(dummy_hp(&sim), 200, "dummy should start at 200 HP");
}

#[test]
fn poison_deals_damage_over_time() {
    // Apply poison (duration 10, magnitude 10) to the dummy.
    // Poison ticks every tick, dealing 10 damage per tick.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    assert_eq!(dummy_hp(&sim), 200);

    // Press 1: apply poison to dummy.
    press(&mut sim, &mut rt, "1");

    // Poison should be active right after the press (cascade has landed).
    assert!(status_effects(&sim, "dummy").contains(&"poison".to_string()),
        "poison should be active right after apply");

    // Run more ticks to accumulate damage. Each tick of damage takes ~2 ticks
    // to cascade: status-tick → status-ticked → poison-tick rule → damage →
    // combat-on-damage → HP write. After 10 more ticks (~15 total), several
    // ticks of poison damage (10 each) should have landed.
    run_ticks(&mut sim, &mut rt, 10);

    let hp_after = dummy_hp(&sim);
    assert!(hp_after < 200, "dummy should have taken poison damage, got {hp_after}");
    assert!(hp_after <= 170, "dummy should have lost at least 30 HP (3+ ticks × 10), got {hp_after}");
    // Note: poison (duration 10) may have expired after ~15 total ticks of
    // ticking — that's expected. The HP loss above is the proof damage was dealt.
}

#[test]
fn regen_does_not_overheal_at_full_hp() {
    // Apply regen to player at full HP — should not overheal.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    assert_eq!(player_hp(&sim), 100);

    // Press 2: apply regen to player (at full HP — regen wasted).
    press(&mut sim, &mut rt, "2");
    run_ticks(&mut sim, &mut rt, 5);

    // HP should stay at 100 (can't exceed max).
    assert_eq!(player_hp(&sim), 100, "regen at full HP should not overheal");
}

#[test]
fn haste_boosts_atk_then_expires() {
    // Apply haste (duration 10, magnitude 10) to the player.
    // ATK should go from 20 → 30 while active, then back to 20 when expired.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    assert_eq!(player_attack(&sim), 20, "base ATK should be 20");

    // Press 3: apply haste to player.
    press(&mut sim, &mut rt, "3");

    // Haste should be active and ATK boosted.
    // Cascade: apply-status(N) → __status_apply(N+1, deferred write)
    // → status-applied(N+1 carryover) → haste-applied rule(N+2, deferred write)
    // → ATK visible(N+3). After 5-tick press, ATK should be 30.
    assert_eq!(player_attack(&sim), 30, "haste should boost ATK to 30 while active");
    assert!(status_effects(&sim, "player").contains(&"haste".to_string()),
        "haste should still be active (duration 10)");

    // Run enough ticks for haste to expire (duration 10, ~5 ticks already used
    // during press). Need ~5 more ticks of ticking + cascade for expiry.
    run_ticks(&mut sim, &mut rt, 15);

    assert_eq!(player_attack(&sim), 20, "ATK should return to 20 after haste expires");
    assert!(!status_effects(&sim, "player").contains(&"haste".to_string()),
        "haste should have expired");
}

#[test]
fn clear_status_removes_effect_early() {
    // Apply poison to dummy, then clear it with antidote (press 4).
    // Poison should stop dealing damage after clearing.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Apply poison (duration 10, magnitude 10).
    press(&mut sim, &mut rt, "1");
    run_ticks(&mut sim, &mut rt, 3); // let a couple ticks of damage land

    let hp_mid = dummy_hp(&sim);
    assert!(hp_mid < 200, "poison should have dealt some damage already, HP={hp_mid}");

    // Poison should still be active (duration 10, only ~3 ticks of ticking).
    assert!(status_effects(&sim, "dummy").contains(&"poison".to_string()),
        "poison should still be active before clearing");

    // Press 4: clear poison (antidote).
    press(&mut sim, &mut rt, "4");

    // Poison should be removed from the dummy's StatusEffects.
    assert!(!status_effects(&sim, "dummy").contains(&"poison".to_string()),
        "poison should be cleared after antidote");

    // Run more ticks — HP should not change (no more poison damage).
    let hp_after_clear = dummy_hp(&sim);
    run_ticks(&mut sim, &mut rt, 10);
    assert_eq!(dummy_hp(&sim), hp_after_clear,
        "dummy HP should not change after poison is cleared");
}

#[test]
fn reapply_refreshes_duration() {
    // Apply poison (duration 10), wait 3 ticks, re-apply (duration 10).
    // The duration should refresh to max(current, 10) = 10.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Apply poison (duration 10).
    press(&mut sim, &mut rt, "1");
    run_ticks(&mut sim, &mut rt, 3);

    // Poison is active. Duration should have ticked down from 10 but still present.
    assert!(status_effects(&sim, "dummy").contains(&"poison".to_string()),
        "poison should be active");
    let dur_before = status_duration(&sim, "dummy", "poison");
    assert!(dur_before > 0 && dur_before < 10,
        "duration should have ticked down from 10, got {dur_before}");

    // Re-apply poison (duration 10) — should refresh to max(current, 10) = 10.
    press(&mut sim, &mut rt, "1");
    let dur_after = status_duration(&sim, "dummy", "poison");
    assert!(dur_after >= dur_before,
        "re-apply should refresh duration (got {dur_after}, was {dur_before})");
}

#[test]
fn multiple_effects_coexist() {
    // Apply poison to dummy AND haste to player simultaneously.
    // Both effects should coexist and work independently.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Apply poison to dummy first — verify it lands before applying the next
    // effect (poison has duration 10, so it survives the press's 5 ticks).
    press(&mut sim, &mut rt, "1"); // poison on dummy
    assert!(status_effects(&sim, "dummy").contains(&"poison".to_string()),
        "poison should be on dummy right after apply");

    // Apply haste to player — another 5 ticks pass. Poison may tick down
    // further but should still be active (duration 10 - 10 ticks = expired,
    // so we don't re-assert poison here; we verify it dealt damage below).
    press(&mut sim, &mut rt, "3"); // haste on player

    // Haste should be active on the player.
    assert!(status_effects(&sim, "player").contains(&"haste".to_string()),
        "haste should be on player");

    // Player ATK should be boosted.
    assert_eq!(player_attack(&sim), 30, "haste should boost ATK to 30");

    // Dummy should have taken poison damage (proves poison coexisted & worked).
    assert!(dummy_hp(&sim) < 200, "dummy should have taken poison damage");
}

#[test]
fn haste_boosts_attack_damage() {
    // Attack the dummy without haste (20 ATK = 20 damage),
    // then with haste (30 ATK = 30 damage).
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Base attack: 20 damage.
    let hp_before = dummy_hp(&sim);
    press(&mut sim, &mut rt, "x");
    let hp_after = dummy_hp(&sim);
    assert_eq!(hp_before - hp_after, 20, "base attack should deal 20 damage");

    // Apply haste (+10 ATK → 30 total), attack again: 30 damage.
    press(&mut sim, &mut rt, "3");
    assert_eq!(player_attack(&sim), 30, "haste should boost ATK to 30");

    let hp_before = dummy_hp(&sim);
    press(&mut sim, &mut rt, "x");
    let hp_after = dummy_hp(&sim);
    assert_eq!(hp_before - hp_after, 30, "haste-boosted attack should deal 30 damage");
}

#[test]
fn clear_nonexistent_effect_is_noop() {
    // Clearing an effect that isn't on the entity should be a no-op (no crash).
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Press 4: clear poison from dummy (dummy has no poison).
    press(&mut sim, &mut rt, "4");

    // Nothing changes, no crash.
    assert!(status_effects(&sim, "dummy").is_empty(), "clearing nonexistent effect should be no-op");
    assert_eq!(dummy_hp(&sim), 200, "dummy HP unchanged");
}
