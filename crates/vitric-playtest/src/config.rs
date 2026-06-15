//! 每游戏的试玩视图覆盖（设计稿一节「自动推 + 可选覆盖」、十一节第 6 条）。
//!
//! 默认情况下 Scene View 从 schema/rules 自动推一份能跑的（任何游戏开箱即试）。游戏可选
//! 在项目根放一份 `playtest.json` 把视图打磨得更顺：挑相关字段、改人话标签、声明派生量
//! （如「到出口的曼哈顿距离」）、给 greedy 指个优化目标、覆盖终止事件名。
//!
//! **覆盖是打磨，不是前提**：没有 `playtest.json` 时 [`PlaytestConfig::default`] 让所有行为
//! 和阶段 1~5 逐字节一致（向后兼容由测试锁）。这里只解析 + 校验配置成结构体，怎么把它套进
//! 投影/策略/终止见 [`crate::scene_view`] 和 [`crate::strategy`]。
//!
//! **派生量不做 DSL**：只支持三种内置量（distance/alias/count），刻意不做表达式引擎——
//! 复杂派生留给游戏自己在规则里算出一个字段，这里只做「把已有数据换个角度看」的轻量声明。

use serde_json::Value;

/// 一份每游戏试玩配置。所有字段都可缺省（缺了就回到自动推的默认行为）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaytestConfig {
    /// 观测投影的覆盖（挑组件/重命名/派生量）。
    pub observation: ObservationConfig,
    /// greedy 的优化目标（None=无目标，greedy 退化为可复现随机，和阶段 1 一致）。
    pub goal: Option<GoalSpec>,
    /// 终止事件名覆盖（None=用 TerminalSpec::default）。
    pub terminal: Option<TerminalOverride>,
}

/// 观测投影的覆盖。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObservationConfig {
    /// 白名单：只保留这些组件名（非空时覆盖默认装饰剔除——白名单外的一律剔）。
    /// 空=不启用白名单，走默认「剔装饰、留其余」。
    pub include: Vec<String>,
    /// 黑名单：额外剔除这些组件名（在默认装饰剔除之上叠加）。
    pub exclude: Vec<String>,
    /// 字段重命名：observation 里的路径 → 人话标签。路径形如 `<实体>/<组件>.<字段>`
    /// （和数值遥测同款 key），命中就把那个叶子的键换成人话名（不改值、不改结构层级）。
    pub relabel: Vec<Relabel>,
    /// 派生量声明：算进 observation 的一个 `"derived"` 子对象（纯投影，不进哈希）。
    pub derived: Vec<DerivedSpec>,
}

/// 一条字段重命名。
#[derive(Debug, Clone, PartialEq)]
pub struct Relabel {
    /// 原始路径（`<实体>/<组件>.<字段>`）。
    pub path: String,
    /// 换上的人话名。
    pub name: String,
}

/// 一个派生量声明（三种内置之一）。算出来挂进 `observation.derived[name]`。
#[derive(Debug, Clone, PartialEq)]
pub enum DerivedSpec {
    /// 两个命名实体的 Position 距离（曼哈顿或欧氏）。任一实体不存在/无 Position → 该量为 null。
    Distance {
        name: String,
        from: String,
        to: String,
        metric: DistanceMetric,
    },
    /// 字段别名：把某个 observation 叶子的值原样镜像到 `derived[name]`（方便 greedy/人直接读）。
    Alias { name: String, path: String },
    /// 计数：带某组件的存活实体数。
    Count { name: String, component: String },
}

impl DerivedSpec {
    /// 这个派生量挂进 `derived` 子对象时用的键。
    pub fn name(&self) -> &str {
        match self {
            DerivedSpec::Distance { name, .. } => name,
            DerivedSpec::Alias { name, .. } => name,
            DerivedSpec::Count { name, .. } => name,
        }
    }
}

/// 距离度量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceMetric {
    Manhattan,
    Euclidean,
}

/// greedy 的优化目标：朝某个派生量的方向走。
#[derive(Debug, Clone, PartialEq)]
pub struct GoalSpec {
    /// 目标派生量名（必须是 `observation.config.derived` 里声明过的量）。
    pub quantity: String,
    /// 优化方向。
    pub direction: GoalDirection,
}

/// 目标方向：让目标量变小（如奔出口=距离 min）还是变大（如攒资源=数量 max）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalDirection {
    Min,
    Max,
}

/// 终止事件名覆盖（缺省字段回退到 TerminalSpec::default 的对应集合）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TerminalOverride {
    pub win_events: Option<Vec<String>>,
    pub lose_events: Option<Vec<String>>,
    pub ending_prefixes: Option<Vec<String>>,
}

