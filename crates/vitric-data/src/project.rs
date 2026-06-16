use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::{Scene, Schema, Sequence, ValidationReport};

/// 项目清单 `vitric.json`。
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
    /// 启动场景，必须出现在 scenes 列表里。
    pub entry: String,
    #[serde(default)]
    pub scenes: Vec<String>,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub scripts: Vec<String>,
    /// 序列（时间轴）定义文件（可选，每个文件一条序列，`sequences/<名>.json`）。
    /// 序列是通用演出原语，运行时由 `Sequence` 组件实例化播放；不声明 = 项目不用序列。
    #[serde(default)]
    pub sequences: Vec<String>,
    /// 动画定义文件（可选）。
    #[serde(default)]
    pub animations: Option<String>,
    /// 主题定义文件（可选，每个文件一份主题，`themes/<名>.json`）。
    /// UI 控件按名字引用主题取样式（check 校验名存在）；不声明 = 项目不用主题。
    #[serde(default)]
    pub themes: Vec<String>,
    /// TTF 矢量字体（可选，路径相对项目根目录，如 "fonts/myfont.ttf"）。
    /// 设了它，所有 Text 组件改用该字体渲染（比例字距 + 抗锯齿，支持字体里有的
    /// 任意字形——含 CJK）；不设 = 维持内嵌 8x8 点阵字体的旧行为（输出字节不变）。
    /// 文件不存在在加载期报错（VD040）；文件损坏在 check/启动时显式报错。
    #[serde(default)]
    pub font: Option<String>,
    /// 性能预算（可选）。超了不是默默卡顿，是显式上报。
    #[serde(default)]
    pub budgets: Budgets,
    /// 交付门禁（可选）。声明了它，`vitric gate` 才会出（或拒发）通关证书；
    /// 不声明 = 项目没有机器可验的交付标准，gate 直接拒绝（无门禁不出证书）。
    #[serde(default)]
    pub gates: Option<Gates>,
    /// 世界随机种子；同种子同输入 = 同结果。
    #[serde(default = "default_seed")]
    pub seed: u64,
}

/// 交付门禁声明（清单 `gates` 字段）。
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
/// 核心约束：通关录像是**不可伪造的交付证书**——重放必须逐校验点逐位一致，
/// 且重放过程中真的观测到 `must_emit` 事件。伪造任何一帧，状态哈希必然跑偏。
#[derive(Debug, Clone, Deserialize)]
pub struct Gates {
    /// 通关录像门。每条录像独立重放验证；列表为空 = 没有证书可发，gate 拒绝。
    #[serde(default)]
    pub playthroughs: Vec<PlaythroughGate>,
    /// 断言集文件（相对项目根，格式 `[{"id", "if": [[左,op,右]...]}, ...]`）。
    /// 声明了就在每条录像重放的**每个 tick** 全量求值，任何一刻违反都拒发证书。
    #[serde(default)]
    pub assertions: Option<String>,
    /// 是否先过完整项目校验（vitric check 同款）。默认 true——数据都不合法谈不上交付。
    #[serde(default = "default_true")]
    pub check: bool,
    /// 录像 tick 数上限（不设 = 不限）。防"挂机一百万 tick 总会赢"式注水证书。
    #[serde(default)]
    pub max_ticks: Option<u64>,
    /// playtest 门（可选）。声明了它，`vitric gate` 多跑一道 playtest 门：按这里的配置
    /// 跑一遍 swarm/lookahead/种子探索、聚合出报告，再逐条核对声明的断言（通关率/软锁数/
    /// 不可达结局/惰性动作/数值崩）。不声明 = 不跑这道门（现有 gate 行为不变）。
    #[serde(default)]
    pub playtest: Option<PlaytestGate>,
}

/// playtest 门声明（清单 `gates.playtest` 字段）。
///
/// 把"自动清地板"变成交付契约：项目声明它必须达到的 playtest 门槛（多少局、能不能通关、
/// 软锁上限等），`vitric gate` 真跑一遍 playtest swarm 断言达标才放行。playtest swarm 是
/// 确定的（同种子同输入 = 同结果），所以这道门可复现。
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
/// 跑法字段（sessions/max_ticks/strategy/horizon/seed_recording）决定怎么跑；断言字段
/// （require_clearable/min_clear_rate/max_soft_locks/...）都可选，**填了才查**——没填的
/// 维度不参与裁决，只把关心的契约写进清单。
#[derive(Debug, Clone, Deserialize)]
pub struct PlaytestGate {
    /// 跑多少局（默认 16）。strategy=lookahead 时是跑多少局 lookahead。
    #[serde(default = "default_sessions")]
    pub sessions: usize,
    /// 每局 tick 上限（默认 600）。
    #[serde(default = "default_pt_max_ticks")]
    pub max_ticks: u64,
    /// 跑法策略（不填=默认策略组 swarm 轮换四策略；可填 "lookahead" 走前瞻搜索跑 sessions 局）。
    #[serde(default)]
    pub strategy: Option<String>,
    /// lookahead 束搜索的**深度**（往前规划几帧；仅 strategy=lookahead 用，默认 8，1=单步前瞻）。
    /// 字段名沿用 `horizon` 向后兼容旧清单（语义已并入「搜索深度」）。
    #[serde(default = "default_horizon")]
    pub horizon: u64,
    /// lookahead 束搜索的**束宽**（每层保留多少最优节点继续展开；仅 strategy=lookahead 用，默认 4）。
    #[serde(default = "default_beam")]
    pub beam: usize,
    /// 种子录像（相对项目根）。填了走种子式探索：以这条录像为基线扰动出 sessions 条变异跑。
    #[serde(default)]
    pub seed_recording: Option<String>,

