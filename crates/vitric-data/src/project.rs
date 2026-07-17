use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::{Scene, Schema, Sequence, ValidationReport};

/// Project manifest `vitric.json`.
///
/// ```json
/// {
///   "name": "coin-run",
///   "schema": "schema.json",
///   "entry": "scenes/main.json",
///   "scenes": ["scenes/main.json"],
///   "rules": ["rules/game.json"],
///   "scripts": ["scripts/systems.js"],
///   "seed": 42
/// }
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectManifest {
    pub name: String,
    pub schema: String,
    /// Entry scene; must appear in the scenes list.
    pub entry: String,
    #[serde(default)]
    pub scenes: Vec<String>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<String>,
    /// Sequence (timeline) definition files (optional; one sequence per file, `sequences/<name>.json`).
    /// A sequence is a generic presentation primitive, instantiated and played at runtime by the `Sequence` component; not declared = the project does not use sequences.
    #[serde(default)]
    pub sequences: Vec<String>,
    /// Animation definition file (optional).
    #[serde(default)]
    pub animations: Option<String>,
    /// Theme definition files (optional; one theme per file, `themes/<name>.json`).
    /// UI controls reference themes by name to fetch styles (check validates name existence); not declared = the project does not use themes.
    #[serde(default)]
    pub themes: Vec<String>,
    /// TTF vector font (optional; path relative to the project root, e.g. "fonts/myfont.ttf").
    /// If set, all Text components are rendered with this font (proportional spacing + anti-aliasing, supporting any
    /// glyph in the font — including CJK); if not set = the old behavior with the embedded 8x8 bitmap font is kept (output bytes unchanged).
    /// A missing file is reported at load time (VD040); a corrupt file is explicitly reported at check/startup.
    #[serde(default)]
    pub font: Option<String>,
    /// Performance budgets (optional). Exceeding them is not silent stutter; it is explicitly reported.
    #[serde(default)]
    pub budgets: Budgets,
    /// Delivery gates (optional). Declaring this is what makes `vitric gate` issue (or refuse) a clearance certificate;
    /// not declared = the project has no machine-verifiable delivery standard, and gate refuses outright (no gates, no certificate).
    #[serde(default)]
    pub gates: Option<Gates>,
    /// World random seed; same seed + same input = same result.
    #[serde(default = "default_seed")]
    pub seed: u64,
}

/// Delivery gate declaration (the manifest's `gates` field).
///
/// ```json
/// "gates": {
///   "playthroughs": [{"recording": "recordings/clear.json", "must_emit": "game-won"}],
///   "assertions": "qa/asserts.json",
///   "check": true,
///   "max_ticks": 100000
/// }
/// ```
///
/// Core constraint: a clearance recording is an **unforgeable delivery certificate** — the replay must be bit-identical at every checkpoint,
/// and the `must_emit` event must actually be observed during replay. Forge any frame and the state hash will diverge.
#[derive(Debug, Clone, Deserialize)]
pub struct Gates {
    /// Clearance recording gate. Each recording is independently replayed and verified; an empty list = no certificate can be issued, gate refuses.
    #[serde(default)]
    pub playthroughs: Vec<PlaythroughGate>,
    /// Assertion set file (relative to the project root; format `[{"id", "if": [[left,op,right]...]}, ...]`).
    /// If declared, it is fully evaluated on **every tick** of each recording's replay; any violation at any moment refuses the certificate.
    #[serde(default)]
    pub assertions: Option<String>,
    /// Whether to run the full project validation first (same as vitric check). Defaults to true — if the data isn't even legal, delivery is out of the question.
    #[serde(default = "default_true")]
    pub check: bool,
    /// Recording tick count cap (not set = unlimited). Prevents water-injection certificates of the "idle a million ticks and eventually win" variety.
    #[serde(default)]
    pub max_ticks: Option<u64>,
    /// Playtest gate (optional). If declared, `vitric gate` runs an extra playtest gate: it runs a swarm/lookahead/seed-exploration pass per this config,
    /// aggregates a report, and then checks each declared assertion one by one (clear rate / soft-lock count /
    /// unreachable endings / inert actions / numeric breakage). Not declared = this gate is not run (existing gate behavior unchanged).
    #[serde(default)]
    pub playtest: Option<PlaytestGate>,
}