impl PlaytestConfig {
    /// 从 `playtest.json` 的 JSON 文档解析。`path` 只用于报错定位（vitric check 风格）。
    /// 缺字段给合理默认；非法配置返回带路径的明确错误（不静默吞）。
    pub fn parse(doc: &Value, path: &str) -> Result<PlaytestConfig, String> {
        let root = doc
            .as_object()
            .ok_or_else(|| format!("{path}: playtest.json 顶层必须是对象"))?;

        let observation = match root.get("observation") {
            Some(v) => parse_observation(v, path)?,
            None => ObservationConfig::default(),
        };
        let goal = match root.get("goal") {
            Some(Value::Null) | None => None,
            Some(v) => Some(parse_goal(v, path)?),
        };
        let terminal = match root.get("terminal") {
            Some(Value::Null) | None => None,
            Some(v) => Some(parse_terminal(v, path)?),
        };

        // 一致性校验：goal 引用的派生量必须真被声明过（否则 greedy 永远读不到目标，是配置 bug）
        if let Some(g) = &goal {
            let declared = observation.derived.iter().any(|d| d.name() == g.quantity);
            if !declared {
                return Err(format!(
                    "{path}: goal.quantity「{}」未在 observation.derived 里声明",
                    g.quantity
                ));
            }
        }

        Ok(PlaytestConfig { observation, goal, terminal })
    }

    /// 从项目根加载 `playtest.json`。不存在返回 None（=用默认 config，行为不变）；
    /// 存在但解析失败返回 Err（带路径，vitric check 风格）。
    pub fn load(project_dir: &std::path::Path) -> Result<Option<PlaytestConfig>, String> {
        let path = project_dir.join("playtest.json");
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("{}: 读取失败 {e}", path.display()))?;
        let doc: Value = serde_json::from_str(&text)
            .map_err(|e| format!("{}: JSON 解析失败 {e}", path.display()))?;
        Ok(Some(PlaytestConfig::parse(&doc, &path.display().to_string())?))
    }
}

fn parse_observation(v: &Value, path: &str) -> Result<ObservationConfig, String> {
    let obj = v
        .as_object()
        .ok_or_else(|| format!("{path}: observation 必须是对象"))?;
    let include = parse_string_array(obj.get("include"), path, "observation.include")?;
    let exclude = parse_string_array(obj.get("exclude"), path, "observation.exclude")?;

    let mut relabel = Vec::new();
    if let Some(rv) = obj.get("relabel") {
        let ro = rv
            .as_object()
            .ok_or_else(|| format!("{path}: observation.relabel 必须是对象（path→人话名）"))?;
        // BTreeMap 序遍历（serde_json Map 默认按插入序；为确定，先收集再按 key 排）
        let mut pairs: Vec<(&String, &Value)> = ro.iter().collect();
        pairs.sort_by(|a, b| a.0.cmp(b.0));
        for (k, val) in pairs {
            let name = val.as_str().ok_or_else(|| {
                format!("{path}: observation.relabel[\"{k}\"] 的值必须是字符串")
            })?;
            relabel.push(Relabel { path: k.clone(), name: name.to_string() });
        }
    }

    let mut derived = Vec::new();
    if let Some(dv) = obj.get("derived") {
        let arr = dv
            .as_array()
            .ok_or_else(|| format!("{path}: observation.derived 必须是数组"))?;
        for (i, item) in arr.iter().enumerate() {
            derived.push(parse_derived(item, path, i)?);
        }
        // 派生量名不能重名（重名会互相覆盖，是配置 bug）
        let mut seen = std::collections::BTreeSet::new();
        for d in &derived {
            if !seen.insert(d.name().to_string()) {
                return Err(format!("{path}: observation.derived 里派生量名「{}」重复", d.name()));
            }
        }
    }

    Ok(ObservationConfig { include, exclude, relabel, derived })
}

fn parse_derived(item: &Value, path: &str, idx: usize) -> Result<DerivedSpec, String> {
    let obj = item
        .as_object()
        .ok_or_else(|| format!("{path}: observation.derived[{idx}] 必须是对象"))?;
    let kind = obj
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{path}: observation.derived[{idx}] 缺 kind（distance/alias/count）"))?;
    let need_str = |key: &str| -> Result<String, String> {
        obj.get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| format!("{path}: observation.derived[{idx}].{key} 必须是字符串"))
    };
    match kind {
        "distance" => {
            let name = need_str("name")?;
            let from = need_str("from")?;
            let to = need_str("to")?;
            // metric 可缺省 → manhattan
            let metric = match obj.get("metric").and_then(|v| v.as_str()) {
                None | Some("manhattan") => DistanceMetric::Manhattan,
                Some("euclidean") => DistanceMetric::Euclidean,
                Some(other) => {
                    return Err(format!(
                        "{path}: observation.derived[{idx}].metric「{other}」只认 manhattan/euclidean"
                    ))
                }
            };
            Ok(DerivedSpec::Distance { name, from, to, metric })
        }
        "alias" => Ok(DerivedSpec::Alias { name: need_str("name")?, path: need_str("path")? }),
        "count" => {
            Ok(DerivedSpec::Count { name: need_str("name")?, component: need_str("component")? })
        }
        other => Err(format!(
            "{path}: observation.derived[{idx}].kind「{other}」只认 distance/alias/count"
        )),
    }
}