    // ---- 断言（都可选，填了才查；没填的维度不参与裁决）----
    /// true = 通关率必须 > 0（swarm 至少通关一次）。
    #[serde(default)]
    pub require_clearable: Option<bool>,
    /// 通关率下限（0..1）。实际通关率 < 这个值就 fail。
    #[serde(default)]
    pub min_clear_rate: Option<f64>,
    /// 软锁簇（stuck_clusters）数上限。超了 fail。
    #[serde(default)]
    pub max_soft_locks: Option<usize>,
    /// 不可达结局（ending_coverage.unreachable_endings）数上限。超了 fail。
    #[serde(default)]
    pub max_unreachable_endings: Option<usize>,
    /// 惰性动作（inert_actions）数上限。超了 fail。
    #[serde(default)]
    pub max_inert_actions: Option<usize>,
    /// true = 数值崩（runaway/collapse/non_finite）必须全空，任一非空就 fail。
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

/// 一条通关录像门。
#[derive(Debug, Clone, Deserialize)]
pub struct PlaythroughGate {
    /// 录像文件（相对项目根），来自 `vitric run --record`。
    pub recording: String,
    /// 重放过程中必须观测到的事件（终局信号）。默认 "game-won"。
    #[serde(default = "default_must_emit")]
    pub must_emit: String,
}

fn default_true() -> bool {
    true
}

fn default_must_emit() -> String {
    "game-won".to_string()
}

/// 性能预算。0 = 不限。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Budgets {
    /// 存活实体数上限。
    #[serde(default)]
    pub max_entities: u64,
    /// 单 tick 事件数上限（事件风暴探测）。
    #[serde(default)]
    pub max_events_per_tick: u64,
}

/// 一个动画片段：帧图序列 + 播放速率。
///
/// ```json
/// { "clips": { "coin-spin": { "frames": ["coin1.png", "coin2.png"], "fps": 8, "loop": true } } }
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct Clip {
    /// 帧图（素材仓库里的路径）。
    pub frames: Vec<String>,
    pub fps: u32,
    /// true 循环播放；false 播完停在末帧并发 anim-finished 事件。
    #[serde(default, rename = "loop")]
    pub looping: bool,
}

fn default_seed() -> u64 {
    0
}

/// 主题名 = 文件名去掉目录和 `.json` 后缀（`themes/dark.json` → `dark`）。
/// 控件引用主题用这个名字。
fn theme_name(rel: &str) -> String {
    rel.rsplit('/')
        .next()
        .unwrap_or(rel)
        .strip_suffix(".json")
        .unwrap_or(rel)
        .to_string()
}

/// 加载完成的项目：清单 + schema + 全部场景（已校验）+ 规则/脚本原文。
///
/// 规则的语义校验在 vitric-rules（它认识触发器/动作的结构）；
/// 这里只保证 JSON 能解析，职责分明。
#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: ProjectManifest,
    pub schema: Schema,
    /// 相对路径 -> 场景
    pub scenes: BTreeMap<String, Scene>,
    /// (相对路径, 规则文档)
    pub rules: Vec<(String, Value)>,
    /// (相对路径, 脚本源码)
    pub scripts: Vec<(String, String)>,
    /// 序列（名字 -> 已校验的静态轨道）。
    pub sequences: BTreeMap<String, Sequence>,
    /// 动画片段（名字 -> 定义）。
    pub animations: BTreeMap<String, Clip>,
    /// 主题（名字 -> 已校验的样式卷）。装配期常量，不进世界状态。
    pub themes: BTreeMap<String, crate::Theme>,
}

impl Project {
    /// 从目录加载整个项目。所有问题（IO/解析/校验）汇总成一份报告一次给全。
    pub fn load(root: impl AsRef<Path>) -> Result<Project, ValidationReport> {
        let root = root.as_ref().to_path_buf();
        let mut report = ValidationReport::default();

        // 清单
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

        // 场景
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

        // 规则（仅解析 JSON，语义校验归 vitric-rules）
        let mut rules = Vec::new();
        for rel in &manifest.rules {
            match read_json(&root.join(rel)) {
                Ok(doc) => rules.push((rel.clone(), doc)),
                Err(e) => report.push("VD040", rel, e, "清单 rules 列表里的文件必须存在"),
            }
        }

        // 脚本（源码原文，执行归 vitric-script）
        let mut scripts = Vec::new();
        for rel in &manifest.scripts {
            match fs::read_to_string(root.join(rel)) {
                Ok(src) => scripts.push((rel.clone(), src)),
                Err(e) => report.push("VD040", rel, format!("读取失败: {e}"), "清单 scripts 列表里的文件必须存在"),
            }
        }

        // 序列（时间轴）：每个文件一条，按 schema 校验（动作名/字段/at 单调等）。
        // 序列名冲突显式报错——运行时 Sequence 组件按名字引用，重名无法消歧。
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

        // 字体：只查文件存在（解析/损坏校验在 vitric-render 的 FontStore::load，
        // 那边认识 TTF；这里和 scenes/rules 一样只管"清单指的文件必须在"）
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

        // 动画
        let mut animations = BTreeMap::new();
        if let Some(rel) = &manifest.animations {
            match read_json(&root.join(rel)) {
                Ok(doc) => parse_animations(&doc, rel, &mut animations, &mut report),
                Err(e) => report.push("VD040", rel, e, "清单 animations 字段指向的文件必须存在"),
            }
        }

        // 主题：每个文件一份，名字取文件名（去 themes/ 前缀和 .json 后缀）。
        // 重名显式报错——控件按名字引用，重名无法消歧（和序列同口径）。
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
        // 不写 font 字段 = 合法（点阵字体旧行为）
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
