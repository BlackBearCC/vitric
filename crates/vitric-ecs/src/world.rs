use std::collections::BTreeMap;

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::{json, Map, Value};

use crate::hash::Fnv1aWriter;
use crate::{EcsError, EntityId};

/// World: the single container for entities + components.
///
/// Component values are `serde_json::Value` (object keys ordered under the default feature), so:
/// - the JSON output of `snapshot()` is the complete truth of the world, and `restore()` can return to this exact moment;
/// - `state_hash()` always yields the same value for the same state, the cornerstone of recording replay verification.
#[derive(Debug, Default, Clone)]
pub struct World {
    /// Current generation of each slot. When an entity is despawned, generation +1, invalidating old handles.
    generations: Vec<u32>,
    /// Whether the slot is alive.
    alive: Vec<bool>,
    /// Reusable free slots (LIFO, deterministic order).
    free: Vec<u32>,
    /// Component name -> (slot -> component value). BTreeMap guarantees deterministic traversal order.
    components: BTreeMap<String, BTreeMap<u32, Value>>,
    /// Entity name -> slot. Entity names are globally unique, for scene files and rules to reference.
    names: BTreeMap<String, u32>,
    /// Slot -> entity name (reverse index).
    slot_names: BTreeMap<u32, String>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- Entity lifecycle ----

    pub fn spawn(&mut self) -> EntityId {
        let index = match self.free.pop() {
            Some(i) => i,
            None => {
                self.generations.push(0);
                self.alive.push(false);
                (self.generations.len() - 1) as u32
            }
        };
        self.alive[index as usize] = true;
        EntityId { index, generation: self.generations[index as usize] }
    }

    pub fn spawn_named(&mut self, name: &str) -> Result<EntityId, EcsError> {
        if let Some(&slot) = self.names.get(name) {
            return Err(EcsError::NameTaken {
                name: name.to_string(),
                holder: EntityId { index: slot, generation: self.generations[slot as usize] },
            });
        }
        let id = self.spawn();
        self.names.insert(name.to_string(), id.index);
        self.slot_names.insert(id.index, name.to_string());
        Ok(id)
    }

    pub fn despawn(&mut self, id: EntityId) -> Result<(), EcsError> {
        self.check_alive(id, "despawn")?;
        self.alive[id.index as usize] = false;
        self.generations[id.index as usize] += 1;
        self.free.push(id.index);
        for column in self.components.values_mut() {
            column.remove(&id.index);
        }
        if let Some(name) = self.slot_names.remove(&id.index) {
            self.names.remove(&name);
        }
        Ok(())
    }

    /// Despawn all alive entities in slot order (for scene switching).
    ///
    /// Constraint: this goes through the normal despawn path, not a "reset world" — each slot's generation +1,
    /// names are deregistered, and slots go into the free list. Old scene entity handles all cleanly invalidate
    /// (is_alive = false), avoiding the confusion of "new scene entities reviving under old handles";
    /// slot/generation traces are preserved into the snapshot, so the world hash differs before vs after the switch.
    pub fn clear_entities(&mut self) {
        for id in self.entities() {
            self.despawn(id).expect("entities() 给出的实体必然存活");
        }
    }

    pub fn is_alive(&self, id: EntityId) -> bool {
        self.alive.get(id.index as usize).copied().unwrap_or(false)
            && self.generations[id.index as usize] == id.generation
    }

    /// Find an entity by name.
    pub fn entity(&self, name: &str) -> Result<EntityId, EcsError> {
        let &slot = self
            .names
            .get(name)
            .ok_or_else(|| EcsError::NoSuchEntityName { name: name.to_string() })?;
        Ok(EntityId { index: slot, generation: self.generations[slot as usize] })
    }

    pub fn name_of(&self, id: EntityId) -> Option<&str> {
        if !self.is_alive(id) {
            return None;
        }
        self.slot_names.get(&id.index).map(|s| s.as_str())
    }

    pub fn entity_names(&self) -> impl Iterator<Item = (&str, EntityId)> {
        self.names.iter().map(|(name, &slot)| {
            (name.as_str(), EntityId { index: slot, generation: self.generations[slot as usize] })
        })
    }

    /// All alive entities, in slot order (deterministic).
    pub fn entities(&self) -> Vec<EntityId> {
        (0..self.generations.len() as u32)
            .filter(|&i| self.alive[i as usize])
            .map(|i| EntityId { index: i, generation: self.generations[i as usize] })
            .collect()
    }

