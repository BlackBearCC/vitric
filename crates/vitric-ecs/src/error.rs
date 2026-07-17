use std::fmt;

use crate::EntityId;

/// ECS-layer errors. Display output is LLM-facing: states clearly what went wrong, why, and how to fix it.
#[derive(Debug, Clone, PartialEq)]
pub enum EcsError {
    DeadEntity {
        id: EntityId,
        op: String,
    },
    NoSuchComponent {
        id: EntityId,
        component: String,
        available: Vec<String>,
    },
    BadFieldPath {
        id: EntityId,
        path: String,
        reason: String,
    },
    NameTaken {
        name: String,
        holder: EntityId,
    },
    NoSuchEntityName {
        name: String,
    },
    BadSnapshot {
        reason: String,
    },
}

impl fmt::Display for EcsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EcsError::DeadEntity { id, op } => write!(
                f,
                "实体 {id} 已被销毁，无法执行 {op}。\
                 提示：用 world.is_alive 先检查，或排查是否有规则/系统在销毁后仍持有旧引用"
            ),
            EcsError::NoSuchComponent { id, component, available } => write!(
                f,
                "实体 {id} 没有组件 {component:?}。该实体现有组件: [{}]。\
                 提示：检查组件名拼写（区分大小写），或先 set_component 添加",
                available.join(", ")
            ),
            EcsError::BadFieldPath { id, path, reason } => write!(
                f,
                "实体 {id} 的字段路径 {path:?} 无效：{reason}。\
                 提示：路径形式为 \"组件名.字段[.子字段]\"，例如 \"Position.x\""
            ),
            EcsError::NameTaken { name, holder } => write!(
                f,
                "实体名 {name:?} 已被 {holder} 占用。提示：实体名全局唯一，换个名字或先销毁旧实体"
            ),
            EcsError::NoSuchEntityName { name } => write!(
                f,
                "没有名为 {name:?} 的实体。提示：用 world.entity_names 查看现有命名实体"
            ),
            EcsError::BadSnapshot { reason } => {
                write!(f, "快照数据无效：{reason}")
            }
        }
    }
}

impl std::error::Error for EcsError {}
