//! vitric-ecs — introspectable deterministic ECS.
//!
//! Design rules:
//! - Component values are JSON (`serde_json::Value`): all state is inherently serializable, queryable, and round-trippable.
//! - All storage uses `BTreeMap`: iteration order is deterministic, same operation sequence = same state hash.
//! - Errors carry paths and remediation hints, intended for LLM consumption.

mod delta;
mod entity;
mod error;
mod hash;
mod spatial;
mod world;

pub use delta::scene_delta;
pub use entity::EntityId;
pub use error::EcsError;
pub use hash::fnv1a_64;
pub use spatial::{
    ascii_map, relate, relate_in_world, AsciiMap, AsciiMapOpts, Direction, Placement,
    RelativeSpatial,
};
pub use world::World;