/// Playtest gate declaration (the manifest's `gates.playtest` field).
///
/// Turns "auto-clearing the floor" into a delivery contract: the project declares the playtest threshold it must meet (how many sessions, whether it can be cleared,
/// soft-lock cap, etc.), and `vitric gate` actually runs a playtest swarm and asserts the threshold is met before letting it through. The playtest swarm is
/// deterministic (same seed + same input = same result), so this gate is reproducible.
///
/// ```json
/// "playtest": {
///   "sessions": 16,
///   "max_ticks": 600,
///   "require_clearable": true,
///   "max_soft_locks": 0
/// }
/// ```
///
/// Run-mode fields (sessions/max_ticks/strategy/horizon/seed_recording) decide how to run; assertion fields
/// (require_clearable/min_clear_rate/max_soft_locks/...) are all optional, **checked only if filled in** — dimensions left blank
/// do not participate in the verdict; only the contracts you care about are written into the manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct PlaytestGate {
    /// How many sessions to run (default 16). When strategy=lookahead, this is how many lookahead sessions to run.
    #[serde(default = "default_sessions")]
    pub sessions: usize,
    /// Per-session tick cap (default 600).
    #[serde(default = "default_pt_max_ticks")]
    pub max_ticks: u64,
    /// Run-mode strategy (blank = default strategy group swarm rotating four strategies; can be set to "lookahead" to run a lookahead search for sessions sessions).
    #[serde(default)]
    pub strategy: Option<String>,
    /// **Depth** of the lookahead beam search (how many frames to plan ahead; only used when strategy=lookahead, default 8, 1 = single-step lookahead).
    /// The field name keeps `horizon` for backward compatibility with old manifests (the semantics have been folded into "search depth").
    #[serde(default = "default_horizon")]
    pub horizon: u64,
    /// **Beam width** of the lookahead beam search (how many optimal nodes to keep per layer for expansion; only used when strategy=lookahead, default 4).
    #[serde(default = "default_beam")]
    pub beam: usize,
    /// Seed recording (relative to the project root). If filled in, runs seed-style exploration: perturbs this recording as a baseline into sessions variant runs.
    #[serde(default)]
    pub seed_recording: Option<String>,

    // ---- Assertions (all optional, checked only if filled in; blank dimensions do not participate in the verdict) ----
    /// true = clear rate must be > 0 (swarm clears at least once).
    #[serde(default)]
    pub require_clearable: Option<bool>,
    /// Clear rate lower bound (0..1). If the actual clear rate < this value, fail.
    #[serde(default)]
    pub min_clear_rate: Option<f64>,
    /// Upper bound on the number of soft-lock clusters (stuck_clusters). Exceeding it fails.
    #[serde(default)]
    pub max_soft_locks: Option<usize>,
    /// Upper bound on the number of unreachable endings (ending_coverage.unreachable_endings). Exceeding it fails.
    #[serde(default)]
    pub max_unreachable_endings: Option<usize>,
    /// Upper bound on the number of inert actions (inert_actions). Exceeding it fails.
    #[serde(default)]
    pub max_inert_actions: Option<usize>,
    /// true = numeric breakage (runaway/collapse/non_finite) must all be empty; any non-empty one fails.
    #[serde(default)]
    pub forbid_numeric_breakage: Option<bool>,
}

fn default_sessions() -> usize {
    16
}

fn default_pt_max_ticks() -> u64 {
    600
}

fn default_horizon() -> u64 {
    8
}

fn default_beam() -> usize {
    4
}

/// One clearance recording gate.
#[derive(Debug, Clone, Deserialize)]
pub struct PlaythroughGate {
    /// Recording file (relative to the project root), produced by `vitric run --record`.
    pub recording: String,
    /// Event that must be observed during replay (the end-game signal). Defaults to "game-won".
    #[serde(default = "default_must_emit")]
    pub must_emit: String,
}

