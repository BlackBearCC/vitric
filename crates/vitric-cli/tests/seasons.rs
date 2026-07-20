//! Task 6 (Seasons & Weather) integration tests.
//!
//! Verifies: (a) season advances on day-wrap boundary; (b) season rolls over at 12 days;
//! (c) weather timer decrements each tick; (d) weather-tick system runs without crashing.

use std::path::PathBuf;

use serde_json::json;
use vitric_cli::runtime::Runtime;

fn frontier_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../games/frontier")
}

#[test]
fn season_advances_on_day_boundary() {
    // Fast test: set Clock.time to just below CLOCK_DAY_SEC (60.0) and Season.day_in_season
    // to 0, then step 1 tick. The day-wrap fires, day_in_season increments to 1.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    let colony_e = sim.world.entity("colony").unwrap();

    // Set time to 59.99 — one tick (dt=1/60≈0.0167) will push it past 60.0, triggering day-wrap.
    let mut clock = sim.world.get_component(colony_e, "Clock").unwrap().clone();
    clock["time"] = json!(59.99);
    clock["day"] = json!(1);
    clock["last_day_emit"] = json!(1); // Suppress day-start emission noise.
    sim.world.set_component(colony_e, "Clock", clock).unwrap();

    let mut season = sim.world.get_component(colony_e, "Season").unwrap().clone();
    season["day_in_season"] = json!(0);
    season["current"] = json!("spring");
    season["year"] = json!(1);
    sim.world.set_component(colony_e, "Season", season).unwrap();

    sim.step(&mut rt).unwrap();

    let season_after = sim.world.get_component(colony_e, "Season").unwrap();
    assert_eq!(season_after["day_in_season"].as_i64(), Some(1),
        "day_in_season should increment to 1 after one day-wrap");
    assert_eq!(season_after["current"].as_str(), Some("spring"),
        "season should still be spring (only 1 day into the season)");
    assert_eq!(season_after["year"].as_i64(), Some(1),
        "year should still be 1");
}

#[test]
fn season_rolls_over_at_12_days() {
    // Set day_in_season to 11 (last day of season), then trigger a day-wrap.
    // The season should roll over from spring to summer, day_in_season reset to 0.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    let colony_e = sim.world.entity("colony").unwrap();

    let mut clock = sim.world.get_component(colony_e, "Clock").unwrap().clone();
    clock["time"] = json!(59.99);
    clock["day"] = json!(12);
    clock["last_day_emit"] = json!(12);
    sim.world.set_component(colony_e, "Clock", clock).unwrap();

    let mut season = sim.world.get_component(colony_e, "Season").unwrap().clone();
    season["day_in_season"] = json!(11);
    season["current"] = json!("spring");
    season["year"] = json!(1);
    sim.world.set_component(colony_e, "Season", season).unwrap();

    sim.step(&mut rt).unwrap();

    let season_after = sim.world.get_component(colony_e, "Season").unwrap();
    assert_eq!(season_after["day_in_season"].as_i64(), Some(0),
        "day_in_season should reset to 0 after season rollover");
    assert_eq!(season_after["current"].as_str(), Some("summer"),
        "season should roll over from spring to summer");
    assert_eq!(season_after["year"].as_i64(), Some(1),
        "year should still be 1 (only rolls over on spring→spring wrap)");
}

#[test]
fn year_increments_on_spring_wrap() {
    // Set season to winter, day_in_season to 11. Trigger day-wrap.
    // Season should roll to spring, year should increment to 2.
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    let colony_e = sim.world.entity("colony").unwrap();

    let mut clock = sim.world.get_component(colony_e, "Clock").unwrap().clone();
    clock["time"] = json!(59.99);
    clock["day"] = json!(48);
    clock["last_day_emit"] = json!(48);
    sim.world.set_component(colony_e, "Clock", clock).unwrap();

    let mut season = sim.world.get_component(colony_e, "Season").unwrap().clone();
    season["day_in_season"] = json!(11);
    season["current"] = json!("winter");
    season["year"] = json!(1);
    sim.world.set_component(colony_e, "Season", season).unwrap();

    sim.step(&mut rt).unwrap();

    let season_after = sim.world.get_component(colony_e, "Season").unwrap();
    assert_eq!(season_after["current"].as_str(), Some("spring"),
        "season should wrap from winter to spring");
    assert_eq!(season_after["year"].as_i64(), Some(2),
        "year should increment on winter→spring wrap");
    assert_eq!(season_after["day_in_season"].as_i64(), Some(0),
        "day_in_season should reset to 0");
}

#[test]
fn weather_timer_decrements_each_tick() {
    // Boot sim, step 1 tick, verify Weather.timer decreased by ~dt (1/60).
    let (mut sim, mut rt) = Runtime::boot(&frontier_dir()).unwrap();
    let colony_e = sim.world.entity("colony").unwrap();

    let weather_before = sim.world.get_component(colony_e, "Weather").unwrap().clone();
    let timer_before = weather_before["timer"].as_f64().unwrap();

    sim.step(&mut rt).unwrap();

    let weather_after = sim.world.get_component(colony_e, "Weather").unwrap();
    let timer_after = weather_after["timer"].as_f64().unwrap();

    // The weather-tick system decrements timer by ctx.dt (1/60 ≈ 0.0167).
    // Allow small floating-point slack.
    let elapsed = timer_before - timer_after;
    assert!(elapsed > 0.0 && elapsed < 0.1,
        "timer should decrement by ~dt (1/60≈0.0167), got elapsed={elapsed}, before={timer_before}, after={timer_after}");
    assert_eq!(weather_after["current"].as_str(), weather_before["current"].as_str(),
        "weather should not change on a single tick (timer hasn't expired)");
}