    // ---- Component read/write ----

    pub fn set_component(&mut self, id: EntityId, component: &str, value: Value) -> Result<(), EcsError> {
        self.check_alive(id, &format!("set_component({component})"))?;
        self.components
            .entry(component.to_string())
            .or_default()
            .insert(id.index, value);
        Ok(())
    }

    pub fn get_component(&self, id: EntityId, component: &str) -> Result<&Value, EcsError> {
        self.check_alive(id, &format!("get_component({component})"))?;
        self.components
            .get(component)
            .and_then(|col| col.get(&id.index))
            .ok_or_else(|| EcsError::NoSuchComponent {
                id,
                component: component.to_string(),
                available: self.components_of(id),
            })
    }

    pub fn has_component(&self, id: EntityId, component: &str) -> bool {
        self.is_alive(id)
            && self.components.get(component).is_some_and(|col| col.contains_key(&id.index))
    }

    pub fn remove_component(&mut self, id: EntityId, component: &str) -> Result<(), EcsError> {
        self.check_alive(id, &format!("remove_component({component})"))?;
        let removed = self
            .components
            .get_mut(component)
            .and_then(|col| col.remove(&id.index));
        if removed.is_none() {
            return Err(EcsError::NoSuchComponent {
                id,
                component: component.to_string(),
                available: self.components_of(id),
            });
        }
        Ok(())
    }

    /// List of component names currently on the entity (ordered).
    pub fn components_of(&self, id: EntityId) -> Vec<String> {
        self.components
            .iter()
            .filter(|(_, col)| col.contains_key(&id.index))
            .map(|(name, _)| name.clone())
            .collect()
    }

    // ---- Field paths ("Position.x", "Inventory.items.0.count") ----

    /// Read a field. The first segment of the path is the component name; the rest descend by JSON object key / array index.
    pub fn get_field(&self, id: EntityId, path: &str) -> Result<&Value, EcsError> {
        let (component, rest) = split_path(id, path)?;
        let mut cur = self.get_component(id, component)?;
        for seg in rest.iter() {
            cur = step(cur, seg).map_err(|reason| EcsError::BadFieldPath {
                id,
                path: path.to_string(),
                reason,
            })?;
        }
        Ok(cur)
    }

    /// Write a field. Intermediate path segments must already exist (no implicit structure creation — failures must surface explicitly).
    pub fn set_field(&mut self, id: EntityId, path: &str, value: Value) -> Result<(), EcsError> {
        let (component, rest) = split_path(id, path)?;
        if rest.is_empty() {
            return self.set_component(id, component, value);
        }
        // Confirm the component exists first (borrow checker requires taking the error info before the mutable borrow)
        if !self.has_component(id, component) {
            return Err(EcsError::NoSuchComponent {
                id,
                component: component.to_string(),
                available: self.components_of(id),
            });
        }
        let path_owned = path.to_string();
        let root = self
            .components
            .get_mut(component)
            .and_then(|col| col.get_mut(&id.index))
            .expect("has_component 已确认存在");
        let mut cur = root;
        let (last, mids) = rest.split_last().expect("rest 非空");
        for seg in mids {
            cur = step_mut(cur, seg).map_err(|reason| EcsError::BadFieldPath {
                id,
                path: path_owned.clone(),
                reason,
            })?;
        }
        write_leaf(cur, last, value).map_err(|reason| EcsError::BadFieldPath {
            id,
            path: path_owned,
            reason,
        })
    }

    // ---- Query ----

    /// Entities that have all the specified components, in slot order (deterministic).
    pub fn query(&self, required: &[&str]) -> Vec<EntityId> {
        self.entities()
            .into_iter()
            .filter(|&id| required.iter().all(|c| self.has_component(id, c)))
            .collect()
    }

    // ---- Snapshot / hash ----

    /// Full world state -> JSON. This is "state is round-trippable": a running world and a scene file speak the same language.
    pub fn snapshot(&self) -> Value {
        let entities: Vec<Value> = self
            .entities()
            .into_iter()
            .map(|id| {
                let mut comps = Map::new();
                for name in self.components_of(id) {
                    comps.insert(
                        name.clone(),
                        self.components[&name][&id.index].clone(),
                    );
                }
                let mut obj = Map::new();
                obj.insert("id".into(), json!(id.to_string()));
                if let Some(name) = self.name_of(id) {
                    obj.insert("name".into(), json!(name));
                }
                obj.insert("components".into(), Value::Object(comps));
                Value::Object(obj)
            })
            .collect();
        json!({
            "slots": self.generations.len(),
            "generations": self.generations,
            "free": self.free,
            "entities": entities,
        })
    }