fn default_true() -> bool {
    true
}

fn default_must_emit() -> String {
    "game-won".to_string()
}

/// Performance budgets. 0 = unlimited.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Budgets {
    /// Upper bound on the number of live entities.
    #[serde(default)]
    pub max_entities: u64,
    /// Upper bound on the number of events per tick (event-storm detection).
    #[serde(default)]
    pub max_events_per_tick: u64,
}

/// One animation clip: a frame-image sequence + playback rate.
///
/// ```json
/// { "clips": { "coin-spin": { "frames": ["coin1.png", "coin2.png"], "fps": 8, "loop": true } } }
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct Clip {
    /// Frame images (paths in the asset repository).
    pub frames: Vec<String>,
    pub fps: u32,
    /// true = loop playback; false = stop on the last frame when done and emit an anim-finished event.
    #[serde(default, rename = "loop")]
    pub looping: bool,
}

fn default_seed() -> u64 {
    0
}

/// Theme name = file name with the directory and `.json` suffix stripped (`themes/dark.json` -> `dark`).
/// Controls reference themes by this name.
fn theme_name(rel: &str) -> String {
    rel.rsplit('/')
        .next()
        .unwrap_or(rel)
        .strip_suffix(".json")
        .unwrap_or(rel)
        .to_string()
}

/// A fully loaded project: manifest + schema + all scenes (validated) + raw rules/scripts.
///
/// Semantic validation of rules happens in vitric-rules (it knows the structure of triggers/actions);
/// here we only guarantee the JSON can be parsed — responsibilities are kept separate.
#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: ProjectManifest,
    pub schema: Schema,
    /// Relative path -> scene
    pub scenes: BTreeMap<String, Scene>,
    /// (relative path, rule document)
    pub rules: Vec<(String, Value)>,
    /// (relative path, script source)
    pub scripts: Vec<(String, String)>,
    /// Sequences (name -> validated static track).
    pub sequences: BTreeMap<String, Sequence>,
    /// Animation clips (name -> definition).
    pub animations: BTreeMap<String, Clip>,
    /// Themes (name -> validated style roll). Assembly-time constants; do not enter world state.
    pub themes: BTreeMap<String, crate::Theme>,
}

