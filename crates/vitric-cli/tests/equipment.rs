//! End-to-end test for the equipment module: boot the equipment-demo, test
//! equip/unequip mechanics, auto-unequip on slot swap, stat bonus application
//! via equipped/unequipped events, and the full combat → equipment composition.
//!
//! Tests the three-module composition: equipment (equip/unequip) ↔ inventory
//! (item moved between inventory and slots) ↔ combat (stat bonuses from
//! equipped items affect attack power). The equipment module reads/writes
//! Inventory directly for atomic transactions (same pattern as shop).

use std::path::PathBuf;

use vitric_cli::runtime::Runtime;

fn demo_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/equipment-demo")
}

fn player_hp(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Health.hp").unwrap().as_i64().unwrap()
}

fn player_max_hp(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Health.max").unwrap().as_i64().unwrap()
}

fn player_attack(sim: &vitric_sim::Sim) -> i64 {
    let p = sim.world.entity("player").unwrap();
    sim.world.get_field(p, "Attack.power").unwrap().as_i64().unwrap()
}

fn inv_count(sim: &vitric_sim::Sim, item: &str) -> i64 {
    let p = sim.world.entity("player").unwrap();
    let items = sim.world.get_field(p, "Inventory.items").unwrap().clone();
    let counts = sim.world.get_field(p, "Inventory.counts").unwrap().clone();
    let arr = items.as_array().unwrap();
    let cnt = counts.as_array().unwrap();
    let mut total = 0;
    for (i, it) in arr.iter().enumerate() {
        if it.as_str().unwrap() == item {
            total += cnt[i].as_i64().unwrap_or(0);
        }
    }
    total
}

/// Read the equipped item id at a named slot ("" = empty).
fn equipped_at(sim: &vitric_sim::Sim, slot: &str) -> String {
    let p = sim.world.entity("player").unwrap();
    let slots = sim.world.get_field(p, "Equipment.slots").unwrap().clone();
    let items = sim.world.get_field(p, "Equipment.items").unwrap().clone();
    let s = slots.as_array().unwrap();
    let it = items.as_array().unwrap();
    for (i, name) in s.iter().enumerate() {
        if name.as_str().unwrap() == slot {
            return it.get(i).map(|v| v.as_str().unwrap().to_string()).unwrap_or_default();
        }
    }
    String::new()
}

/// Press a key once and step enough ticks for the event cascade to land:
/// input → emit equip/unequip → __equip/__unequip → equipped/unequipped → bonus.
fn press(sim: &mut vitric_sim::Sim, rt: &mut Runtime, key: &str) {
    sim.inject_input(key, "pressed");
    for _ in 0..5 {
        sim.step(rt).unwrap();
    }
}

#[test]
fn equipment_demo_check_passes() {
    let (_sim, _rt) =
        Runtime::boot(&demo_dir()).expect("equipment-demo must pass vitric check and boot");
}

#[test]
fn initial_state_has_empty_slots_and_full_inventory() {
    let (sim, _rt) = Runtime::boot(&demo_dir()).unwrap();

    // Player starts at base stats: 100/100 HP, 10 ATK.
    assert_eq!(player_hp(&sim), 100);
    assert_eq!(player_max_hp(&sim), 100);
    assert_eq!(player_attack(&sim), 10, "base attack should be 10");

    // All 3 equipment slots start empty.
    assert_eq!(equipped_at(&sim, "weapon"), "", "weapon slot should start empty");
    assert_eq!(equipped_at(&sim, "armor"), "", "armor slot should start empty");
    assert_eq!(equipped_at(&sim, "accessory"), "", "accessory slot should start empty");

    // Inventory has all 5 items, 1 each.
    assert_eq!(inv_count(&sim, "sword"), 1);
    assert_eq!(inv_count(&sim, "armor"), 1);
    assert_eq!(inv_count(&sim, "ring"), 1);
    assert_eq!(inv_count(&sim, "gloves"), 1);
    assert_eq!(inv_count(&sim, "spare_sword"), 1);
}

