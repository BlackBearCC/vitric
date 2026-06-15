use std::collections::BTreeMap;

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::{json, Map, Value};

use crate::hash::Fnv1aWriter;
use crate::{EcsError, EntityId};

/// 世界：实体 + 组件的唯一容器。
///
/// 组件值是 `serde_json::Value`（默认 feature 下对象键有序），所以：
/// - `snapshot()` 输出的 JSON 就是世界的完整真相，`restore()` 能精确回到这一刻；
/// - `state_hash()` 对同一状态永远算出同一个值，是录像回放校验的基石。
#[derive(Debug, Default, Clone)]
pub struct World {
    /// 每个槽位的当前代数。实体销毁时代数 +1，旧句柄随之失效。
    generations: Vec<u32>,
    /// 槽位是否存活。
    alive: Vec<bool>,
    /// 可复用的空闲槽位（后进先出，顺序确定）。
    free: Vec<u32>,
    /// 组件名 -> (槽位 -> 组件值)。BTreeMap 保证遍历顺序确定。
    components: BTreeMap<String, BTreeMap<u32, Value>>,
    /// 实体名 -> 槽位。实体名全局唯一，给场景文件和规则引用用。
    names: BTreeMap<String, u32>,
    /// 槽位 -> 实体名（反向索引）。
    slot_names: BTreeMap<u32, String>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- 实体生命周期 ----

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

    /// 按槽位序销毁全部存活实体（场景切换用）。
    ///
    /// 约束：走的是正规 despawn 路径，不是"重置世界"——每个槽位代数 +1、
    /// 名字注销、槽位进 free 列表。旧场景的实体句柄全部干净地失效
    /// （is_alive = false），不存在"新场景实体顶着旧句柄复活"的混淆；
    /// 槽位/代数痕迹保留进快照，切换前后的世界哈希因此可区分。
    pub fn clear_entities(&mut self) {
        for id in self.entities() {
            self.despawn(id).expect("entities() 给出的实体必然存活");
        }
    }

    pub fn is_alive(&self, id: EntityId) -> bool {
        self.alive.get(id.index as usize).copied().unwrap_or(false)
            && self.generations[id.index as usize] == id.generation
    }

    /// 按名字找实体。
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

    /// 所有存活实体，按槽位序（确定性）。
    pub fn entities(&self) -> Vec<EntityId> {
        (0..self.generations.len() as u32)
            .filter(|&i| self.alive[i as usize])
            .map(|i| EntityId { index: i, generation: self.generations[i as usize] })
            .collect()
    }

    // ---- 组件读写 ----

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

    /// 实体现有的组件名列表（有序）。
    pub fn components_of(&self, id: EntityId) -> Vec<String> {
        self.components
            .iter()
            .filter(|(_, col)| col.contains_key(&id.index))
            .map(|(name, _)| name.clone())
            .collect()
    }

    // ---- 字段路径（"Position.x"、"Inventory.items.0.count"）----

    /// 读字段。路径第一段是组件名，其余按 JSON 对象键/数组下标逐层深入。
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

    /// 写字段。中间路径必须已存在（不隐式建结构——失败要显式暴露）。
    pub fn set_field(&mut self, id: EntityId, path: &str, value: Value) -> Result<(), EcsError> {
        let (component, rest) = split_path(id, path)?;
        if rest.is_empty() {
            return self.set_component(id, component, value);
        }
        // 先确认组件存在（借用检查需要先取错误信息再可变借用）
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

    // ---- 查询 ----

    /// 拥有全部指定组件的实体，按槽位序（确定性）。
    pub fn query(&self, required: &[&str]) -> Vec<EntityId> {
        self.entities()
            .into_iter()
            .filter(|&id| required.iter().all(|c| self.has_component(id, c)))
            .collect()
    }

    // ---- 快照 / 哈希 ----

    /// 世界完整状态 → JSON。这就是「状态可往返」：运行中的世界和场景文件是同一种语言。
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

    /// 从快照精确恢复（覆盖当前内容）。
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

    /// 状态哈希：对世界的规范 JSON 字节做 FNV-1a。
    /// 同状态必同哈希；录像回放靠它判定「一帧都没跑偏」。
    ///
    /// **流式、零中间分配**：直接把规范序列化喂进 `Fnv1aWriter`，绕过 `snapshot()` 的
    /// 整世界深拷 + `to_string` 的整世界字符串——两个全世界级分配都省掉。字节与
    /// `fnv1a_64(to_string(snapshot()))` **逐位相同**（[`CanonicalWorld`] 镜像 snapshot
    /// 的结构与键序；由 `canonical_hash_byte_identical_to_snapshot` 锁死，改一位都报红），
    /// 所以已落盘的录像校验点不受影响。
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

/// 规范序列化包装：按 [`World::snapshot`] **完全相同**的结构与键序把世界序列化出去，
/// 但**借用**组件值（不深拷）——给 [`World::state_hash`] 流式哈希用，字节与
/// `to_string(snapshot())` 逐位一致。
///
/// 键序对齐 serde_json 默认（无 `preserve_order`）的 `Map`＝`BTreeMap` 字典序：
/// 顶层 `entities < free < generations < slots`，实体内 `components < id < name`，
/// 组件按组件名排序。组件值本身是 `&Value`，由 serde_json 原样序列化（和 snapshot 里
/// 那份 clone 出来的 Value 序列化字节相同）——这是「不深拷却逐位一致」的关键。
struct CanonicalWorld<'a>(&'a World);

impl Serialize for CanonicalWorld<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let w = self.0;
        // 顶层四键，字典序：entities, free, generations, slots
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
        // 实体键字典序：components, id, name（name 可缺，和 snapshot 一致）
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
        // components_of 源自 BTreeMap 已是字典序；这里 entry 顺序即输出顺序，和 snapshot
        // 把它们塞进 Map(BTreeMap) 后的序列化序一致。
        let names = w.components_of(id);
        let mut m = s.serialize_map(Some(names.len()))?;
        for name in &names {
            m.serialize_entry(name, &w.components[name][&id.index])?;
        }
        m.end()
    }
}

