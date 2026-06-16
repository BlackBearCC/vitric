//! vitric-ecs — 可内省的确定性 ECS。
//!
//! 设计铁律：
//! - 组件值就是 JSON（`serde_json::Value`）：一切状态天生可序列化、可查询、可往返。
//! - 所有存储用 `BTreeMap`：迭代顺序确定，同操作序列 = 同状态哈希。
//! - 错误带路径和修复提示，给 LLM 看的。

mod entity;
mod error;
mod hash;
mod spatial;
mod world;

pub use entity::EntityId;
pub use error::EcsError;
pub use hash::fnv1a_64;
pub use spatial::{relate, Direction, Placement, RelativeSpatial};
pub use world::World;