#[test]
fn equip_sword_moves_item_and_applies_atk_bonus() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Press 1: equip sword to weapon slot.
    press(&mut sim, &mut rt, "1");

    // Sword removed from inventory, equipped to weapon slot.
    assert_eq!(inv_count(&sim, "sword"), 0, "sword should be removed from inventory");
    assert_eq!(equipped_at(&sim, "weapon"), "sword", "sword should be in weapon slot");

    // ATK bonus applied: 10 base + 10 = 20.
    assert_eq!(player_attack(&sim), 20, "sword should give +10 ATK (10+10=20)");
}

#[test]
fn equip_armor_applies_max_hp_bonus_and_heals() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Press 2: equip armor to armor slot.
    press(&mut sim, &mut rt, "2");

    // Armor removed from inventory, equipped to armor slot.
    assert_eq!(inv_count(&sim, "armor"), 0, "armor should be removed from inventory");
    assert_eq!(equipped_at(&sim, "armor"), "armor", "armor should be in armor slot");

    // Max HP bonus applied: 100 + 20 = 120. HP healed to new max.
    assert_eq!(player_max_hp(&sim), 120, "armor should give +20 max HP (100+20=120)");
    assert_eq!(player_hp(&sim), 120, "HP should be full-healed to new max");
}

#[test]
fn equip_ring_to_accessory_applies_atk_bonus() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Press 3: equip ring to accessory slot.
    press(&mut sim, &mut rt, "3");

    assert_eq!(inv_count(&sim, "ring"), 0, "ring should be removed from inventory");
    assert_eq!(equipped_at(&sim, "accessory"), "ring", "ring should be in accessory slot");
    assert_eq!(player_attack(&sim), 15, "ring should give +5 ATK (10+5=15)");
}

#[test]
fn unequip_weapon_returns_item_and_removes_bonus() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Equip sword first.
    press(&mut sim, &mut rt, "1");
    assert_eq!(player_attack(&sim), 20);

    // Press Q: unequip weapon.
    press(&mut sim, &mut rt, "q");

    // Sword back in inventory, weapon slot empty, ATK back to base.
    assert_eq!(inv_count(&sim, "sword"), 1, "sword should be back in inventory");
    assert_eq!(equipped_at(&sim, "weapon"), "", "weapon slot should be empty");
    assert_eq!(player_attack(&sim), 10, "ATK should return to base 10 after unequip");
}

#[test]
fn unequip_empty_slot_is_noop() {
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Press Q: unequip weapon (which is already empty).
    press(&mut sim, &mut rt, "q");

    // Nothing changes — no crash, no item created.
    assert_eq!(equipped_at(&sim, "weapon"), "", "empty slot stays empty");
    assert_eq!(player_attack(&sim), 10, "ATK unchanged");
}

#[test]
fn auto_unequip_on_slot_swap_returns_old_item() {
    // Equip ring to accessory, then equip gloves to the same slot.
    // The ring should auto-unequip back to inventory, gloves take the slot.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Equip ring: +5 ATK → 15.
    press(&mut sim, &mut rt, "3");
    assert_eq!(equipped_at(&sim, "accessory"), "ring");
    assert_eq!(inv_count(&sim, "ring"), 0);
    assert_eq!(inv_count(&sim, "gloves"), 1);
    assert_eq!(player_attack(&sim), 15);

    // Equip gloves to accessory (occupied by ring) → auto-unequip ring.
    // ATK: 15 - 5 (ring removed) + 3 (gloves added) = 13.
    press(&mut sim, &mut rt, "4");
    assert_eq!(equipped_at(&sim, "accessory"), "gloves", "gloves should be in accessory slot");
    assert_eq!(inv_count(&sim, "gloves"), 0, "gloves removed from inventory");
    assert_eq!(inv_count(&sim, "ring"), 1, "ring should be back in inventory");
    assert_eq!(player_attack(&sim), 13, "ATK should be 10+3=13 after ring→gloves swap");
}

