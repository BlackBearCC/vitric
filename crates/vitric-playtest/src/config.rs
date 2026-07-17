//! Per-game playtest view overrides (design draft section 1 "auto-derive + optional overrides", section 11 item 6).
//!
//! By default Scene View auto-derives a working view from the schema/rules (any game is playtestable out of the box). A game may optionally
//! place a `playtest.json` at the project root to polish the view: pick relevant fields, relabel with human-readable names, declare derived
//! quantities (e.g. "Manhattan distance to the exit"), give greedy an optimization target, or override terminal event names.
//!
//! **Overrides are polish, not a prerequisite**: without `playtest.json`, [`PlaytestConfig::default`] keeps all behavior
//! byte-identical to stages 1~5 (backward compatibility is locked by tests). This module only parses + validates config into structs;
//! how it gets applied to projection/strategy/termination is in [`crate::scene_view`] and [`crate::strategy`].
//!
//! **No DSL for derived quantities**: only three built-in quantities are supported (distance/alias/count), deliberately no expression engine —
//! complex derivations are left to the game to compute as a field in its own rules; here we only do lightweight declarations of "viewing existing data from another angle".

use serde_json::Value;

/// A per-game playtest config. All fields are optional (omitting falls back to the auto-derived default behavior).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaytestConfig {
    /// Observation projection overrides (pick components / rename / derived quantities).
    pub observation: ObservationConfig,
    /// greedy's optimization target (None = no target, greedy degrades to reproducible random, consistent with stage 1).
    pub goal: Option<GoalSpec>,
    /// Terminal event name overrides (None = use TerminalSpec::default).
    pub terminal: Option<TerminalOverride>,
}

/// Observation projection overrides.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObservationConfig {
    /// Whitelist: only keep these component names (when non-empty, overrides the default decoration stripping — anything outside the whitelist is stripped).
    /// Empty = whitelist disabled, falls back to the default "strip decorations, keep the rest".
    pub include: Vec<String>,
    /// Blacklist: additionally strip these component names (stacked on top of the default decoration stripping).
    pub exclude: Vec<String>,
    /// Field renaming: a path in observation → a human-readable label. The path looks like `<entity>/<component>.<field>`
    /// (same key style as numeric telemetry); on hit, that leaf's key is replaced with the human-readable name (value and structural level are unchanged).
    pub relabel: Vec<Relabel>,
    /// Derived quantity declarations: computed into a `"derived"` sub-object of observation (pure projection, not hashed).
    pub derived: Vec<DerivedSpec>,
}

/// One field rename.
#[derive(Debug, Clone, PartialEq)]
pub struct Relabel {
    /// Original path (`<entity>/<component>.<field>`).
    pub path: String,
    /// The human-readable name to apply.
    pub name: String,
}

/// One derived quantity declaration (one of the three built-ins). Computed and attached to `observation.derived[name]`.
#[derive(Debug, Clone, PartialEq)]
pub enum DerivedSpec {
    /// Distance between the Positions of two named entities (Manhattan or Euclidean). If either entity is missing or has no Position → the quantity is null.
    Distance {
        name: String,
        from: String,
        to: String,
        metric: DistanceMetric,
    },
    /// Field alias: mirrors the value of some observation leaf verbatim into `derived[name]` (for greedy/humans to read directly).
    Alias { name: String, path: String },
    /// Count: number of live entities with a given component.
    Count { name: String, component: String },
}

impl DerivedSpec {
    /// The key used when this derived quantity is attached to the `derived` sub-object.
    pub fn name(&self) -> &str {
        match self {
            DerivedSpec::Distance { name, .. } => name,
            DerivedSpec::Alias { name, .. } => name,
            DerivedSpec::Count { name, .. } => name,
        }
    }
}

/// Distance metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DistanceMetric {
    Manhattan,
    Euclidean,
}

/// greedy's optimization target: move toward/against a derived quantity.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalSpec {
    /// Target derived quantity name (must be a quantity declared in `observation.config.derived`).
    pub quantity: String,
    /// Optimization direction.
    pub direction: GoalDirection,
}

/// Target direction: make the quantity smaller (e.g. head for the exit = distance min) or larger (e.g. hoard resources = count max).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalDirection {
    Min,
    Max,
}

/// Terminal event name overrides (omitted fields fall back to the corresponding set in TerminalSpec::default).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TerminalOverride {
    pub win_events: Option<Vec<String>>,
    pub lose_events: Option<Vec<String>>,
    pub ending_prefixes: Option<Vec<String>>,
}

impl PlaytestConfig {
    /// Parse from the JSON document of `playtest.json`. `path` is only used for error location (vitric check style).
    /// Missing fields get reasonable defaults; invalid config returns a clear error with the path (not silently swallowed).
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

        // Consistency check: the derived quantity referenced by goal must actually be declared (otherwise greedy can never read the target — a config bug)
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

    /// Load `playtest.json` from the project root. Returns None if it does not exist (= use the default config, behavior unchanged);
    /// returns Err if it exists but parsing fails (with the path, vitric check style).
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
        // Iterate in BTreeMap order (serde_json Map defaults to insertion order; for determinism, collect first then sort by key)
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
        // Derived quantity names must not collide (collisions would overwrite each other — a config bug)
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
            // metric is optional → defaults to manhattan
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

/// Default = empty array (enabled but omitted is treated as empty).
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

/// Default = None (distinguishes "this override was not written" from "an empty array was written" — None falls back to the default set).
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
        // relabel sorted by path (deterministic)
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

    // ---- Invalid config: error with path ----

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
        // goal references an undeclared derived quantity → error (greedy can never read the target)
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
