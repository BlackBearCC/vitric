//! Scene View — a "what the agent sees" view that the engine auto-emits for the current world state.
//!
//! Three parts (see design draft section 1): observation (a gameplay-state projection with pure decorations stripped),
//! actions (the game's declared input vocabulary), done (termination decision). **Pure projection**: read-only on the world,
//! never modifies world, not hashed, does not affect determinism — so it takes `&World`/`&Engine`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use vitric_ecs::{ascii_map, relate_in_world, AsciiMapOpts, EntityId, Placement, World};
use vitric_rules::{input_actions, Engine};

use crate::config::{
    DerivedSpec, DistanceMetric, ObservationConfig, PlaytestConfig, TerminalOverride,
};

/// An injectable action (one item in the input vocabulary).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    pub action: String,
    /// "pressed" | "released"
    pub phase: String,
}

/// A session's outcome type. Stage 1 only produces Win/Lose/Timeout; soft-lock/unreachable etc. are left to later stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// Win (hit a victory-class terminal event).
    Win,
    /// Lose/death (hit a failure-class terminal event).
    Lose,
    /// Ran to max_ticks without terminating.
    Timeout,
}

/// Which event names count as "this session is over". Default: the victory set + failure set + prefix set
/// (`ending-*` style ending events). The game may override these via playtest.json in later stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSpec {
    /// Event names that classify as Win on hit.
    pub win_events: Vec<String>,
    /// Event names that classify as Lose on hit.
    pub lose_events: Vec<String>,
    /// Classifies as ending on hit (by prefix, all normalized to Win — reaching an ending itself counts as "reaching a conclusion").
    pub ending_prefixes: Vec<String>,
}

impl Default for TerminalSpec {
    fn default() -> TerminalSpec {
        // Jump-style mini-games emit game-won; the generic default collects common win/lose names,
        // so any game can resolve a terminal out of the box without first writing playtest.json.
        TerminalSpec {
            win_events: ["win", "game-won", "victory", "level-complete"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            lose_events: ["lose", "game-over", "death", "dead"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ending_prefixes: vec!["ending".to_string()],
        }
    }
}

impl TerminalSpec {
    /// Apply a `playtest.json` terminal override: whichever set is written is replaced, omitted ones fall back to self
    /// (= the default sets). Overrides "replace" rather than "stack" — once a game declares its own win/lose names they take precedence,
    /// and the generic defaults are no longer mixed in (otherwise game-won and the like would misfire).
    pub fn apply_override(&self, ovr: &TerminalOverride) -> TerminalSpec {
        TerminalSpec {
            win_events: ovr.win_events.clone().unwrap_or_else(|| self.win_events.clone()),
            lose_events: ovr.lose_events.clone().unwrap_or_else(|| self.lose_events.clone()),
            ending_prefixes: ovr
                .ending_prefixes
                .clone()
                .unwrap_or_else(|| self.ending_prefixes.clone()),
        }
    }

    /// **Append** the win event names declared in the project manifest's `gates.playthroughs[].must_emit`
    /// into `win_events` (stacked on top of the default/existing set, not replacing; deduped, original order preserved, new names appended in input order).
    /// Rationale: each project has effectively already declared its authoritative win event (gate enforcement uses it to verify clear recordings),
    /// and scripted/LLM games (e.g. echo's `run-complete`) have victory events not in the generic default set —
    /// without merging them in, they would be misjudged as "nobody can clear it". The playtest.json terminal override still takes effect before this step;
    /// this step only **supplements** the win set with manifest-declared events, leaving lose/ending untouched.
    pub fn with_manifest_must_emit<I, S>(&self, events: I) -> TerminalSpec
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut win = self.win_events.clone();
        for ev in events {
            let ev = ev.as_ref();
            if !win.iter().any(|n| n == ev) {
                win.push(ev.to_string());
            }
        }
        TerminalSpec {
            win_events: win,
            lose_events: self.lose_events.clone(),
            ending_prefixes: self.ending_prefixes.clone(),
        }
    }

    /// Does an event name hit a terminal? On hit returns the corresponding outcome; on miss returns None.
    /// win/ending both normalize to Win (reaching an ending = reaching a conclusion); lose maps to Lose.
    pub fn classify(&self, event_name: &str) -> Option<Outcome> {
        if self.win_events.iter().any(|n| n == event_name) {
            return Some(Outcome::Win);
        }
        if self.lose_events.iter().any(|n| n == event_name) {
            return Some(Outcome::Lose);
        }
        if self.ending_prefixes.iter().any(|p| event_name.starts_with(p.as_str())) {
            return Some(Outcome::Win);
        }
        None
    }
}

/// Purely decorative components: serve only the visuals, are not gameplay state, and are stripped during projection (design draft section 1, render-only).
/// Stage 1 uses a constant list as a fallback; later stages may refine based on schema annotations.
const DECORATIVE_COMPONENTS: &[&str] =
    &["Sprite", "Particle", "Emitter", "Bloom", "Ambient", "Anim", "Camera"];

fn is_decorative(component: &str) -> bool {
    DECORATIVE_COMPONENTS.contains(&component)
}

/// Focal entity (the "self" in egocentric relations): the entity named by the first `Camera`'s `follow` field.
/// The convention matches vitric-sim's camera follow and vitric-render's describe — follow is an entity name (text),
/// and omitted/empty/nonexistent = no focal point (no relative_to_focal output, no distance sorting).
///
/// Returns the focal's (id, world placement). Placement w/h come from `Sprite.w`/`Sprite.h` (default 0 if absent, so adjacency degrades to
/// strict center coincidence). **Configurable later**: playtest.json does not yet expose a focal-entity override (that would touch the config parser,
/// out of scope); for now we use Camera.follow uniformly, adding an observation.focal override later if needed.
fn focal_of(world: &World) -> Option<(EntityId, Placement)> {
    let cam = *world.query(&["Camera"]).first()?;
    let name = world.get_field(cam, "Camera.follow").ok()?.as_str()?.to_string();
    if name.is_empty() {
        return None;
    }
    let id = world.entity(&name).ok()?;
    Some((id, placement_of(world, id)?))
}

/// Read an entity's world placement (Position is required; Sprite dimensions are optional, default 0 if absent).
fn placement_of(world: &World, id: EntityId) -> Option<Placement> {
    let pos = world.get_component(id, "Position").ok()?;
    let x = pos.get("x").and_then(|v| v.as_f64())?;
    let y = pos.get("y").and_then(|v| v.as_f64())?;
    let (w, h) = match world.get_component(id, "Sprite") {
        Ok(s) => (
            s.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0),
            s.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0),
        ),
        Err(_) => (0.0, 0.0),
    };
    Some(Placement::new(x, y, w, h))
}