fn parse_goal(v: &Value, path: &str) -> Result<GoalSpec, String> {
    let obj = v.as_object().ok_or_else(|| format!("{path}: goal 必须是对象"))?;
    let quantity = obj
        .get("quantity")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{path}: goal.quantity 必须是字符串（指向一个派生量名）"))?
        .to_string();
    let direction = match obj.get("direction").and_then(|v| v.as_str()) {
        Some("min") => GoalDirection::Min,
        Some("max") => GoalDirection::Max,
        Some(other) => return Err(format!("{path}: goal.direction「{other}」只认 min/max")),
        None => return Err(format!("{path}: goal.direction 缺失（min/max）")),
    };
    Ok(GoalSpec { quantity, direction })
}

fn parse_terminal(v: &Value, path: &str) -> Result<TerminalOverride, String> {
    let obj = v.as_object().ok_or_else(|| format!("{path}: terminal 必须是对象"))?;
    let win_events = parse_opt_string_array(obj.get("win_events"), path, "terminal.win_events")?;
    let lose_events = parse_opt_string_array(obj.get("lose_events"), path, "terminal.lose_events")?;
    let ending_prefixes =
        parse_opt_string_array(obj.get("ending_prefixes"), path, "terminal.ending_prefixes")?;
    Ok(TerminalOverride { win_events, lose_events, ending_prefixes })
}

/// 缺省=空数组（已启用但没写就当空）。
fn parse_string_array(v: Option<&Value>, path: &str, field: &str) -> Result<Vec<String>, String> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(arr)) => arr
            .iter()
            .map(|x| {
                x.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| format!("{path}: {field} 的每一项必须是字符串"))
            })
            .collect(),
        Some(_) => Err(format!("{path}: {field} 必须是字符串数组")),
    }
}

