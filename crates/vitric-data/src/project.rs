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
    /// Reusable modules to include (paths relative to the project root, each pointing at a directory with a `module.json`).
    /// Each module contributes: a schema fragment (merged field-by-field with conflict detection),
    /// rule files (appended to `rules`), and script files (appended to `scripts`).
    /// Nested includes are supported (a module may itself include other modules); cycles are detected and reported (VD093).
    #[serde(default)]
    pub includes: Vec<String>,
}

/// Module manifest `module.json` — the slim descriptor a reusable module ships alongside its schema/rules/scripts.
///
/// All paths inside are relative to the module directory. `name` is informational (for error messages and future tooling);
/// `schema` / `rules` / `scripts` / `includes` are all optional — a module contributes only what it declares.
///
/// ```json
/// {
///   "name": "inventory",
///   "schema": "schema.json",
///   "rules": ["rules/inventory.json"],
///   "scripts": ["scripts/inventory.js"]
/// }
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ModuleManifest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<String>,
    /// Nested includes (paths relative to this module's directory). Cycles are detected (VD093).
    #[serde(default)]
    pub includes: Vec<String>,
    /// Domain-specific test assertions shipped with the module (path relative to the module directory).
    /// When a project includes this module, these assertions are available for `vitric playtest` / `vitric gate`
    /// to verify module-specific invariants (e.g. inventory counts non-negative, HP never below zero).
    #[serde(default)]
    pub test_assertions: Option<String>,
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
        let mut manifest: ProjectManifest = match serde_json::from_value(manifest_doc) {
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
        let mut schema = match read_json(&root.join(&manifest.schema)) {
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

        // Includes: merge module schema fragments into `schema`, append module rules/scripts to the manifest.
        // Must happen BEFORE scenes load (scenes are validated against the merged schema).
        // Tracks visited module directories (by canonicalized path) to detect cycles (VD093).
        // `take` the includes list to avoid borrowing manifest while mutating it.
        let includes = std::mem::take(&mut manifest.includes);
        if !includes.is_empty() {
            let mut visited: Vec<PathBuf> = Vec::new();
            process_includes(
                &includes,
                "",
                &root,
                &mut schema,
                &mut manifest,
                &mut report,
                &mut visited,
            );
        }

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

/// Join a relative-include path with the current module directory (both relative to the project root).
/// Empty `cur_dir_rel` means the project root is the current directory.
fn join_rel(cur_dir_rel: &str, rel: &str) -> String {
    if cur_dir_rel.is_empty() {
        rel.to_string()
    } else {
        format!("{cur_dir_rel}/{rel}")
    }
}

/// Process `includes`: for each module, merge its schema fragment, append its rules/scripts, and recurse into nested includes.
///
/// - `includes`: the list of include paths (relative to `cur_dir_rel`).
/// - `cur_dir_rel`: the directory of the manifest currently being processed, relative to the project root
///   ("" for the project root itself; "../../modules/inventory" for a module two levels up).
/// - `root`: the project root directory.
/// - `schema`: the merged schema so far (mutated in place).
/// - `manifest`: the project manifest (its `rules`/`scripts` lists are extended with module-relative paths).
/// - `visited`: canonicalized module directory paths, for cycle detection (VD093).
fn process_includes(
    includes: &[String],
    cur_dir_rel: &str,
    root: &Path,
    schema: &mut Schema,
    manifest: &mut ProjectManifest,
    report: &mut ValidationReport,
    visited: &mut Vec<PathBuf>,
) {
    for inc in includes {
        let module_dir_rel = join_rel(cur_dir_rel, inc);
        let module_dir_abs = root.join(&module_dir_rel);

        // Cycle detection: canonicalize the module directory and check against visited.
        // canonicalize fails for nonexistent paths — that's reported below as VD090.
        if let Ok(canon) = module_dir_abs.canonicalize() {
            if visited.contains(&canon) {
                report.push(
                    "VD093",
                    format!("vitric.json#/includes (via {module_dir_rel})"),
                    format!("检测到 include 循环：模块 {module_dir_rel:?} 已被引用过"),
                    "移除循环引用（A includes B includes A 是非法的）",
                );
                continue;
            }
            visited.push(canon);
        }

        // Load module.json
        let module_manifest_path = format!("{module_dir_rel}/module.json");
        let module_doc = match read_json(&root.join(&module_manifest_path)) {
            Ok(v) => v,
            Err(e) => {
                report.push(
                    "VD090",
                    &module_manifest_path,
                    e,
                    "includes 指向的目录必须包含 module.json 清单",
                );
                continue;
            }
        };
        let module_manifest: ModuleManifest = match serde_json::from_value(module_doc) {
            Ok(m) => m,
            Err(e) => {
                report.push(
                    "VD091",
                    &module_manifest_path,
                    format!("模块清单解析失败: {e}"),
                    "必填: 可选字段 name(文本)、schema(路径)、rules/scripts(路径数组)、includes(路径数组)",
                );
                continue;
            }
        };

        // Merge module schema fragment
        if let Some(schema_rel) = &module_manifest.schema {
            let schema_path_rel = join_rel(&module_dir_rel, schema_rel);
            match read_json(&root.join(&schema_path_rel)) {
                Ok(doc) => match Schema::parse(&doc, &schema_path_rel) {
                    Ok(module_schema) => schema.merge(module_schema, &schema_path_rel, report),
                    Err(r) => report.merge(r),
                },
                Err(e) => {
                    report.push(
                        "VD040",
                        &schema_path_rel,
                        e,
                        "module.json 的 schema 字段指向的文件必须存在",
                    );
                }
            }
        }

        // Append module rules/scripts (paths become project-root-relative via module_dir_rel)
        for r in &module_manifest.rules {
            manifest.rules.push(join_rel(&module_dir_rel, r));
        }
        for s in &module_manifest.scripts {
            manifest.scripts.push(join_rel(&module_dir_rel, s));
        }

        // Recurse into nested includes
        if !module_manifest.includes.is_empty() {
            process_includes(
                &module_manifest.includes,
                &module_dir_rel,
                root,
                schema,
                manifest,
                report,
                visited,
            );
        }
    }
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

    // ---- includes mechanism ----

    fn write_module(dir: &Path, mod_rel: &str, schema_json: &str, rules: &[&str], scripts: &[&str]) {
        let mod_dir = dir.join(mod_rel);
        let rules_str = format!(
            "[{}]",
            rules.iter().map(|r| format!("\"{}\"", r)).collect::<Vec<_>>().join(",")
        );
        let scripts_str = format!(
            "[{}]",
            scripts.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(",")
        );
        let module_json = format!(
            r#"{{"name":"{}","schema":"schema.json","rules":{},"scripts":{}}}"#,
            mod_rel.replace('/', "-"),
            if rules.is_empty() { "[]".to_string() } else { rules_str },
            if scripts.is_empty() { "[]".to_string() } else { scripts_str },
        );
        write(&mod_dir.join("module.json"), &module_json);
        write(&mod_dir.join("schema.json"), schema_json);
    }

    #[test]
    fn includes_merges_schema_and_appends_rules_scripts() {
        let dir = temp_project("inc-merge");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"],"includes":["mods/inventory"]}"#,
        );
        // Project schema: Position + the Inventory component (proves field-level merge works —
        // the project declares `capacity` as int, the module also declares it as int → no conflict).
        write(
            &dir.join("schema.json"),
            r#"{"components":{
                "Position":{"fields":{"x":{"type":"number"},"y":{"type":"number"}}},
                "Inventory":{"fields":{"capacity":{"type":"int","default":16,"min":1}}}
            }}"#,
        );
        write(
            &dir.join("scenes/main.json"),
            r#"{"entities":[{"name":"player","components":{
                "Position":{"x":0,"y":0},
                "Inventory":{"items":[],"counts":[],"capacity":8}
            }}]}"#,
        );
        // Module: contributes Inventory.items/counts (new fields, merged in) + a rule file + a script file
        write_module(
            &dir,
            "mods/inventory",
            r#"{"components":{"Inventory":{"fields":{
                "items":{"type":"list","of":{"type":"text"},"default":[]},
                "counts":{"type":"list","of":{"type":"int"},"default":[]}
            }}}}"#,
            &["rules/inventory.json"],
            &["scripts/inventory.js"],
        );
        write(&dir.join("mods/inventory/rules/inventory.json"), r#"{"rules":[]}"#);
        write(&dir.join("mods/inventory/scripts/inventory.js"), "// inventory module system\n");

        let p = Project::load(&dir).unwrap();
        // Schema merged: Inventory has all three fields
        let inv = p.schema.component("Inventory").unwrap();
        assert!(inv.fields.contains_key("capacity"), "项目原字段保留");
        assert!(inv.fields.contains_key("items"), "模块新增字段合并进来");
        assert!(inv.fields.contains_key("counts"), "模块新增字段合并进来");
        // Rules/scripts appended with module-relative paths
        assert!(
            p.manifest.rules.iter().any(|r| r == "mods/inventory/rules/inventory.json"),
            "rules 列表含模块规则: {:?}",
            p.manifest.rules
        );
        assert!(
            p.manifest.scripts.iter().any(|s| s == "mods/inventory/scripts/inventory.js"),
            "scripts 列表含模块脚本: {:?}",
            p.manifest.scripts
        );
        // The merged schema actually validates the scene (Inventory.items/counts are now legal)
        assert!(p.scenes.contains_key("scenes/main.json"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn includes_field_type_conflict_reports_vd092() {
        let dir = temp_project("inc-conflict");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"],"includes":["mods/bad"]}"#,
        );
        // Project says Inventory.capacity is int; module says it's text → VD092
        write(
            &dir.join("schema.json"),
            r#"{"components":{"Inventory":{"fields":{"capacity":{"type":"int","default":16}}}}}"#,
        );
        write(&dir.join("scenes/main.json"), r#"{"entities":[]}"#);
        write_module(
            &dir,
            "mods/bad",
            r#"{"components":{"Inventory":{"fields":{"capacity":{"type":"text","default":""}}}}}"#,
            &[],
            &[],
        );
        let err = Project::load(&dir).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("VD092"), "要有字段冲突错误码: {text}");
        assert!(text.contains("capacity"), "要点名冲突字段: {text}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn includes_missing_module_json_reports_vd090() {
        let dir = temp_project("inc-missing");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"],"includes":["mods/ghost"]}"#,
        );
        write(
            &dir.join("schema.json"),
            r#"{"components":{"P":{"fields":{"x":{"type":"number"}}}}}"#,
        );
        write(&dir.join("scenes/main.json"), r#"{"entities":[]}"#);
        // mods/ghost directory exists but has no module.json
        fs::create_dir_all(dir.join("mods/ghost")).unwrap();
        let err = Project::load(&dir).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("VD090"), "要有模块缺失错误码: {text}");
        assert!(text.contains("mods/ghost/module.json"), "要点名缺失文件: {text}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn includes_cycle_reports_vd093() {
        let dir = temp_project("inc-cycle");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"],"includes":["mods/a"]}"#,
        );
        write(&dir.join("schema.json"), r#"{"components":{"P":{"fields":{"x":{"type":"number"}}}}}"#);
        write(&dir.join("scenes/main.json"), r#"{"entities":[]}"#);
        // A includes B, B includes A → cycle
        write(&dir.join("mods/a/module.json"), r#"{"name":"a","includes":["../b"]}"#);
        write(&dir.join("mods/a/schema.json"), r#"{"components":{}}"#);
        write(&dir.join("mods/b/module.json"), r#"{"name":"b","includes":["../a"]}"#);
        write(&dir.join("mods/b/schema.json"), r#"{"components":{}}"#);
        let err = Project::load(&dir).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("VD093"), "要有循环引用错误码: {text}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nested_includes_merge_transitively() {
        let dir = temp_project("inc-nested");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"],"includes":["mods/a"]}"#,
        );
        write(&dir.join("schema.json"), r#"{"components":{}}"#);
        write(&dir.join("scenes/main.json"), r#"{"entities":[]}"#);
        // A contributes component A_comp, includes B; B contributes B_comp
        write(
            &dir.join("mods/a/module.json"),
            r#"{"name":"a","schema":"schema.json","includes":["../b"]}"#,
        );
        write(
            &dir.join("mods/a/schema.json"),
            r#"{"components":{"A_comp":{"fields":{"a":{"type":"int","default":0}}}}}"#,
        );
        write(&dir.join("mods/b/module.json"), r#"{"name":"b","schema":"schema.json"}"#);
        write(
            &dir.join("mods/b/schema.json"),
            r#"{"components":{"B_comp":{"fields":{"b":{"type":"text","default":""}}}}}"#,
        );
        let p = Project::load(&dir).unwrap();
        assert!(p.schema.component("A_comp").is_some(), "A 的组件合并进来");
        assert!(p.schema.component("B_comp").is_some(), "B 的组件通过嵌套 include 也合并进来");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn idempotent_include_same_field_same_type_no_error() {
        // A field declared identically in both project and module is not a conflict (idempotent merge).
        let dir = temp_project("inc-idem");
        write(
            &dir.join("vitric.json"),
            r#"{"name":"demo","schema":"schema.json","entry":"scenes/main.json",
                "scenes":["scenes/main.json"],"includes":["mods/dup"]}"#,
        );
        write(
            &dir.join("schema.json"),
            r#"{"components":{"Inventory":{"fields":{"capacity":{"type":"int","default":16,"min":1}}}}}"#,
        );
        write(&dir.join("scenes/main.json"), r#"{"entities":[]}"#);
        write_module(
            &dir,
            "mods/dup",
            r#"{"components":{"Inventory":{"fields":{"capacity":{"type":"int","default":16,"min":1}}}}}"#,
            &[],
            &[],
        );
        let p = Project::load(&dir).unwrap();
        assert!(p.schema.component("Inventory").unwrap().fields.contains_key("capacity"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