/// Append `relative_to_focal` to an entity object (an already-built `{"id","name","components"}` Map),
/// and return the primary/secondary sort key `(has_name, distance_to_focal)`. The focal itself is not appended (it has no relation to itself),
/// and nothing is appended when there's no focal or the entity itself has no Position. **Same source as describe**: calls the same world-perception operator
/// `ecs::relate_in_world` — bringing along blocked (whether the line of sight is blocked by a third-party Solid) in one shot.
///
/// The distance in the sort key is 0 when there's no focal (sorting is disabled entirely, the key is ignored).
fn attach_relative(
    world: &World,
    id: EntityId,
    has_name: bool,
    focal: Option<(EntityId, Placement)>,
    obj: &mut Map<String, Value>,
) -> (bool, f64) {
    let mut dist = 0.0;
    if let Some((fid, _fplace)) = focal {
        if fid != id {
            // Only append when the entity itself has a Position (relate_in_world internally takes the placement; confirm coordinates here first,
            // consistent with the original behavior — entities without Position do not output relative_to_focal)
            if placement_of(world, id).is_some() {
                let rel = relate_in_world(world, fid, id);
                dist = rel.distance;
                obj.insert("relative_to_focal".to_string(), rel.to_json());
            }
        }
    }
    (has_name, dist)
}

/// Sort the entity list by primary/secondary key (only enabled when there's a focal): named ones first, then ascending by distance to focal,
/// ties broken by id — deterministic key → deterministic output. `keys[i]` corresponds to `entities[i]`'s (has_name, distance, id).
fn sort_entities_by_focus(entities: &mut [Value], keys: &mut [(bool, f64, EntityId)]) {
    // Sort together: first pair up (key, entity) and sort, then write back. Equal length of entities/keys is guaranteed by the caller.
    let mut idx: Vec<usize> = (0..entities.len()).collect();
    idx.sort_by(|&a, &b| {
        let (na, da, ia) = keys[a];
        let (nb, db, ib) = keys[b];
        nb.cmp(&na) // named=true first
            .then(da.total_cmp(&db))
            .then(ia.cmp(&ib))
    });
    let reordered: Vec<Value> = idx.iter().map(|&i| entities[i].clone()).collect();
    entities.clone_from_slice(&reordered);
}

/// A "what the agent sees" view. observation is machine-readable JSON, shared by strategies and (later) LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneView {
    /// Current relevant state: live entities projected to JSON, with purely decorative components stripped.
    pub observation: Value,
    /// The list of actions available at this moment (the game's declared input vocabulary × {pressed, released}).
    pub actions: Vec<Action>,
    /// Termination decision: None = still in progress, Some = this session has reached its end.
    pub done: Option<Outcome>,
}

impl SceneView {
    /// Derive a view from the world + rules engine + terminal spec (**auto-derive**, no config override).
    /// `done` is always None — termination is only known via "event hit", decided by session after step by scanning events,
    /// not by the static world state. (A static world projection does not contain transient info like "game-won just happened".)
    ///
    /// Equivalent to `derive_with_config(.., &PlaytestConfig::default())` — the default config reproduces this function's output byte for byte
    /// (backward compatibility is locked by the test `default_config_derive_byte_identical_to_plain_derive`).
    pub fn derive(world: &World, engine: &Engine, _terminal: &TerminalSpec) -> SceneView {
        SceneView {
            observation: project_observation(world),
            actions: derive_actions(engine),
            done: None,
        }
    }

