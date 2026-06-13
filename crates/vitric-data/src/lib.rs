//! vitric-data — 声明式数据层：项目格式、组件 schema、校验、场景实例化。
//!
//! 这是引擎的心脏：游戏的结构全部是强 schema 的 JSON 数据，
//! **写入即校验**，错误信息结构化（路径 + 错误码 + 修复提示），是给 LLM 看的。

mod error;
mod project;
mod scene;
mod schema;
mod sequence;
mod theme;
mod ui_check;

pub use error::{ValidationError, ValidationReport};
pub use project::{Budgets, Clip, Gates, PlaythroughGate, Project, ProjectManifest};
pub use scene::{instantiate_scene, Scene};
pub use schema::{ComponentSchema, FieldDef, FieldType, Schema};
pub use sequence::{SeqStep, Sequence, SEQ_ACTION_KINDS};
pub use theme::{ButtonStyle, Theme};
pub use ui_check::{
    validate_ui_components, UI_ALIGNS, UI_ANCHORS, UI_BUTTON_STATES, UI_CONTAINER_KINDS,
};