#[test]
fn auto_unequip_weapon_swap_returns_old_sword() {
    // Equip sword, then equip spare_sword to the same weapon slot.
    // The sword should auto-unequip back to inventory.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Equip sword: +10 ATK → 20.
    press(&mut sim, &mut rt, "1");
    assert_eq!(equipped_at(&sim, "weapon"), "sword");
    assert_eq!(inv_count(&sim, "sword"), 0);
    assert_eq!(inv_count(&sim, "spare_sword"), 1);
    assert_eq!(player_attack(&sim), 20);

    // Equip spare_sword to weapon (occupied by sword) → auto-unequip sword.
    // ATK: 20 - 10 (sword removed) + 8 (spare_sword added) = 18.
    press(&mut sim, &mut rt, "5");
    assert_eq!(equipped_at(&sim, "weapon"), "spare_sword", "spare_sword should be in weapon slot");
    assert_eq!(inv_count(&sim, "spare_sword"), 0, "spare_sword removed from inventory");
    assert_eq!(inv_count(&sim, "sword"), 1, "sword should be back in inventory");
    assert_eq!(player_attack(&sim), 18, "ATK should be 10+8=18 after sword→spare_sword swap");
}

#[test]
fn full_kit_sword_armor_ring_stacks_bonuses() {
    // Equip all three: sword (weapon), armor (armor), ring (accessory).
    // Bonuses should stack: +10 ATK, +20 max HP, +5 ATK.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    press(&mut sim, &mut rt, "1"); // sword
    press(&mut sim, &mut rt, "2"); // armor
    press(&mut sim, &mut rt, "3"); // ring

    // ATK: 10 + 10 (sword) + 5 (ring) = 25.
    assert_eq!(player_attack(&sim), 25, "ATK should stack: 10+10+5=25");

    // HP: 100 + 20 (armor) = 120.
    assert_eq!(player_max_hp(&sim), 120, "max HP should be 100+20=120");
    assert_eq!(player_hp(&sim), 120, "HP should be full-healed to 120");

    // All slots occupied, all 3 items removed from inventory.
    assert_eq!(equipped_at(&sim, "weapon"), "sword");
    assert_eq!(equipped_at(&sim, "armor"), "armor");
    assert_eq!(equipped_at(&sim, "accessory"), "ring");
    assert_eq!(inv_count(&sim, "sword"), 0);
    assert_eq!(inv_count(&sim, "armor"), 0);
    assert_eq!(inv_count(&sim, "ring"), 0);
}

#[test]
fn equipped_weapon_boosts_attack_damage() {
    // Equip sword (+10 ATK), then attack the dummy. The damage dealt should
    // reflect the equipped weapon's bonus, proving equipment → combat composition.
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Base attack (no weapon): 10 damage.
    let dummy = sim.world.entity("dummy").unwrap();
    let dummy_hp_before = sim.world.get_field(dummy, "Health.hp").unwrap().as_i64().unwrap();
    press(&mut sim, &mut rt, "x");
    let dummy_hp_after = sim.world.get_field(dummy, "Health.hp").unwrap().as_i64().unwrap();
    let base_damage = dummy_hp_before - dummy_hp_after;
    assert_eq!(base_damage, 10, "base attack should deal 10 damage");

    // Equip sword (+10 ATK → 20 total), attack again: 20 damage.
    press(&mut sim, &mut rt, "1");
    assert_eq!(player_attack(&sim), 20);
    let dummy_hp_before = sim.world.get_field(dummy, "Health.hp").unwrap().as_i64().unwrap();
    press(&mut sim, &mut rt, "x");
    let dummy_hp_after = sim.world.get_field(dummy, "Health.hp").unwrap().as_i64().unwrap();
    let armed_damage = dummy_hp_before - dummy_hp_after;
    assert_eq!(armed_damage, 20, "equipped sword should deal 20 damage (10 base + 10 bonus)");
}

#[test]
fn unequip_armor_clamps_hp_to_new_max() {
    // Equip armor (+20 max HP, full heal to 120), then unequip it.
    // Max HP returns to 100, current HP should clamp to 100 (not stay at 120).
    let (mut sim, mut rt) = Runtime::boot(&demo_dir()).unwrap();

    // Equip armor: max HP 100→120, HP 100→120.
    press(&mut sim, &mut rt, "2");
    assert_eq!(player_max_hp(&sim), 120);
    assert_eq!(player_hp(&sim), 120);

    // Unequip armor: max HP 120→100, HP clamps 120→100.
    press(&mut sim, &mut rt, "w");
    assert_eq!(player_max_hp(&sim), 100, "max HP should return to 100");
    assert_eq!(player_hp(&sim), 100, "HP should clamp to new max 100");
    assert_eq!(inv_count(&sim, "armor"), 1, "armor back in inventory");
}