    /// Restore precisely from a snapshot (overwrites current contents).
    pub fn restore(&mut self, snap: &Value) -> Result<(), EcsError> {
        let bad = |reason: &str| EcsError::BadSnapshot { reason: reason.to_string() };
        let obj = snap.as_object().ok_or_else(|| bad("顶层必须是对象"))?;
        let generations: Vec<u32> = serde_json::from_value(
            obj.get("generations").cloned().ok_or_else(|| bad("缺 generations"))?,
        )
        .map_err(|e| bad(&format!("generations 解析失败: {e}")))?;
        let free: Vec<u32> =
            serde_json::from_value(obj.get("free").cloned().ok_or_else(|| bad("缺 free"))?)
                .map_err(|e| bad(&format!("free 解析失败: {e}")))?;
        let entities = obj
            .get("entities")
            .and_then(|v| v.as_array())
            .ok_or_else(|| bad("缺 entities 数组"))?;

        let mut fresh = World {
            alive: vec![false; generations.len()],
            generations,
            free,
            ..Default::default()
        };
        for ent in entities {
            let id: EntityId = ent
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| bad("实体缺 id"))?
                .parse()
                .map_err(|e: String| bad(&e))?;
            if id.index as usize >= fresh.generations.len()
                || fresh.generations[id.index as usize] != id.generation
            {
                return Err(bad(&format!("实体 {id} 与 generations 表不一致")));
            }
            fresh.alive[id.index as usize] = true;
            if let Some(name) = ent.get("name").and_then(|v| v.as_str()) {
                fresh.names.insert(name.to_string(), id.index);
                fresh.slot_names.insert(id.index, name.to_string());
            }
            let comps = ent
                .get("components")
                .and_then(|v| v.as_object())
                .ok_or_else(|| bad(&format!("实体 {id} 缺 components 对象")))?;
            for (cname, cval) in comps {
                fresh
                    .components
                    .entry(cname.clone())
                    .or_default()
                    .insert(id.index, cval.clone());
            }
        }
        *self = fresh;
        Ok(())
    }

    /// State hash: FNV-1a over the world's canonical JSON bytes.
    /// Same state -> same hash; recording replay relies on it to assert "not a single frame drifted".
    ///
    /// **Streaming, zero intermediate allocation**: feeds canonical serialization directly into `Fnv1aWriter`,
    /// bypassing `snapshot()`'s whole-world deep copy + `to_string`'s whole-world string — both world-sized
    /// allocations are skipped. The bytes are **bit-identical** to `fnv1a_64(to_string(snapshot()))`
    /// ([`CanonicalWorld`] mirrors the structure and key order of snapshot; locked down by
    /// `canonical_hash_byte_identical_to_snapshot` — change one bit and it goes red),
    /// so already-on-disk recording checkpoints are unaffected.
    pub fn state_hash(&self) -> u64 {
        let mut hasher = Fnv1aWriter::new();
        serde_json::to_writer(&mut hasher, &CanonicalWorld(self)).expect("世界必可规范序列化");
        hasher.finish()
    }

    fn check_alive(&self, id: EntityId, op: &str) -> Result<(), EcsError> {
        if self.is_alive(id) {
            Ok(())
        } else {
            Err(EcsError::DeadEntity { id, op: op.to_string() })
        }
    }
}

/// Canonical serialization wrapper: serializes the world with **exactly the same** structure and key order as [`World::snapshot`],
/// but **borrows** the component values (no deep copy) — for streaming hash in [`World::state_hash`], byte-identical to
/// `to_string(snapshot())`.
///
/// Key order aligns with serde_json's default (no `preserve_order`) `Map` = `BTreeMap` lexicographic order:
/// top-level `entities < free < generations < slots`, inside an entity `components < id < name`,
/// components sorted by component name. The component value itself is a `&Value`, serialized as-is by serde_json
/// (same bytes as the cloned Value in snapshot) — this is the key to "no deep copy yet bit-identical".
struct CanonicalWorld<'a>(&'a World);