    /// Derivation with `playtest.json` overrides (design draft section 1 "auto-derive + optional overrides", section 11 item 6).
    /// Still a **pure projection**: read-only on world/rules, does not modify world, not hashed, does not affect determinism.
    ///
    /// observation is adjusted per config: first pick components via include/exclude, relabel with human-readable names,
    /// then compute declared derived quantities into a `"derived"` sub-object (the default config does not inject the derived key —
    /// guaranteeing byte-identical backward compatibility). Overrides for actions/terminal are handled by the caller (session) using the
    /// TerminalSpec from the config; this function only handles observation.
    pub fn derive_with_config(
        world: &World,
        engine: &Engine,
        _terminal: &TerminalSpec,
        config: &PlaytestConfig,
    ) -> SceneView {
        let mut observation = project_observation_with_config(world, &config.observation);
        // Only inject the derived key when derived quantities are non-empty (omitted when empty — the default config's output must be byte-identical to the old derive)
        if !config.observation.derived.is_empty() {
            let derived = compute_derived(world, &config.observation.derived);
            if let Some(obj) = observation.as_object_mut() {
                obj.insert("derived".to_string(), Value::Object(derived));
            }
        }
        SceneView { observation, actions: derive_actions(engine), done: None }
    }
}

/// Observation projection with config: stacks include/exclude/relabel on top of the auto decoration-strip.
/// When config.observation is entirely empty it is byte-identical to [`project_observation`] (backward compatible).
fn project_observation_with_config(world: &World, cfg: &ObservationConfig) -> Value {
    let use_include = !cfg.include.is_empty();
    let focal = focal_of(world);
    let mut entities = Vec::new();
    let mut keys: Vec<(bool, f64, EntityId)> = Vec::new();
    for id in world.entities() {
        let ent_label = world.name_of(id).map(|s| s.to_string());
        let mut comps = Map::new();
        for cname in world.components_of(id) {
            // Component selection: whitelist takes precedence (when enabled, only keep whitelist items); otherwise strip default decorations + user exclude
            let keep = if use_include {
                cfg.include.iter().any(|c| c == &cname)
            } else {
                !is_decorative(&cname) && !cfg.exclude.iter().any(|c| c == &cname)
            };
            if !keep {
                continue;
            }
            if let Ok(v) = world.get_component(id, &cname) {
                let mut cval = v.clone();
                // relabel: replace the leaf key matching `<entity>/<component>.<field>` with the human-readable name (value/level unchanged)
                if let Some(label) = &ent_label {
                    apply_relabel(&mut cval, &cname, label, cfg);
                }
                comps.insert(cname, cval);
            }
        }
        let mut e = Map::new();
        e.insert("id".to_string(), json!(id.to_string()));
        if let Some(name) = &ent_label {
            e.insert("name".to_string(), json!(name));
        }
        e.insert("components".to_string(), Value::Object(comps));
        // Egocentric relation (same source as describe): not appended for the focal itself / no Position / no focal
        let (named, dist) = attach_relative(world, id, ent_label.is_some(), focal, &mut e);
        keys.push((named, dist, id));
        entities.push(Value::Object(e));
    }
    if focal.is_some() {
        sort_entities_by_focus(&mut entities, &mut keys);
    }
    let mut obs = json!({ "entities": entities });
    attach_ascii_map(world, focal, &mut obs);
    obs
}

/// When there's a focal, add an ASCII grid map centered on the focal to observation (top-level `ascii_map` key).
/// Same source as describe (both call `ecs::ascii_map`, default radius / auto-derived cell) — agents read this map for navigation.
/// Not added when there's no focal (backward compatible: default config / no-follow output is byte-identical).
fn attach_ascii_map(world: &World, focal: Option<(EntityId, Placement)>, obs: &mut Value) {
    if let Some((fid, _)) = focal {
        if let Some(map) = obs.as_object_mut() {
            map.insert("ascii_map".to_string(), ascii_map(world, fid, &AsciiMapOpts::default()).to_json());
        }
    }
}

/// Apply relabel to a component value: iterate cfg.relabel, and for any path shaped `<entity>/<this component>.<field path>`,
/// replace the leaf's last key with the human-readable name. Only top-level field renames are supported (`component.field`) — enough to cover the derived-quantity use case.
fn apply_relabel(cval: &mut Value, cname: &str, ent_label: &str, cfg: &ObservationConfig) {
    let prefix = format!("{ent_label}/{cname}.");
    if let Some(obj) = cval.as_object_mut() {
        for r in &cfg.relabel {
            if let Some(field) = r.path.strip_prefix(&prefix) {
                // Only handle top-level fields (field does not contain another '.'); nested fields are left as-is (good enough)
                if !field.contains('.') {
                    if let Some(v) = obj.remove(field) {
                        obj.insert(r.name.clone(), v);
                    }
                }
            }
        }
    }
}

/// Compute all derived quantities into a `derived` sub-object (key = derived-quantity name; deterministic order is guaranteed by declaration order).
fn compute_derived(world: &World, specs: &[DerivedSpec]) -> Map<String, Value> {
    let mut out = Map::new();
    for spec in specs {
        let v = match spec {
            DerivedSpec::Distance { from, to, metric, .. } => {
                derived_distance(world, from, to, *metric)
            }
            DerivedSpec::Alias { path, .. } => derived_alias(world, path),
            DerivedSpec::Count { component, .. } => derived_count(world, component),
        };
        out.insert(spec.name().to_string(), v);
    }
    out
}