/// 缺省=None（区分「没写这个覆盖」和「写了空数组」——None 回退到默认集合）。
fn parse_opt_string_array(
    v: Option<&Value>,
    path: &str,
    field: &str,
) -> Result<Option<Vec<String>>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(_) => Ok(Some(parse_string_array(v, path, field)?)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_config_is_empty_no_goal_no_terminal() {
        let c = PlaytestConfig::default();
        assert!(c.observation.include.is_empty());
        assert!(c.observation.exclude.is_empty());
        assert!(c.observation.relabel.is_empty());
        assert!(c.observation.derived.is_empty());
        assert!(c.goal.is_none());
        assert!(c.terminal.is_none());
    }

    #[test]
    fn parse_empty_object_yields_default() {
        let c = PlaytestConfig::parse(&json!({}), "p.json").unwrap();
        assert_eq!(c, PlaytestConfig::default());
    }

    #[test]
    fn parse_include_exclude_relabel() {
        let c = PlaytestConfig::parse(
            &json!({
                "observation": {
                    "include": ["Position", "Resources"],
                    "exclude": ["Debug"],
                    "relabel": { "hero/Position.x": "横坐标", "hero/Position.y": "纵坐标" }
                }
            }),
            "p.json",
        )
        .unwrap();
        assert_eq!(c.observation.include, vec!["Position", "Resources"]);
        assert_eq!(c.observation.exclude, vec!["Debug"]);
        // relabel 按 path 排序（确定）
        assert_eq!(c.observation.relabel.len(), 2);
        assert_eq!(c.observation.relabel[0].path, "hero/Position.x");
        assert_eq!(c.observation.relabel[0].name, "横坐标");
    }

    #[test]
    fn parse_derived_distance_with_default_metric() {
        let c = PlaytestConfig::parse(
            &json!({
                "observation": { "derived": [
                    { "kind": "distance", "name": "to_exit", "from": "hero", "to": "flag" }
                ]}
            }),
            "p.json",
        )
        .unwrap();
        assert_eq!(c.observation.derived.len(), 1);
        match &c.observation.derived[0] {
            DerivedSpec::Distance { name, from, to, metric } => {
                assert_eq!(name, "to_exit");
                assert_eq!(from, "hero");
                assert_eq!(to, "flag");
                assert_eq!(*metric, DistanceMetric::Manhattan, "metric 缺省=manhattan");
            }
            other => panic!("应解析成 Distance: {other:?}"),
        }
    }

    #[test]
    fn parse_derived_alias_and_count() {
        let c = PlaytestConfig::parse(
            &json!({
                "observation": { "derived": [
                    { "kind": "alias", "name": "gold", "path": "hero/Resources.gold" },
                    { "kind": "count", "name": "enemies", "component": "Enemy" }
                ]}
            }),
            "p.json",
        )
        .unwrap();
        assert_eq!(c.observation.derived.len(), 2);
        assert!(matches!(&c.observation.derived[0], DerivedSpec::Alias { name, path }
            if name == "gold" && path == "hero/Resources.gold"));
        assert!(matches!(&c.observation.derived[1], DerivedSpec::Count { name, component }
            if name == "enemies" && component == "Enemy"));
    }

    #[test]
    fn parse_euclidean_metric() {
        let c = PlaytestConfig::parse(
            &json!({"observation": {"derived": [
                {"kind": "distance", "name": "d", "from": "a", "to": "b", "metric": "euclidean"}
            ]}}),
            "p.json",
        )
        .unwrap();
        assert!(matches!(&c.observation.derived[0],
            DerivedSpec::Distance { metric: DistanceMetric::Euclidean, .. }));
    }

    #[test]
    fn parse_goal_min_max() {
        let c = PlaytestConfig::parse(
            &json!({
                "observation": {"derived": [{"kind": "alias", "name": "d", "path": "x/Y.z"}]},
                "goal": { "quantity": "d", "direction": "min" }
            }),
            "p.json",
        )
        .unwrap();
        let g = c.goal.unwrap();
        assert_eq!(g.quantity, "d");
        assert_eq!(g.direction, GoalDirection::Min);
    }

    #[test]
    fn parse_terminal_override() {
        let c = PlaytestConfig::parse(
            &json!({
                "terminal": { "win_events": ["reached-exit"], "lose_events": ["fell"] }
            }),
            "p.json",
        )
        .unwrap();
        let t = c.terminal.unwrap();
        assert_eq!(t.win_events, Some(vec!["reached-exit".to_string()]));
        assert_eq!(t.lose_events, Some(vec!["fell".to_string()]));
        assert_eq!(t.ending_prefixes, None, "没写的字段=None，回退默认");
    }

    // ---- 非法配置：带路径报错 ----

    #[test]
    fn reject_non_object_root() {
        let err = PlaytestConfig::parse(&json!([1, 2]), "bad.json").unwrap_err();
        assert!(err.contains("bad.json"), "错误必须带路径: {err}");
        assert!(err.contains("顶层必须是对象"));
    }

    #[test]
    fn reject_unknown_derived_kind() {
        let err = PlaytestConfig::parse(
            &json!({"observation": {"derived": [{"kind": "magic", "name": "x"}]}}),
            "bad.json",
        )
        .unwrap_err();
        assert!(err.contains("bad.json") && err.contains("magic"), "{err}");
    }

    #[test]
    fn reject_unknown_metric() {
        let err = PlaytestConfig::parse(
            &json!({"observation": {"derived": [
                {"kind": "distance", "name": "d", "from": "a", "to": "b", "metric": "taxicab"}
            ]}}),
            "bad.json",
        )
        .unwrap_err();
        assert!(err.contains("taxicab"), "{err}");
    }

    #[test]
    fn reject_goal_without_declared_quantity() {
        // goal 引用了一个没声明的派生量 → 报错（greedy 永远读不到目标）
        let err = PlaytestConfig::parse(
            &json!({"goal": {"quantity": "ghost", "direction": "min"}}),
            "bad.json",
        )
        .unwrap_err();
        assert!(err.contains("ghost") && err.contains("未在 observation.derived"), "{err}");
    }

    #[test]
    fn reject_bad_goal_direction() {
        let err = PlaytestConfig::parse(
            &json!({
                "observation": {"derived": [{"kind": "alias", "name": "d", "path": "x/Y.z"}]},
                "goal": {"quantity": "d", "direction": "sideways"}
            }),
            "bad.json",
        )
        .unwrap_err();
        assert!(err.contains("sideways"), "{err}");
    }

    #[test]
    fn reject_duplicate_derived_name() {
        let err = PlaytestConfig::parse(
            &json!({"observation": {"derived": [
                {"kind": "count", "name": "n", "component": "A"},
                {"kind": "count", "name": "n", "component": "B"}
            ]}}),
            "bad.json",
        )
        .unwrap_err();
        assert!(err.contains("重复"), "{err}");
    }

    #[test]
    fn reject_distance_missing_field() {
        let err = PlaytestConfig::parse(
            &json!({"observation": {"derived": [{"kind": "distance", "name": "d", "from": "a"}]}}),
            "bad.json",
        )
        .unwrap_err();
        assert!(err.contains("to"), "缺 to 字段应报错: {err}");
    }
}