impl Project {
    /// Load the entire project from a directory. All problems (IO / parse / validation) are aggregated into one report and given all at once.
    pub fn load(root: impl AsRef<Path>) -> Result<Project, ValidationReport> {
        let root = root.as_ref().to_path_buf();
        let mut report = ValidationReport::default();

        // Manifest
        let manifest_path = root.join("vitric.json");
        let manifest_doc = match read_json(&manifest_path) {
            Ok(v) => v,
            Err(e) => {
                report.push("VD040", "vitric.json", e, "项目根目录必须有 vitric.json 清单");
                return Err(report);
            }
        };
        let manifest: ProjectManifest = match serde_json::from_value(manifest_doc) {
            Ok(m) => m,
            Err(e) => {
                report.push(
                    "VD041",
                    "vitric.json",
                    format!("清单解析失败: {e}"),
                    "必填字段: name(文本)、schema(路径)、entry(路径)。可选: scenes/rules/scripts(路径数组)、font(TTF 路径)、seed(整数)",
                );
                return Err(report);
            }
        };
        if !manifest.scenes.contains(&manifest.entry) {
            report.push(
                "VD042",
                "vitric.json#/entry",
                format!("入口场景 {:?} 不在 scenes 列表里", manifest.entry),
                "把它加进 scenes 数组",
            );
        }

        // schema
        let schema = match read_json(&root.join(&manifest.schema)) {
            Ok(doc) => match Schema::parse(&doc, &manifest.schema) {
                Ok(s) => s,
                Err(r) => {
                    report.merge(r);
                    Schema::default()
                }
            },
            Err(e) => {
                report.push("VD040", &manifest.schema, e, "清单 schema 字段指向的文件必须存在");
                Schema::default()
            }
        };

        // Scenes
        let mut scenes = BTreeMap::new();
        for rel in &manifest.scenes {
            match read_json(&root.join(rel)) {
                Ok(doc) => match Scene::parse(doc, rel, &schema) {
                    Ok(s) => {
                        scenes.insert(rel.clone(), s);
                    }
                    Err(r) => report.merge(r),
                },
                Err(e) => report.push("VD040", rel, e, "清单 scenes 列表里的文件必须存在"),
            }
        }

        // Rules (only parse JSON; semantic validation belongs to vitric-rules)
        let mut rules = Vec::new();
        for rel in &manifest.rules {
            match read_json(&root.join(rel)) {
                Ok(doc) => rules.push((rel.clone(), doc)),
                Err(e) => report.push("VD040", rel, e, "清单 rules 列表里的文件必须存在"),
            }
        }

        // Scripts (raw source; execution belongs to vitric-script)
        let mut scripts = Vec::new();
        for rel in &manifest.scripts {
            match fs::read_to_string(root.join(rel)) {
                Ok(src) => scripts.push((rel.clone(), src)),
                Err(e) => report.push("VD040", rel, format!("读取失败: {e}"), "清单 scripts 列表里的文件必须存在"),
            }
        }

        // Sequences (timelines): one per file, validated against the schema (action names / fields / at monotonicity etc.).
        // Sequence name conflicts are reported explicitly — the runtime Sequence component references by name, and duplicates cannot be disambiguated.
        let mut sequences = BTreeMap::new();
        for rel in &manifest.sequences {
            match read_json(&root.join(rel)) {
                Ok(doc) => match Sequence::parse(&doc, rel, &schema) {
                    Ok(seq) => {
                        if sequences.contains_key(&seq.id) {
                            report.push(
                                "VD066",
                                format!("{rel}#/id"),
                                format!("序列名 {:?} 重复", seq.id),
                                "序列名（默认取文件名）在项目内必须唯一——Sequence 组件按名字引用",
                            );
                        }
                        sequences.insert(seq.id.clone(), seq);
                    }
                    Err(r) => report.merge(r),
                },
                Err(e) => report.push("VD040", rel, e, "清单 sequences 列表里的文件必须存在"),
            }
        }

        // Font: only check file existence (parsing / corruption validation is in vitric-render's FontStore::load,
        // which knows TTF; here, like scenes/rules, we only care that "the file the manifest points to must exist")
        if let Some(rel) = &manifest.font {
            if !root.join(rel).is_file() {
                report.push(
                    "VD040",
                    rel.as_str(),
                    "字体文件不存在".to_string(),
                    "清单 font 字段指向的 TTF 文件必须存在（路径相对项目根目录）",
                );
            }
        }

        // Animations
        let mut animations = BTreeMap::new();
        if let Some(rel) = &manifest.animations {
            match read_json(&root.join(rel)) {
                Ok(doc) => parse_animations(&doc, rel, &mut animations, &mut report),
                Err(e) => report.push("VD040", rel, e, "清单 animations 字段指向的文件必须存在"),
            }
        }

        // Themes: one per file, name taken from the file name (stripping the themes/ prefix and .json suffix).
        // Duplicate names are reported explicitly — controls reference by name, and duplicates cannot be disambiguated (same scope as sequences).
        let mut themes = BTreeMap::new();
        for rel in &manifest.themes {
            let name = theme_name(rel);
            match read_json(&root.join(rel)) {
                Ok(doc) => {
                    if themes.contains_key(&name) {
                        report.push(
                            "VD084",
                            rel.as_str(),
                            format!("主题名 {name:?} 重复"),
                            "主题名（取文件名）在项目内必须唯一——控件按名字引用",
                        );
                    }
                    let theme = crate::Theme::parse(&doc, &name, rel, &mut report);
                    themes.insert(name, theme);
                }
                Err(e) => report.push("VD040", rel, e, "清单 themes 列表里的文件必须存在"),
            }
        }

        report.into_result(Project { root, manifest, schema, scenes, rules, scripts, sequences, animations, themes })
    }