/// Position distance between two named entities. If either entity is missing / has no Position / has non-numeric coordinates → Null.
fn derived_distance(world: &World, from: &str, to: &str, metric: DistanceMetric) -> Value {
    let Some((ax, ay)) = entity_position(world, from) else { return Value::Null };
    let Some((bx, by)) = entity_position(world, to) else { return Value::Null };
    let d = match metric {
        DistanceMetric::Manhattan => (ax - bx).abs() + (ay - by).abs(),
        DistanceMetric::Euclidean => ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt(),
    };
    json!(d)
}

/// Read a named entity's Position.{x,y} (both must be numbers to count).
fn entity_position(world: &World, name: &str) -> Option<(f64, f64)> {
    let (_, id) = world.entity_names().find(|(n, _)| *n == name)?;
    let pos = world.get_component(id, "Position").ok()?;
    let x = pos.get("x").and_then(|v| v.as_f64())?;
    let y = pos.get("y").and_then(|v| v.as_f64())?;
    Some((x, y))
}

/// Field alias: mirror verbatim the value pointed to by the observation path `<entity>/<component>.<field path>`.
/// If unresolvable (entity/component/field missing) → Null.
fn derived_alias(world: &World, path: &str) -> Value {
    let Some((ent, rest)) = path.split_once('/') else { return Value::Null };
    let Some((comp, field_path)) = rest.split_once('.') else { return Value::Null };
    let Some((_, id)) = world.entity_names().find(|(n, _)| *n == ent) else {
        return Value::Null;
    };
    let Ok(cval) = world.get_component(id, comp) else { return Value::Null };
    // Drill down along the field path (supports a.b.c nesting; array indices also go through the unified object/array step)
    let mut cur = cval;
    for seg in field_path.split('.') {
        cur = match cur {
            Value::Object(m) => match m.get(seg) {
                Some(v) => v,
                None => return Value::Null,
            },
            Value::Array(a) => match seg.parse::<usize>().ok().and_then(|i| a.get(i)) {
                Some(v) => v,
                None => return Value::Null,
            },
            _ => return Value::Null,
        };
    }
    cur.clone()
}

/// Number of live entities with a given component.
fn derived_count(world: &World, component: &str) -> Value {
    let n = world
        .entities()
        .into_iter()
        .filter(|&id| world.components_of(id).iter().any(|c| c == component))
        .count();
    json!(n)
}

/// Observation projection: iterate live entities (slot order = deterministic), projecting each entity to
/// `{"id":..,"name":..,"components":{gameplay components...}}`, stripping purely decorative components.
/// Entities that are entirely decorative (e.g. pure camera / pure background Sprite) keep an empty components list —
/// its existence itself is state (whether the entity is present).
fn project_observation(world: &World) -> Value {
    let focal = focal_of(world);
    let mut entities = Vec::new();
    let mut keys: Vec<(bool, f64, EntityId)> = Vec::new();
    for id in world.entities() {
        let mut comps = Map::new();
        for cname in world.components_of(id) {
            if is_decorative(&cname) {
                continue;
            }
            if let Ok(v) = world.get_component(id, &cname) {
                comps.insert(cname, v.clone());
            }
        }
        let has_name = world.name_of(id).is_some();
        let mut e = Map::new();
        e.insert("id".to_string(), json!(id.to_string()));
        if let Some(name) = world.name_of(id) {
            e.insert("name".to_string(), json!(name));
        }
        e.insert("components".to_string(), Value::Object(comps));
        // Egocentric relation (same source as describe): not appended for the focal itself / no Position / no focal
        let (named, dist) = attach_relative(world, id, has_name, focal, &mut e);
        keys.push((named, dist, id));
        entities.push(Value::Object(e));
    }
    if focal.is_some() {
        sort_entities_by_focus(&mut entities, &mut keys);
    }
    let mut obs = json!({ "entities": entities });
    attach_ascii_map(world, focal, &mut obs);
    obs
}

