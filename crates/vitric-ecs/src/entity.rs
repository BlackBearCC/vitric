use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// 实体句柄：index 定位、generation 防悬挂（旧句柄指向已回收的槽位时失效）。
///
/// JSON 表示是字符串 `"e<index>v<generation>"`（如 `"e12v3"`），
/// 让实体引用在场景文件、控制面响应、日志里都是同一种人类可读写法。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntityId {
    pub index: u32,
    pub generation: u32,
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e{}v{}", self.index, self.generation)
    }
}

impl FromStr for EntityId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || {
            format!(
                "无效的实体引用 {s:?}：应为 \"e<序号>v<代数>\" 形式，例如 \"e12v3\""
            )
        };
        let rest = s.strip_prefix('e').ok_or_else(err)?;
        let (idx, gen) = rest.split_once('v').ok_or_else(err)?;
        Ok(EntityId {
            index: idx.parse().map_err(|_| err())?,
            generation: gen.parse().map_err(|_| err())?,
        })
    }
}

impl Serialize for EntityId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for EntityId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_display_parse() {
        let id = EntityId { index: 12, generation: 3 };
        assert_eq!(id.to_string(), "e12v3");
        assert_eq!("e12v3".parse::<EntityId>().unwrap(), id);
    }

    #[test]
    fn parse_rejects_garbage() {
        for bad in ["", "12", "e12", "ev", "e12v", "eXv3", "e1v2v3x"] {
            assert!(bad.parse::<EntityId>().is_err(), "{bad:?} 不该解析成功");
        }
    }

    #[test]
    fn json_roundtrip() {
        let id = EntityId { index: 7, generation: 1 };
        let j = serde_json::to_string(&id).unwrap();
        assert_eq!(j, "\"e7v1\"");
        let back: EntityId = serde_json::from_str(&j).unwrap();
        assert_eq!(back, id);
    }
}