    pub fn entry_scene(&self) -> &Scene {
        &self.scenes[&self.manifest.entry]
    }
}

fn parse_animations(
    doc: &Value,
    file: &str,
    out: &mut BTreeMap<String, Clip>,
    report: &mut ValidationReport,
) {
    let Some(clips) = doc.get("clips").and_then(|v| v.as_object()) else {
        report.push(
            "VD050",
            format!("{file}#/clips"),
            "动画文件缺少 clips 对象",
            "顶层结构: {\"clips\": {\"片段名\": {\"frames\": [\"图.png\"], \"fps\": 8, \"loop\": true}}}",
        );
        return;
    };
    for (name, cdoc) in clips {
        let cpath = format!("{file}#/clips/{name}");
        let clip: Clip = match serde_json::from_value(cdoc.clone()) {
            Ok(c) => c,
            Err(e) => {
                report.push(
                    "VD051",
                    &cpath,
                    format!("片段解析失败: {e}"),
                    "片段写法: {\"frames\": [\"图.png\", ...], \"fps\": 8, \"loop\": true}",
                );
                continue;
            }
        };
        if clip.frames.is_empty() {
            report.push("VD052", format!("{cpath}/frames"), "frames 不能为空", "至少一帧");
            continue;
        }
        if clip.fps == 0 {
            report.push("VD053", format!("{cpath}/fps"), "fps 必须 > 0", "常用 4-12");
            continue;
        }
        out.insert(name.clone(), clip);
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("读取失败: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("JSON 解析失败（第 {} 行第 {} 列）: {e}", e.line(), e.column()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn temp_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vitric-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_full_project() {
        let dir = temp_project("load");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"],"seed":7}"#,
        );
        write(
            &dir.join("schema.json"),
            r#"{"components":{"Position":{"fields":{"x":{"type":"number"},"y":{"type":"number"}}}}}"#,
        );
        write(
            &dir.join("scenes/main.json"),
            r#"{"entities":[{"name":"player","components":{"Position":{"x":1,"y":2}}}]}"#,
        );
        let p = Project::load(&dir).unwrap();
        assert_eq!(p.manifest.name, "demo");
        assert_eq!(p.manifest.seed, 7);
        assert!(p.entry_scene().doc.get("entities").is_some());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_font_file_is_an_explicit_error_naming_the_path() {
        let dir = temp_project("font");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"],"font":"fonts/ghost.ttf"}"#,
        );
        write(
            &dir.join("schema.json"),
            r#"{"components":{"Position":{"fields":{"x":{"type":"number"},"y":{"type":"number"}}}}}"#,
        );
        write(&dir.join("scenes/main.json"), r#"{"entities":[]}"#);
        let err = Project::load(&dir).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("VD040") && text.contains("fonts/ghost.ttf"), "{text}");
        // Not writing the font field = legal (old bitmap-font behavior)
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"]}"#,
        );
        let p = Project::load(&dir).unwrap();
        assert!(p.manifest.font.is_none());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn all_problems_reported_in_one_pass() {
        let dir = temp_project("problems");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/missing.json",
                "scenes":["scenes/bad.json"],"rules":["rules/none.json"]}"#,
        );
        write(
            &dir.join("schema.json"),
            r#"{"components":{"P":{"fields":{"x":{"type":"number"}}}}}"#,
        );
        write(&dir.join("scenes/bad.json"), r#"{"entities":[{"components":{"Nope":{}}}]}"#);
        let err = Project::load(&dir).unwrap_err();
        let codes: Vec<&str> = err.errors.iter().map(|e| e.code).collect();
        assert!(codes.contains(&"VD042"), "入口不在列表: {err}");
        assert!(codes.contains(&"VD005"), "未知组件: {err}");
        assert!(codes.contains(&"VD040"), "规则文件缺失: {err}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