impl Serialize for CanonicalWorld<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let w = self.0;
        // Top-level four keys, lexicographic: entities, free, generations, slots
        let mut m = s.serialize_map(Some(4))?;
        m.serialize_entry("entities", &EntitiesSer(w))?;
        m.serialize_entry("free", &w.free)?;
        m.serialize_entry("generations", &w.generations)?;
        m.serialize_entry("slots", &w.generations.len())?;
        m.end()
    }
}

struct EntitiesSer<'a>(&'a World);

impl Serialize for EntitiesSer<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let ids = self.0.entities();
        let mut seq = s.serialize_seq(Some(ids.len()))?;
        for id in ids {
            seq.serialize_element(&EntitySer(self.0, id))?;
        }
        seq.end()
    }
}

struct EntitySer<'a>(&'a World, EntityId);

impl Serialize for EntitySer<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (w, id) = (self.0, self.1);
        let name = w.name_of(id);
        // Entity key lexicographic order: components, id, name (name may be absent, matching snapshot)
        let mut m = s.serialize_map(Some(if name.is_some() { 3 } else { 2 }))?;
        m.serialize_entry("components", &ComponentsSer(w, id))?;
        m.serialize_entry("id", &id.to_string())?;
        if let Some(n) = name {
            m.serialize_entry("name", n)?;
        }
        m.end()
    }
}

struct ComponentsSer<'a>(&'a World, EntityId);

impl Serialize for ComponentsSer<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let (w, id) = (self.0, self.1);
        // components_of comes from BTreeMap already in lexicographic order; entry order here is the output order,
        // matching the serialization order after snapshot stuffs them into a Map(BTreeMap).
        let names = w.components_of(id);
        let mut m = s.serialize_map(Some(names.len()))?;
        for name in &names {
            m.serialize_entry(name, &w.components[name][&id.index])?;
        }
        m.end()
    }
}

/// Split "Comp.a.b" into ("Comp", ["a","b"]).
fn split_path(id: EntityId, path: &str) -> Result<(&str, Vec<&str>), EcsError> {
    let mut parts = path.split('.');
    let component = parts.next().filter(|s| !s.is_empty()).ok_or_else(|| {
        EcsError::BadFieldPath {
            id,
            path: path.to_string(),
            reason: "路径为空".to_string(),
        }
    })?;
    let rest: Vec<&str> = parts.collect();
    if rest.iter().any(|s| s.is_empty()) {
        return Err(EcsError::BadFieldPath {
            id,
            path: path.to_string(),
            reason: "路径里有空段（连续的点）".to_string(),
        });
    }
    Ok((component, rest))
}

fn step<'v>(cur: &'v Value, seg: &str) -> Result<&'v Value, String> {
    match cur {
        Value::Object(map) => map.get(seg).ok_or_else(|| {
            format!(
                "没有字段 {seg:?}，现有字段: [{}]",
                map.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        }),
        Value::Array(arr) => {
            let i: usize = seg
                .parse()
                .map_err(|_| format!("{seg:?} 不是合法数组下标"))?;
            arr.get(i)
                .ok_or_else(|| format!("数组下标 {i} 越界（长度 {}）", arr.len()))
        }
        other => Err(format!(
            "无法在 {} 类型的值里继续取 {seg:?}",
            type_name(other)
        )),
    }
}