/// Action derivation: enumerate the distinct actions of all input triggers in the rules, pairing each with {pressed, released}
/// (pressed first = the strategy's mainstay, released also listed in full).
///
/// The "scan rules to collect the action vocabulary" logic has been moved to vitric-rules' [`input_actions`] (the natural home for rules introspection;
/// the describe control surface and this site share the same copy, no longer duplicated). Here we only do the SceneView-side adaptation:
/// take the distinct actions in order of appearance, and expand each into the pressed/released phases — the SceneView affordance contract is
/// "both phases are injectable", and does not take the phase actually declared in the rules (an action declared only as pressed in the rules
/// still has a legal injectable release input).
fn derive_actions(engine: &Engine) -> Vec<Action> {
    let mut actions = Vec::new();
    for ia in input_actions(&engine.rules) {
        actions.push(Action { action: ia.action.clone(), phase: "pressed".to_string() });
        actions.push(Action { action: ia.action, phase: "released".to_string() });
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vitric_data::Schema;
    use vitric_rules::RuleSet;

    fn engine(rules: Value, schema: Value) -> Engine {
        let schema = Schema::parse(&schema, "schema.json").unwrap();
        Engine::new(RuleSet::parse(&rules, "rules.json").unwrap(), schema)
    }

    fn jump_like_engine() -> Engine {
        engine(
            json!({"rules": [
                {"id": "left", "on": {"event": "input", "filter": {"action": "left", "phase": "pressed"}},
                 "do": [{"set": "@hero.Velocity.x", "to": -8}]},
                {"id": "left-stop", "on": {"event": "input", "filter": {"action": "left", "phase": "released"}},
                 "do": [{"set": "@hero.Velocity.x", "to": 0}]},
                {"id": "right", "on": {"event": "input", "filter": {"action": "right", "phase": "pressed"}},
                 "do": [{"set": "@hero.Velocity.x", "to": 8}]},
                {"id": "jump", "on": {"event": "input", "filter": {"action": "space", "phase": "pressed"}},
                 "do": [{"set": "@hero.Velocity.y", "to": 14}]},
                // Rules not triggered by input should not contribute to the action vocabulary
                {"id": "tickrule", "on": "tick", "do": [{"emit": "noop", "data": {}}]}
            ]}),
            json!({"components": {
                "Velocity": {"fields": {"x": {"type": "number"}, "y": {"type": "number"}}}
            }}),
        )
    }

    #[test]
    fn actions_come_from_distinct_input_rule_actions() {
        let eng = jump_like_engine();
        let view = SceneView::derive(&World::new(), &eng, &TerminalSpec::default());
        // distinct action set = {left, right, space}, each ×{pressed,released}
        let names: Vec<&str> = view.actions.iter().map(|a| a.action.as_str()).collect();
        assert_eq!(view.actions.len(), 6, "3 个动作 ×2 phase: {:?}", view.actions);
        // left appears only once (even with both pressed and released rules for left, it counts as one action)
        assert_eq!(names.iter().filter(|n| **n == "left").count(), 2);
        assert!(names.contains(&"left") && names.contains(&"right") && names.contains(&"space"));
        // pressed precedes its corresponding released
        let left_pressed = view.actions.iter().position(|a| a.action == "left" && a.phase == "pressed");
        let left_released = view.actions.iter().position(|a| a.action == "left" && a.phase == "released");
        assert!(left_pressed < left_released);
    }

    #[test]
    fn actions_derivation_is_deterministic() {
        let eng = jump_like_engine();
        let a = SceneView::derive(&World::new(), &eng, &TerminalSpec::default());
        let b = SceneView::derive(&World::new(), &eng, &TerminalSpec::default());
        assert_eq!(a.actions, b.actions);
    }

    #[test]
    fn observation_drops_decorative_keeps_gameplay() {
        let eng = jump_like_engine();
        let mut w = World::new();
        let hero = w.spawn_named("hero").unwrap();
        w.set_component(hero, "Velocity", json!({"x": 1.0, "y": 2.0})).unwrap();
        // Sprite is purely decorative and should be stripped
        let schema = Schema::parse(
            &json!({"components": {"Sprite": {"fields": {"w": {"type": "number"}}}}}),
            "s.json",
        )
        .unwrap();
        let _ = schema; // Sprite is outside this test's schema validation scope; stuffing the raw value is fine
        w.set_component(hero, "Sprite", json!({"w": 1.0})).unwrap();

        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let ents = view.observation.get("entities").unwrap().as_array().unwrap();
        assert_eq!(ents.len(), 1);
        let comps = ents[0].get("components").unwrap().as_object().unwrap();
        assert!(comps.contains_key("Velocity"), "玩法组件保留: {comps:?}");
        assert!(!comps.contains_key("Sprite"), "装饰组件剔除: {comps:?}");
        assert_eq!(ents[0].get("name").unwrap(), &json!("hero"));
    }

    // ---- Stage 6: config overrides (include/exclude/relabel/derived/terminal) ----

    use crate::config::{
        DerivedSpec, DistanceMetric, ObservationConfig, PlaytestConfig, Relabel, TerminalOverride,
    };

    /// Build a world with Position (hero at origin, flag at (3,4)) + some decorative components.
    fn pos_world() -> World {
        let mut w = World::new();
        let hero = w.spawn_named("hero").unwrap();
        w.set_component(hero, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(hero, "Velocity", json!({"x": 1.0, "y": 0.0})).unwrap();
        w.set_component(hero, "Sprite", json!({"w": 1.0})).unwrap();
        let flag = w.spawn_named("flag").unwrap();
        w.set_component(flag, "Position", json!({"x": 3.0, "y": 4.0})).unwrap();
        w.set_component(flag, "Goal", json!({})).unwrap();
        w
    }

    #[test]
    fn default_config_derive_byte_identical_to_plain_derive() {
        // Backward-compat ironclad rule: the default config's derive_with_config must be byte-identical to the old derive
        let eng = jump_like_engine();
        let w = pos_world();
        let a = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let b = SceneView::derive_with_config(
            &w,
            &eng,
            &TerminalSpec::default(),
            &PlaytestConfig::default(),
        );
        let ja = serde_json::to_string(&a).unwrap();
        let jb = serde_json::to_string(&b).unwrap();
        assert_eq!(ja, jb, "默认 config 必须和老行为逐字节一致");
    }

    #[test]
    fn config_include_whitelist_keeps_only_listed() {
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                include: vec!["Position".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let ents = view.observation.get("entities").unwrap().as_array().unwrap();
        let hero = ents.iter().find(|e| e.get("name") == Some(&json!("hero"))).unwrap();
        let comps = hero.get("components").unwrap().as_object().unwrap();
        assert!(comps.contains_key("Position"), "白名单内保留: {comps:?}");
        assert!(!comps.contains_key("Velocity"), "白名单外剔除: {comps:?}");
    }

    #[test]
    fn config_exclude_drops_extra_components() {
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                exclude: vec!["Velocity".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let ents = view.observation.get("entities").unwrap().as_array().unwrap();
        let hero = ents.iter().find(|e| e.get("name") == Some(&json!("hero"))).unwrap();
        let comps = hero.get("components").unwrap().as_object().unwrap();
        assert!(comps.contains_key("Position"), "未排除的留: {comps:?}");
        assert!(!comps.contains_key("Velocity"), "exclude 的额外剔除: {comps:?}");
    }

    #[test]
    fn config_relabel_renames_leaf_key() {
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                relabel: vec![Relabel {
                    path: "hero/Position.x".to_string(),
                    name: "横坐标".to_string(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let ents = view.observation.get("entities").unwrap().as_array().unwrap();
        let hero = ents.iter().find(|e| e.get("name") == Some(&json!("hero"))).unwrap();
        let pos = hero.get("components").unwrap().get("Position").unwrap().as_object().unwrap();
        assert!(pos.contains_key("横坐标"), "x 被改人话名: {pos:?}");
        assert!(!pos.contains_key("x"), "原键被改掉: {pos:?}");
        assert!(pos.contains_key("y"), "没动的字段保留");
    }

    #[test]
    fn config_derived_distance_manhattan() {
        let eng = jump_like_engine();
        let w = pos_world(); // hero(0,0) flag(3,4)
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                derived: vec![DerivedSpec::Distance {
                    name: "to_exit".to_string(),
                    from: "hero".to_string(),
                    to: "flag".to_string(),
                    metric: DistanceMetric::Manhattan,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let derived = view.observation.get("derived").unwrap();
        assert_eq!(derived.get("to_exit").unwrap().as_f64().unwrap(), 7.0, "|3|+|4|=7");
    }

    #[test]
    fn config_derived_distance_euclidean() {
        let eng = jump_like_engine();
        let w = pos_world(); // hero(0,0) flag(3,4)
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                derived: vec![DerivedSpec::Distance {
                    name: "d".to_string(),
                    from: "hero".to_string(),
                    to: "flag".to_string(),
                    metric: DistanceMetric::Euclidean,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let d = view.observation.get("derived").unwrap().get("d").unwrap().as_f64().unwrap();
        assert!((d - 5.0).abs() < 1e-9, "sqrt(9+16)=5，实际 {d}");
    }

    #[test]
    fn config_derived_distance_null_when_entity_missing() {
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                derived: vec![DerivedSpec::Distance {
                    name: "d".to_string(),
                    from: "hero".to_string(),
                    to: "nonexistent".to_string(),
                    metric: DistanceMetric::Manhattan,
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let d = view.observation.get("derived").unwrap().get("d").unwrap();
        assert!(d.is_null(), "缺实体 → null: {d:?}");
    }

    #[test]
    fn config_derived_alias_mirrors_value() {
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                derived: vec![DerivedSpec::Alias {
                    name: "vx".to_string(),
                    path: "hero/Velocity.x".to_string(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let vx = view.observation.get("derived").unwrap().get("vx").unwrap().as_f64().unwrap();
        assert_eq!(vx, 1.0, "alias 镜像 hero/Velocity.x=1.0");
    }

    #[test]
    fn config_derived_count_entities_with_component() {
        let eng = jump_like_engine();
        let w = pos_world(); // hero+flag both have Position
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                derived: vec![
                    DerivedSpec::Count {
                        name: "positioned".to_string(),
                        component: "Position".to_string(),
                    },
                    DerivedSpec::Count {
                        name: "goals".to_string(),
                        component: "Goal".to_string(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        let derived = view.observation.get("derived").unwrap();
        assert_eq!(derived.get("positioned").unwrap().as_u64().unwrap(), 2);
        assert_eq!(derived.get("goals").unwrap().as_u64().unwrap(), 1);
    }

    #[test]
    fn config_alias_reads_relabeled_or_original_path() {
        // alias's path uses the original (pre-relabel) path — derived quantities are appended after projection and reference the original field name
        let eng = jump_like_engine();
        let w = pos_world();
        let cfg = PlaytestConfig {
            observation: ObservationConfig {
                relabel: vec![Relabel {
                    path: "hero/Velocity.x".to_string(),
                    name: "速度".to_string(),
                }],
                derived: vec![DerivedSpec::Alias {
                    name: "vx".to_string(),
                    path: "hero/Velocity.x".to_string(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let view = SceneView::derive_with_config(&w, &eng, &TerminalSpec::default(), &cfg);
        // relabel changed the keys in observation, but alias still reads the value via the original path (derived is computed on the original world)
        let vx = view.observation.get("derived").unwrap().get("vx").unwrap().as_f64().unwrap();
        assert_eq!(vx, 1.0);
    }

    #[test]
    fn terminal_override_applies_custom_events() {
        // TerminalSpec::apply_override applies the custom win/lose names
        let ovr = TerminalOverride {
            win_events: Some(vec!["reached-exit".to_string()]),
            lose_events: Some(vec!["fell".to_string()]),
            ending_prefixes: None,
        };
        let spec = TerminalSpec::default().apply_override(&ovr);
        assert_eq!(spec.classify("reached-exit"), Some(Outcome::Win));
        assert_eq!(spec.classify("fell"), Some(Outcome::Lose));
        // ending prefixes not overridden fall back to the default
        assert_eq!(spec.classify("ending-x"), Some(Outcome::Win));
        // old default win names are replaced (game-won no longer recognized)
        assert_eq!(spec.classify("game-won"), None);
    }

    #[test]
    fn with_manifest_must_emit_appends_and_dedups() {
        // manifest-declared must_emit is merged into the win set: new names are recognized, old defaults remain (appended not replaced)
        let spec = TerminalSpec::default().with_manifest_must_emit(["run-complete"]);
        assert_eq!(spec.classify("run-complete"), Some(Outcome::Win));
        assert_eq!(spec.classify("game-won"), Some(Outcome::Win), "默认 win 名仍保留");
        // already-present names (game-won) are not duplicated in the set
        let spec2 = TerminalSpec::default().with_manifest_must_emit(["game-won", "run-complete"]);
        assert_eq!(spec2.win_events.iter().filter(|n| *n == "game-won").count(), 1, "去重");
        assert!(spec2.win_events.iter().any(|n| n == "run-complete"));
    }

    #[test]
    fn with_manifest_must_emit_empty_is_unchanged() {
        // empty manifest (no gates declared) degrades to the default set — backward compatibility rule
        let base = TerminalSpec::default();
        let merged = base.with_manifest_must_emit(Vec::<String>::new());
        assert_eq!(merged, base, "无 must_emit 时必须和默认逐字段一致");
    }

    #[test]
    fn with_manifest_must_emit_stacks_on_override() {
        // playtest.json override takes effect first, then manifest must_emit stacks on top: both are recognized
        let ovr = TerminalOverride {
            win_events: Some(vec!["reached-exit".to_string()]),
            lose_events: None,
            ending_prefixes: None,
        };
        let spec = TerminalSpec::default()
            .apply_override(&ovr)
            .with_manifest_must_emit(["quest-done"]);
        assert_eq!(spec.classify("reached-exit"), Some(Outcome::Win), "覆盖的 win 名认得");
        assert_eq!(spec.classify("quest-done"), Some(Outcome::Win), "清单 must_emit 也认得");
        assert_eq!(spec.classify("game-won"), None, "覆盖替换掉了默认 win 名");
    }

    // ---- Egocentric relations + primary/secondary sort (relative_to_focal) ----

    /// World with a follow camera: hero(0,0) focal + coin(3,0) right neighbor + anon unnamed near neighbor(1,0).
    fn focal_world() -> World {
        let mut w = World::new();
        let hero = w.spawn_named("hero").unwrap();
        w.set_component(hero, "Position", json!({"x": 0.0, "y": 0.0})).unwrap();
        w.set_component(hero, "Sprite", json!({"w": 2.0, "h": 2.0})).unwrap();
        let coin = w.spawn_named("coin").unwrap();
        w.set_component(coin, "Position", json!({"x": 3.0, "y": 0.0})).unwrap();
        let cam = w.spawn();
        w.set_component(cam, "Camera", json!({"x": 0.0, "y": 0.0, "follow": "hero"})).unwrap();
        w
    }

    fn find_ent<'a>(obs: &'a Value, name: &str) -> &'a Value {
        obs.get("entities").unwrap().as_array().unwrap().iter()
            .find(|e| e.get("name") == Some(&json!(name))).unwrap()
    }

    #[test]
    fn observation_attaches_relative_to_focal() {
        let eng = jump_like_engine();
        let w = focal_world();
        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let coin = find_ent(&view.observation, "coin");
        let rel = coin.get("relative_to_focal").unwrap();
        assert_eq!(rel["direction"], json!("right"));
        assert_eq!(rel["distance"], json!(3.0));
        assert_eq!(rel["same_row"], json!(true));
    }

    #[test]
    fn observation_focal_has_no_relative_block() {
        let eng = jump_like_engine();
        let w = focal_world();
        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let hero = find_ent(&view.observation, "hero");
        assert!(hero.get("relative_to_focal").is_none(), "焦点自己不输出");
    }

    #[test]
    fn observation_relative_value_matches_describe_shared_function() {
        // same source as describe: both call ecs::relate_in_world, values must match.
        use vitric_ecs::relate_in_world;
        let eng = jump_like_engine();
        let w = focal_world();
        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let coin = find_ent(&view.observation, "coin");
        let hero = w.entity("hero").unwrap();
        let coin_id = w.entity("coin").unwrap();
        // call the shared operator directly to get the expected value — SceneView must match byte for byte (including blocked)
        let expected = relate_in_world(&w, hero, coin_id).to_json();
        assert_eq!(coin.get("relative_to_focal").unwrap(), &expected);
    }

    #[test]
    fn observation_no_camera_no_relative_no_reorder() {
        // backward compatibility: no camera / no follow → don't append relative_to_focal, keep slot order
        let eng = jump_like_engine();
        let w = pos_world(); // no Camera
        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        for e in view.observation.get("entities").unwrap().as_array().unwrap() {
            assert!(e.get("relative_to_focal").is_none());
        }
    }

    #[test]
    fn observation_primary_sort_named_then_distance() {
        let eng = jump_like_engine();
        let mut w = focal_world(); // hero(0,0 focal) + coin(3,0)
        // unnamed near neighbor(1,0)
        let near = w.spawn();
        w.set_component(near, "Position", json!({"x": 1.0, "y": 0.0})).unwrap();
        // named far neighbor star(5,0)
        let star = w.spawn_named("star").unwrap();
        w.set_component(star, "Position", json!({"x": 5.0, "y": 0.0})).unwrap();

        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let ents = view.observation.get("entities").unwrap().as_array().unwrap();
        let names: Vec<String> = ents.iter()
            .map(|e| e.get("name").and_then(|n| n.as_str()).unwrap_or("<anon>").to_string())
            .collect();
        // note: the camera entity (unnamed, no Position) is also in the list, sorted into the unnamed segment.
        // named segment by distance: hero(0) < coin(3) < star(5)
        assert_eq!(&names[0..3], &["hero", "coin", "star"], "有名字优先、按距离升序: {names:?}");
        assert!(names[3..].iter().all(|n| n == "<anon>"), "无名实体殿后: {names:?}");
    }

    #[test]
    fn terminal_classifies_win_lose_and_ending_prefix() {
        let spec = TerminalSpec::default();
        assert_eq!(spec.classify("game-won"), Some(Outcome::Win));
        assert_eq!(spec.classify("win"), Some(Outcome::Win));
        assert_eq!(spec.classify("game-over"), Some(Outcome::Lose));
        assert_eq!(spec.classify("ending-true"), Some(Outcome::Win));
        assert_eq!(spec.classify("ending-bad"), Some(Outcome::Win));
        assert_eq!(spec.classify("input"), None);
        assert_eq!(spec.classify("collision"), None);
    }

    // ---- Line-of-sight occlusion (blocked) + ASCII grid map (ascii_map) ----

    /// Place a w×h Solid wall at (x,y) (Solid+Position+Collider).
    fn add_wall(w: &mut World, x: f64, y: f64, cw: f64, ch: f64) {
        let e = w.spawn();
        w.set_component(e, "Position", json!({"x": x, "y": y})).unwrap();
        w.set_component(e, "Collider", json!({"w": cw, "h": ch})).unwrap();
        w.set_component(e, "Solid", json!({})).unwrap();
    }

    #[test]
    fn observation_relative_carries_blocked() {
        // relative_to_focal carries blocked: false without a wall, true after adding one
        let eng = jump_like_engine();
        let mut w = focal_world(); // hero(0,0) focal + coin(3,0)
        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let coin = find_ent(&view.observation, "coin");
        assert_eq!(coin["relative_to_focal"]["blocked"], json!(false), "无墙不挡");
        // erect a wall between hero and coin
        add_wall(&mut w, 1.5, 0.0, 0.5, 2.0);
        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let coin = find_ent(&view.observation, "coin");
        assert_eq!(coin["relative_to_focal"]["blocked"], json!(true), "中间有墙 → 挡");
    }

    #[test]
    fn observation_has_ascii_map_with_focus() {
        // has focal → observation top-level has ascii_map, @ in the center, coin enters the legend
        let eng = jump_like_engine();
        let w = focal_world();
        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let map = &view.observation["ascii_map"];
        assert!(map.is_object(), "有焦点 → 有 ascii_map: {map:?}");
        let grid = map["grid"].as_array().unwrap();
        let center = grid.len() / 2;
        assert_eq!(grid[center].as_str().unwrap().chars().nth(center), Some('@'));
        assert!(map["legend"].as_object().unwrap().values().any(|v| v == "coin"));
    }

    #[test]
    fn observation_no_ascii_map_without_focus() {
        // backward compatibility: no Camera.follow → no ascii_map key appears
        let eng = jump_like_engine();
        let w = pos_world(); // no Camera
        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        assert!(view.observation.get("ascii_map").is_none(), "无焦点不该有 ascii_map");
    }

    #[test]
    fn observation_ascii_map_matches_shared_function() {
        // same source as describe: both call ecs::ascii_map (default opts), values match byte for byte
        use vitric_ecs::{ascii_map, AsciiMapOpts};
        let eng = jump_like_engine();
        let w = focal_world();
        let view = SceneView::derive(&w, &eng, &TerminalSpec::default());
        let hero = w.entity("hero").unwrap();
        let expected = ascii_map(&w, hero, &AsciiMapOpts::default()).to_json();
        assert_eq!(&view.observation["ascii_map"], &expected);
    }

    #[test]
    fn config_observation_also_has_ascii_map_and_blocked() {
        // config-derived (derive_with_config) also attaches ascii_map + blocked
        use crate::config::PlaytestConfig;
        let eng = jump_like_engine();
        let mut w = focal_world();
        add_wall(&mut w, 1.5, 0.0, 0.5, 2.0);
        let view = SceneView::derive_with_config(
            &w, &eng, &TerminalSpec::default(), &PlaytestConfig::default(),
        );
        assert!(view.observation.get("ascii_map").is_some(), "config 派生也有 ascii_map");
        let coin = find_ent(&view.observation, "coin");
        assert_eq!(coin["relative_to_focal"]["blocked"], json!(true));
    }
}