/// 把 "Comp.a.b" 切成 ("Comp", ["a","b"])。
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
    use crate::fnv1a_64; // 等价性测试比对老路径用；非测试构建里 state_hash 已不用它

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
        // 槽位复用，代数 +1，旧句柄失效
        let b = w.spawn();
        assert_eq!(b.index, a.index);
        assert_eq!(b.generation, a.generation + 1);
        assert!(!w.is_alive(a));
        assert!(w.is_alive(b));
        // 对死实体操作给出可读错误
        let err = w.get_component(a, "Position").unwrap_err();
        assert!(matches!(err, EcsError::DeadEntity { .. }));
    }

    #[test]
    fn named_entities() {
        let mut w = World::new();
        let player = w.spawn_named("player").unwrap();
        assert_eq!(w.entity("player").unwrap(), player);
        assert_eq!(w.name_of(player), Some("player"));
        // 重名报错
        assert!(matches!(
            w.spawn_named("player"),
            Err(EcsError::NameTaken { .. })
        ));
        // 销毁后名字释放
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
        // 不存在的组件 → 错误里列出现有组件
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
        // 不隐式建字段
        let err = w.set_field(e, "Inv.diamond", json!(1)).unwrap_err();
        assert!(err.to_string().contains("不会隐式建新字段"), "{err}");
        // 错误信息列出现有字段
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
        w.despawn(tmp).unwrap(); // 留下代数痕迹，快照必须还原它
        let coin = w.spawn();
        w.set_component(coin, "Coin", json!({"value": 10})).unwrap();

        let snap = w.snapshot();
        let h1 = w.state_hash();

        let mut w2 = World::new();
        w2.restore(&snap).unwrap();
        assert_eq!(w2.state_hash(), h1, "恢复后的世界必须哈希一致");
        assert_eq!(w2.entity("player").unwrap(), p);
        assert_eq!(w2.get_field(coin, "Coin.value").unwrap(), &json!(10));
        // 槽位复用行为也必须一致：两边各 spawn 一个，结果相同
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
        // 旧句柄全部干净失效（代数 +1），名字释放，世界空了
        assert!(!w.is_alive(p) && !w.is_alive(c));
        assert!(w.entity("player").is_err());
        assert!(w.entities().is_empty());
        // 名字可立即复用；新实体拿到的代数和旧句柄不同
        let p2 = w.spawn_named("player").unwrap();
        assert_ne!(p2, p);
        assert!(!w.is_alive(p), "新实体不许让旧句柄复活");
        // 清空是确定性操作：同样的序列两遍，哈希一致
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

    /// 流式 state_hash 必须与老路径 `fnv1a(to_string(snapshot()))` **逐位一致**——
    /// 这是已落盘录像校验点不被打破的保证（老路径正是它们的生成者）。改一位都报红。
    #[test]
    fn canonical_hash_byte_identical_to_snapshot() {
        let old = |w: &World| fnv1a_64(serde_json::to_string(&w.snapshot()).unwrap().as_bytes());

        // 形态1：空世界
        let w = World::new();
        assert_eq!(w.state_hash(), old(&w), "空世界");

        // 形态2：单实体单组件
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Position", pos(3.0, 4.0)).unwrap();
        assert_eq!(w.state_hash(), old(&w), "单实体");

        // 形态3：多实体 + 有名/无名 + 多组件 + 嵌套值 + despawn 留代数痕迹 + unicode 名 + null/bool
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

        // 形态4：组件名乱序插入也等价（键序由序列化排，不靠插入序）
        let mut w = World::new();
        let e = w.spawn();
        w.set_component(e, "Zeta", json!({"z": 1})).unwrap();
        w.set_component(e, "Alpha", json!({"a": 1})).unwrap();
        w.set_component(e, "Mid", json!({"m": 1})).unwrap();
        assert_eq!(w.state_hash(), old(&w), "组件名乱序");
    }
}