fn step_mut<'v>(cur: &'v mut Value, seg: &str) -> Result<&'v mut Value, String> {
    match cur {
        Value::Object(map) => {
            if !map.contains_key(seg) {
                return Err(format!(
                    "没有字段 {seg:?}，现有字段: [{}]",
                    map.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            Ok(map.get_mut(seg).expect("已确认存在"))
        }
        Value::Array(arr) => {
            let len = arr.len();
            let i: usize = seg
                .parse()
                .map_err(|_| format!("{seg:?} 不是合法数组下标"))?;
            arr.get_mut(i)
                .ok_or_else(|| format!("数组下标 {i} 越界（长度 {len}）"))
        }
        other => Err(format!(
            "无法在 {} 类型的值里继续取 {seg:?}",
            type_name(other)
        )),
    }
}

fn write_leaf(parent: &mut Value, seg: &str, value: Value) -> Result<(), String> {
    match parent {
        Value::Object(map) => {
            if !map.contains_key(seg) {
                return Err(format!(
                    "没有字段 {seg:?}，现有字段: [{}]。写入不会隐式建新字段——字段集合由 schema 决定",
                    map.keys().cloned().collect::<Vec<_>>().join(", ")
                ));
            }
            map.insert(seg.to_string(), value);
            Ok(())
        }
        Value::Array(arr) => {
            let len = arr.len();
            let i: usize = seg
                .parse()
                .map_err(|_| format!("{seg:?} 不是合法数组下标"))?;
            let slot = arr
                .get_mut(i)
                .ok_or_else(|| format!("数组下标 {i} 越界（长度 {len}）"))?;
            *slot = value;
            Ok(())
        }
        other => Err(format!("无法往 {} 类型的值里写 {seg:?}", type_name(other))),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fnv1a_64; // Used by the equivalence test to compare against the old path; non-test builds no longer use it in state_hash

    fn pos(x: f64, y: f64) -> Value {
        json!({"x": x, "y": y})
    }

    #[test]
    fn spawn_despawn_generation() {
        let mut w = World::new();
        let a = w.spawn();
        assert!(w.is_alive(a));
        w.despawn(a).unwrap();
        assert!(!w.is_alive(a));
        // Slot reuse, generation +1, old handle invalidated
        let b = w.spawn();
        assert_eq!(b.index, a.index);
        assert_eq!(b.generation, a.generation + 1);
        assert!(!w.is_alive(a));
        assert!(w.is_alive(b));
        // Operating on a dead entity produces a readable error
        let err = w.get_component(a, "Position").unwrap_err();
        assert!(matches!(err, EcsError::DeadEntity { .. }));
    }

    #[test]
    fn named_entities() {
        let mut w = World::new();
        let player = w.spawn_named("player").unwrap();
        assert_eq!(w.entity("player").unwrap(), player);
        assert_eq!(w.name_of(player), Some("player"));
        // Duplicate name errors
        assert!(matches!(
            w.spawn_named("player"),
            Err(EcsError::NameTaken { .. })
        ));
        // Name released after despawn
        w.despawn(player).unwrap();
        assert!(w.entity("player").is_err());
        w.spawn_named("player").unwrap();
    }

    #[test]
    fn component_crud_and_errors() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", pos(1.0, 2.0)).unwrap();
        assert_eq!(w.get_component(e, "Position").unwrap(), &pos(1.0, 2.0));
        // Non-existent component -> the error lists the existing components
        let err = w.get_component(e, "Velocity").unwrap_err();
        match err {
            EcsError::NoSuchComponent { available, .. } => {
                assert_eq!(available, vec!["Position".to_string()]);
            }
            other => panic!("错误类型不对: {other:?}"),
        }
        w.remove_component(e, "Position").unwrap();
        assert!(!w.has_component(e, "Position"));
    }

    #[test]
    fn field_paths() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Inv", json!({"gold": 5, "items": [{"id": "sword", "count": 1}]}))
            .unwrap();
        assert_eq!(w.get_field(e, "Inv.gold").unwrap(), &json!(5));
        assert_eq!(w.get_field(e, "Inv.items.0.id").unwrap(), &json!("sword"));
        w.set_field(e, "Inv.gold", json!(7)).unwrap();
        assert_eq!(w.get_field(e, "Inv.gold").unwrap(), &json!(7));
        w.set_field(e, "Inv.items.0.count", json!(2)).unwrap();
        assert_eq!(w.get_field(e, "Inv.items.0.count").unwrap(), &json!(2));
        // No implicit field creation
        let err = w.set_field(e, "Inv.diamond", json!(1)).unwrap_err();
        assert!(err.to_string().contains("不会隐式建新字段"), "{err}");
        // Error message lists existing fields
        let err = w.get_field(e, "Inv.golf").unwrap_err();
        assert!(err.to_string().contains("gold"), "{err}");
    }

    #[test]
    fn query_is_deterministic_and_filtered() {
        let mut w = World::new();
        let a = w.spawn();
        let b = w.spawn();
        let c = w.spawn();
        for &e in &[a, b, c] {
            w.set_component(e, "Position", pos(0.0, 0.0)).unwrap();
        }
        w.set_component(b, "Velocity", json!({"x": 1.0, "y": 0.0})).unwrap();
        assert_eq!(w.query(&["Position"]), vec![a, b, c]);
        assert_eq!(w.query(&["Position", "Velocity"]), vec![b]);
        assert!(w.query(&["Ghost"]).is_empty());
    }

    #[test]
    fn snapshot_restore_roundtrip_and_hash() {
        let mut w = World::new();
        let p = w.spawn_named("player").unwrap();
        w.set_component(p, "Position", pos(3.0, 4.0)).unwrap();
        let tmp = w.spawn();
        w.set_component(tmp, "Position", pos(9.0, 9.0)).unwrap();
        w.despawn(tmp).unwrap(); // Leaves a generation trace; the snapshot must restore it
        let coin = w.spawn();
        w.set_component(coin, "Coin", json!({"value": 10})).unwrap();

        let snap = w.snapshot();
        let h1 = w.state_hash();

        let mut w2 = World::new();
        w2.restore(&snap).unwrap();
        assert_eq!(w2.state_hash(), h1, "恢复后的世界必须哈希一致");
        assert_eq!(w2.entity("player").unwrap(), p);
        assert_eq!(w2.get_field(coin, "Coin.value").unwrap(), &json!(10));
        // Slot reuse behavior must also match: spawn one on each side, results are equal
        assert_eq!(w.spawn(), w2.spawn());
        assert_eq!(w.state_hash(), w2.state_hash());
    }

    #[test]
    fn clear_entities_kills_all_handles_and_frees_names() {
        let mut w = World::new();
        let p = w.spawn_named("player").unwrap();
        w.set_component(p, "Position", pos(1.0, 2.0)).unwrap();
        let c = w.spawn();
        w.set_component(c, "Coin", json!({"value": 1})).unwrap();

        w.clear_entities();
        // All old handles cleanly invalidated (generation +1), names released, world empty
        assert!(!w.is_alive(p) && !w.is_alive(c));
        assert!(w.entity("player").is_err());
        assert!(w.entities().is_empty());
        // Names are immediately reusable; the new entity gets a different generation from the old handle
        let p2 = w.spawn_named("player").unwrap();
        assert_ne!(p2, p);
        assert!(!w.is_alive(p), "新实体不许让旧句柄复活");
        // Clear is a deterministic operation: same sequence twice, same hash
        let run = || {
            let mut w = World::new();
            w.spawn_named("a").unwrap();
            w.spawn();
            w.clear_entities();
            w.spawn_named("b").unwrap();
            w.state_hash()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn hash_changes_on_any_state_change() {
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", pos(0.0, 0.0)).unwrap();
        let h0 = w.state_hash();
        w.set_field(e, "Position.x", json!(0.1)).unwrap();
        assert_ne!(w.state_hash(), h0);
    }

    /// The streaming state_hash must be **bit-identical** to the old path `fnv1a(to_string(snapshot()))` —
    /// this is the guarantee that already-on-disk recording checkpoints are not broken (the old path is what generated them). Change one bit and it goes red.
    #[test]
    fn canonical_hash_byte_identical_to_snapshot() {
        let old = |w: &World| fnv1a_64(serde_json::to_string(&w.snapshot()).unwrap().as_bytes());

        // Form 1: empty world
        let w = World::new();
        assert_eq!(w.state_hash(), old(&w), "空世界");

        // Form 2: single entity, single component
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", pos(3.0, 4.0)).unwrap();
        assert_eq!(w.state_hash(), old(&w), "单实体");

        // Form 3: multiple entities + named/unnamed + multiple components + nested values + despawn generation trace + unicode name + null/bool
        let mut w = World::new();
        let p = w.spawn_named("玩家").unwrap();
        w.set_component(p, "Position", pos(1.5, -2.0)).unwrap();
        w.set_component(p, "Inventory", json!({"items": [{"id": "sword", "n": 2}], "gold": 99}))
            .unwrap();
        let tmp = w.spawn();
        w.set_component(tmp, "Position", pos(0.0, 0.0)).unwrap();
        w.despawn(tmp).unwrap();
        let n = w.spawn();
        w.set_component(n, "Velocity", json!({"x": 0.0, "y": 9.81})).unwrap();
        w.set_component(n, "Health", json!({"hp": 100, "alive": true, "tag": null})).unwrap();
        assert_eq!(w.state_hash(), old(&w), "复杂世界");

        // Form 4: components inserted out of order are still equivalent (key order is set by serialization, not insertion order)
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Zeta", json!({"z": 1})).unwrap();
        w.set_component(e, "Alpha", json!({"a": 1})).unwrap();
        w.set_component(e, "Mid", json!({"m": 1})).unwrap();
        assert_eq!(w.state_hash(), old(&w), "组件名乱序");
    }
}
