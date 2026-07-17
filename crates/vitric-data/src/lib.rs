//! vitric-data — declarative data layer: project format, component schema, validation, scene instantiation.
//!
//! This is the heart of the engine: the game's structure is entirely strongly-schema'd JSON data,
//! **write is validation**, error messages are structured (path + error code + fix hint), meant for LLMs to read.

mod error;
mod project;
mod scene;
mod schema;
mod sequence;
mod theme;
mod ui_check;

pub use error::{ValidationError, ValidationReport};
pub use project::{Budgets, Clip, Gates, PlaytestGate, PlaythroughGate, Project, ProjectManifest};
pub use scene::{instantiate_scene, Scene};
pub use schema::{ComponentSchema, FieldDef, FieldType, Schema};
pub use sequence::{SeqStep, Sequence, SEQ_ACTION_KINDS};
pub use theme::{ButtonStyle, Theme};
pub use ui_check::{
    validate_ui_components, UI_ALIGNS, UI_ANCHORS, UI_BUTTON_STATES, UI_CONTAINER_KINDS,
};
