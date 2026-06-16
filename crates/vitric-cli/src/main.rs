//! vitric — 命令行入口。
//!
//! 子命令：
//! - `vitric check <项目目录>`            校验项目（schema/场景/规则/脚本），出报告
//! - `vitric run <项目目录> [选项]`        无头运行 + AI 控制面
//! - `vitric replay <项目目录> <录像.json>` 重放录像并校验确定性
//! - `vitric playtest <项目目录> [选项]`     进程内自动试玩：单局出可重放录像（默认），--sessions N 并行跑批聚合地板报告，--seed-recording 种子式探索（扰动证书录像逮不可达结局/顺序软锁）
//! - `vitric gate <项目目录>`              交付门禁：check + 通关录像重放 + 断言集 + 可选 playtest 门（清单声明 gates.playtest 才跑：真跑 swarm 断言达标），全过才出证书
//! - `vitric bundle <项目目录> [选项]`      发行打包：gate PASS 后把项目附进引擎副本，出自包含单文件（无证书不发行）
//! - `vitric assets <项目目录> [选项]`      全项目 PNG 统一色板（AI 出图规整成一个调）
//! - `vitric team <项目目录>`              多 agent 班子协同黑板：各角色交付物健康度 + 门禁/合同状态（只读，永远退出 0）
//! - `vitric turf <项目目录> --role <角色> <改动文件...>`  地盘执法：改动文件越出角色地盘即退出 1
//!
//! assets 选项：
//!   --colors <N>     色板颜色数（默认 32）
//!   --height <H>     高于 H 的图先最近邻缩到高 H（保持宽高比）
//!   --palette-lock   跳过提取，按项目已有 palette.json 量化（新素材入伙老色板）
//!   --normals        给没有 _n 配对的 PNG 生成法线贴图（程序化，确定性；与色板选项互斥）
//!   --normals-ai     法线生成改走豆包 Ark Seedream 图生图（需要环境变量 ARK_API_KEY；
//!                    模型 VITRIC_NORMALS_MODEL，默认 doubao-seedream-5-0-260128）
//!   --frames <目录>   帧进口流水线：把一组序列图（png，自然排序）一键变优化过的动画
//!                    素材——相邻帧去重（记停留）+ 裁透明边（记偏移）+ 打包图集 +
//!                    统一色板 + 写 animations.json + BC7 压缩图集。片段名取目录名。
//!                    视频先用 ffmpeg 转序列图（不内置解码器）。与色板/法线互斥；
//!                    自己接受 --colors（整组色板数）和 --no-compress（不压 BC7）。
//!   --no-compress    （仅 --frames）不离线压 BC7，只出未压缩 RGBA8 图集
//!
//! run 选项：
//!   --port <N>       控制面端口（默认 6173，0=自动分配）
//!   --speed <X>      初始倍速（默认 1.0）
//!   --ticks <N>      跑满 N tick 后自动退出（CI/脚本用，全速不限速）
//!   --record <文件>   退出时把录像写到文件
//!   --load <槽名>     启动后立刻从 <项目>/saves/<槽名>.json 恢复续玩（与 --record 互斥）
//!   --renderer <gpu|cpu> 窗口呈现路径（默认 cpu=softbuffer；gpu=wgpu，自带开窗）
//!
//! bundle 选项：
//!   --out <文件>     输出路径（默认 <项目名>-<平台>[.exe]）
//!   --engine <exe>   附加到指定引擎二进制（跨平台出包：linux 上给 windows 出包就指交叉产物）
//!
//! 发行包（exe 尾部有内嵌项目，见 src/bundle.rs）的启动行为：
//!   无参数            玩家双击：解包后开窗运行（CPU 渲染）
//!   run-embedded …    运行内嵌项目，run 同款选项透传（--ticks 5 无头冒烟 / --renderer gpu）
//!   其他参数          正常 CLI（发行包同时也是完整引擎）
//!
//! 运行时 LLM 经环境变量启用：VITRIC_LLM_URL / VITRIC_LLM_KEY / VITRIC_LLM_MODEL（见 src/llm.rs）。

mod audio;
mod gpu;
mod window;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use vitric_cli::llm::{self, Llm};
use vitric_cli::runtime::{self, Runtime};
use vitric_sim::GameLogic;
use vitric_control::{ControlServer, Dispatcher, SaveStore};
use vitric_data::Project;
use vitric_sim::{Recording, DT};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(|s| s.as_str()) {
        Some("check") => cmd_check(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        Some("replay") => cmd_replay(&args[1..]),
        Some("playtest") => cmd_playtest(&args[1..]),
        Some("gate") => cmd_gate(&args[1..]),
        Some("bundle") => vitric_cli::bundle::run(&args[1..]),
        Some("run-embedded") => cmd_run_embedded(&args[1..]),
        Some("assets") => vitric_cli::assets_cmd::run(&args[1..]),
        Some("team") => cmd_team(&args[1..]),
        Some("turf") => cmd_turf(&args[1..]),
        Some("gpu-probe") => gpu::gpu_probe(&args[1..]),
        None => cmd_default(),
        Some(_) => {
            usage_and_exit();
        }
    };
    if let Err(message) = result {
        // 错误走 stderr 且保持结构化前缀，AI 和人都好解析
        eprintln!("vitric 错误: {message}");
        std::process::exit(1);
    }
}

fn usage_and_exit() -> ! {
    eprintln!("用法: vitric <check|run|replay|playtest|gate|bundle|assets|team|turf> <项目目录> [选项]\n详见 vitric 仓库 docs/");
    std::process::exit(2);
}

/// 无参数启动。发行包（exe 尾部有内嵌项目）= 玩家双击：解包后开窗运行内嵌项目
/// （CPU 渲染，处处能跑；要 GPU 走 `run-embedded --renderer gpu`）；普通引擎 = 打用法。
fn cmd_default() -> Result<(), String> {
    if let Some(dir) = vitric_cli::bundle::extract_self()? {
        return cmd_run(&[dir.display().to_string(), "--window".to_string()]);
    }
    usage_and_exit();
}

/// `run-embedded [run 选项]`：发行包专用——解出内嵌项目再走 cmd_run，选项原样透传
/// （无头冒烟 `--ticks 5`、玩家要 GPU `--renderer gpu` 都从这进）。
fn cmd_run_embedded(args: &[String]) -> Result<(), String> {
    let Some(dir) = vitric_cli::bundle::extract_self()? else {
        return Err(
            "本 vitric 不是发行包（exe 尾部没有内嵌项目）。\
             提示：发行包用 vitric bundle <项目目录> 制作；普通运行用 vitric run <项目目录>"
                .to_string(),
        );
    };
    let mut full = vec![dir.display().to_string()];
    full.extend_from_slice(args);
    cmd_run(&full)
}

fn cmd_check(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("check 缺少项目目录参数")?;
    let report = runtime::check(&PathBuf::from(dir))?;
    println!("{}", serde_json::to_string_pretty(&report).expect("报告可序列化"));
    Ok(())
}

fn cmd_replay(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("replay 缺少项目目录参数")?;
    let rec_path = args.get(1).ok_or("replay 缺少录像文件参数")?;
    let rec_text =
        std::fs::read_to_string(rec_path).map_err(|e| format!("读取录像 {rec_path} 失败: {e}"))?;
    let rec: Recording =
        serde_json::from_str(&rec_text).map_err(|e| format!("录像解析失败: {e}"))?;
    // 重放模式不装配 LLM：llm-ask 事件无人监听，llm-reply/llm-error 全部来自
    // 录像的 replies 通道——重放永远不碰网络，离线逐位复现带 LLM 内容的那一局
    let (mut sim, mut rt) = Runtime::boot(&PathBuf::from(dir))?;
    sim.replay(&rec, &mut rt).map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::json!({
            "replayed_ticks": rec.ticks,
            "final_hash": format!("{:#018x}", rec.final_hash),
            "verified": true,
        })
    );
    Ok(())
}

/// `vitric playtest`：进程内自动试玩一局。boot 项目 → 派生场景视图喂策略 →
/// 循环步进直到通关/死亡/超时 → 出一份可重放录像。这是 agent 集群试玩的地基
/// （设计稿第 1 阶段）：装配住在本 crate（Runtime::boot），循环/视图/策略住在
/// vitric-playtest，这里把两边接起来。
///
/// 选项：
///   --strategy <random|greedy|economy|lookahead>  策略（默认 random；仅单局 N=1 用）。
///                                lookahead=前瞻搜索（技巧类游戏专用，慢但聪明，不进 swarm 默认轮换）
///   --horizon <K>                lookahead 每真 tick 向前投机多少帧（默认 12，仅 --strategy lookahead 用）
///   --seed <N>                   策略 PCG 播种（默认 0；swarm 模式从此起递增）
///   --max-ticks <N>              超时上限（默认 600）
///   --sessions <N>               跑 N 局 swarm 聚合出报告（默认 1=单局旧行为）
///   --llm <N>                    额外跑 N 局 LLM 拟人玩（吐定性 note 进报告 qualitative_notes）；
///                                需 VITRIC_LLM_URL/KEY/MODEL 配齐，没配齐报明确错误（不静默跳过）
///   --seed-recording <录像.json> 种子式探索：拿这条录像当种子，扰动它的输入序列跑 N 局
///   --out <路径>                 N=1 写录像；N>1 / 种子探索 / --llm 写完整报告 JSON
///   --report-dir <目录>          代表录像落盘目录（默认 <项目>/playtest-report/）；报告主体只挂相对路径
///
/// **每游戏视图覆盖**（设计稿一节、十一节第 6 条）：自动加载项目根 `playtest.json`（存在即用，
/// 否则默认 config=自动推视图、行为不变）——可挑组件/重命名/声明派生量（距离/别名/计数）/给
/// greedy 指目标（朝某派生量 min/max 走）/覆盖终止事件名。报告里 stuck/runaway 等维度的代表
/// 录像不再内联进报告主体，改成各存一份 json 落 report-dir，报告只挂相对路径（主体干净可读）。
///
/// 行为分档（优先级从上到下）：
/// - `--llm N>0`：swarm（sessions 局策略档）+ N 局 LLM 档拟人玩，合并聚合出报告。LLM 局的
///   note 进 `qualitative_notes`（主观提示，待人复核），它选的输入进录像可重放；策略档部分仍确定。
///
/// 其余三档：
/// - 给了 `--seed-recording`：**种子式探索**（设计稿第 3 阶段）——加载录像当种子，
///   `perturb_plan` 生成 N 条变异脚本（drop/swap/substitute/truncate 轮换 + 截断接 random 发散），
///   `run_seed_swarm` 并行跑，聚合报告含 `ending_coverage`（哪些声明结局不可达）。`--sessions`=变异条数。
/// - 没给但 `--sessions>1`：swarm——random+greedy+coverage+economy 四策略轮流 × 递增 seed
///   （economy 专为模拟经营找数值崩，报告含 `numeric_breakage`：经济跑飞/崩盘/溢出）。
///   **声明了 goal 的项目**（playtest.json 有 goal）默认自动把末尾少数几局（约 25%）换成前瞻搜索
///   （lookahead），让导航/技巧类游戏被真玩起来、不被随机策略误报 unbeatable；没声明 goal 则
///   默认组完全不变（向后兼容）。无需新选项，默认行为自动变聪明。
/// - 没给且 `--sessions=1`：单局旧行为，出一条可重放录像。
///
/// 各局都在自己线程内 boot 一份运行时并行跑（QuickJS 非 Send，运行时绝不跨线程）。
fn cmd_playtest(args: &[String]) -> Result<(), String> {
    use std::sync::Arc;

    use vitric_playtest::{
        aggregate_with_endings_and_declared, default_plan, perturb_plan, run_llm_sessions,
        run_seed_swarm, run_session, run_session_lookahead,
        run_swarm_with_config, EconomyStrategy, GreedyStrategy, LlmClient, LookaheadConfig,
        PlaytestConfig, RandomStrategy, SessionConfig, Strategy,
        TerminalSpec,
    };

    let dir = args.first().ok_or("playtest 缺少项目目录参数")?;
    let dir = PathBuf::from(dir);

    let mut strategy_name = "random".to_string();
    let mut horizon: u64 = 12;
    let mut seed: u64 = 0;
    let mut max_ticks: u64 = 600;
    let mut sessions: u64 = 1;
    let mut llm_sessions: u64 = 0;
    let mut out_path: Option<PathBuf> = None;
    let mut seed_recording: Option<PathBuf> = None;
    let mut report_dir: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        let need = |key: &str| format!("{key} 缺少参数值");
        match args[i].as_str() {
            "--report-dir" => {
                report_dir = Some(PathBuf::from(args.get(i + 1).ok_or(need("--report-dir"))?));
                i += 2;
            }
            "--strategy" => {
                strategy_name = args.get(i + 1).ok_or(need("--strategy"))?.clone();
                i += 2;
            }
            "--horizon" => {
                horizon = args.get(i + 1).ok_or(need("--horizon"))?.parse().map_err(|e| format!("--horizon: {e}"))?;
                i += 2;
            }
            "--seed" => {
                seed = args.get(i + 1).ok_or(need("--seed"))?.parse().map_err(|e| format!("--seed: {e}"))?;
                i += 2;
            }
            "--max-ticks" => {
                max_ticks = args.get(i + 1).ok_or(need("--max-ticks"))?.parse().map_err(|e| format!("--max-ticks: {e}"))?;
                i += 2;
            }
            "--sessions" => {
                sessions = args.get(i + 1).ok_or(need("--sessions"))?.parse().map_err(|e| format!("--sessions: {e}"))?;
                if sessions == 0 {
                    return Err("--sessions 至少为 1".to_string());
                }
                i += 2;
            }
            "--llm" => {
                llm_sessions = args.get(i + 1).ok_or(need("--llm"))?.parse().map_err(|e| format!("--llm: {e}"))?;
                i += 2;
            }
            "--seed-recording" => {
                seed_recording = Some(PathBuf::from(args.get(i + 1).ok_or(need("--seed-recording"))?));
                i += 2;
            }
            "--out" => {
                out_path = Some(PathBuf::from(args.get(i + 1).ok_or(need("--out"))?));
                i += 2;
            }
            other => {
                return Err(format!("未知选项 {other:?}。可用: --strategy --horizon --seed --max-ticks --sessions --llm --seed-recording --out --report-dir"))
            }
        }
    }

    // 自动加载项目根 playtest.json（存在即用，否则默认 config=自动推视图、行为不变）。
    // 解析失败带路径明确报错（vitric check 风格），不静默跳过。
    let config = PlaytestConfig::load(&dir)?.unwrap_or_default();
    // 项目清单声明的权威通关事件：gates.playthroughs[].must_emit。脚本/LLM 游戏
    // （如 echo 的 run-complete）的胜利事件不在通用默认 TerminalSpec 里，靠这条并进来，
    // 否则会被误判"谁也通不了"、ending_coverage 空。boot 内部已加载，这里再 load 一次取
    // manifest 是廉价的（只读 JSON），换来不必把 must_emit 一路穿过 boot 的好处。
    let manifest_must_emit: Vec<String> = match vitric_data::Project::load(&dir) {
        Ok(project) => project
            .manifest
            .gates
            .as_ref()
            .map(|g| g.playthroughs.iter().map(|p| p.must_emit.clone()).collect())
            .unwrap_or_default(),
        // 清单读不出来不在这里硬错（boot 自己会报）——退化为空集，等价没声明 gates。
        Err(_) => Vec::new(),
    };

    // 终止规格：playtest.json 的 terminal 覆盖（没写就是默认集合）先生效，
    // 再把清单 must_emit 追加进 win 集合（叠加去重，不替换覆盖结果）。
    let terminal = match &config.terminal {
        Some(ovr) => TerminalSpec::default().apply_override(ovr),
        None => TerminalSpec::default(),
    }
    .with_manifest_must_emit(&manifest_must_emit);
    // 代表录像落盘目录：默认 <项目>/playtest-report/。报告主体只挂相对路径，录像各自一份文件。
    let report_dir = report_dir.unwrap_or_else(|| dir.join("playtest-report"));

    // 报告产出口径统一：外置代表录像到 report-dir（回填相对路径）→ 打干净 JSON 到 stdout →
    // --out 时另存一份完整报告 JSON。录像不内联进报告主体（设计稿十一节第 6 条）。
    let emit_report = |mut report: vitric_playtest::Report,
                       out_path: &Option<PathBuf>|
     -> Result<(), String> {
        report.externalize_recordings(&report_dir)?;
        let json = serde_json::to_string_pretty(&report).expect("报告可序列化");
        if let Some(out) = out_path {
            std::fs::write(out, &json)
                .map_err(|e| format!("写报告 {} 失败: {e}", out.display()))?;
        }
        println!("{json}");
        Ok(())
    };

    // --llm N>0：在 swarm 里额外跑 N 局 LLM 拟人玩（设计稿五阶段）。LLM 局的 note 进报告
    // 的 qualitative_notes，它选的输入进录像（可重放）。装真 client（VITRIC_LLM_* 没配齐
    // 直接报明确错误，不静默跳过）。LLM 档本就慢，单独串行跑，跑完并进策略档结果集聚合。
    if llm_sessions > 0 {
        let client: Arc<dyn LlmClient> = Arc::new(
            vitric_cli::playtest_llm::PlaytestLlmClient::from_env()?,
        );
        let factory = || -> Result<(_, _, _), String> {
            let (sim, rt) = Runtime::boot(&dir)?;
            let engine = rt.rules.clone();
            Ok((sim, rt, engine))
        };
        // 同一个工厂闭包要喂两个函数（run_swarm 移走会再无法用），借一份共享引用：
        // &F where F: Fn 仍是 Fn + Sync，两边都用引用，不重复 boot 逻辑。
        let factory_ref = &factory;
        // 策略档：sessions 局廉价策略（轮换 + 递增 seed），和无 --llm 时同口径。
        // 声明了 goal 的项目自动掺几局前瞻（default_plan 内部判 has_goal）。
        let plan = default_plan(sessions, seed, max_ticks, terminal.clone(), config.goal.is_some());
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        // 带 config：greedy 朝 config.goal 走，视图按 include/exclude/relabel/派生量调整
        let mut results = run_swarm_with_config(factory_ref, &plan, &config, threads)?;
        // LLM 档：N 局拟人玩，结果并进同一个集合喂聚合器。目标描述拼 config.goal（有则给 LLM 指向）
        let goal_hint = config
            .goal
            .as_ref()
            .map(|g| format!("{:?} {}", g.direction, g.quantity))
            .unwrap_or_default();
        let llm_results = run_llm_sessions(
            factory_ref,
            client,
            llm_sessions as usize,
            &goal_hint,
            seed,
            max_ticks,
            terminal.clone(),
        )?;
        results.extend(llm_results);

        // 有声明结局就一并算结局覆盖（叙事项目尤其需要）
        let (_, rt) = Runtime::boot(&dir)?;
        let report = aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &manifest_must_emit);
        return emit_report(report, &out_path);
    }

    // 给了 --seed-recording：种子式探索（第 3 阶段）。加载种子录像 → perturb_plan 生成
    // N 条变异 → ScriptedStrategy（截断接 random 发散）并行跑 → 聚合含 ending_coverage。
    if let Some(seed_path) = &seed_recording {
        let rec_text = std::fs::read_to_string(seed_path)
            .map_err(|e| format!("读取种子录像 {} 失败: {e}", seed_path.display()))?;
        let seed_rec: Recording =
            serde_json::from_str(&rec_text).map_err(|e| format!("种子录像解析失败: {e}"))?;

        // 变异条数 = --sessions（第 0 条是基线=原种子）。--seed 复用为扰动 PCG 播种，
        // 截断脚本的 random 发散用 seed+1 错开（与扰动 PCG 不同源）。
        let plan = perturb_plan(&seed_rec, sessions as usize, seed);
        let factory = || -> Result<(_, _, _), String> {
            let (sim, rt) = Runtime::boot(&dir)?;
            let engine = rt.rules.clone();
            Ok((sim, rt, engine))
        };
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        // 种子回复跟着种子走：扰动只动输入，回复原样按原 tick 注回去（run_seed_swarm 内部按截断过滤）。
        let results = run_seed_swarm(
            factory,
            &plan,
            &seed_rec.replies,
            max_ticks,
            terminal.clone(),
            seed.wrapping_add(1),
            threads,
        )?;

        // 结局覆盖要扫规则声明的结局集合，单独 boot 一份只读 Engine 喂聚合器
        let (_, rt) = Runtime::boot(&dir)?;
        let report = aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &manifest_must_emit);
        return emit_report(report, &out_path);
    }

    // N>1：swarm 跑批 + 聚合报告
    if sessions > 1 {
        // 计划：四策略轮流 × 递增 seed，凑够 N 局。每条 spec 自带 (策略,seed,max_ticks,terminal)，
        // 一局结果只由 spec + config 决定（确定性铁律）——串行/并行结果一致。
        // 声明了 goal（playtest.json）的项目，default_plan 自动把末尾少数几局换成前瞻搜索，
        // 让导航/技巧类不被随机策略误报 unbeatable；没声明 goal 则默认组完全不变（向后兼容）。
        let plan = default_plan(sessions, seed, max_ticks, terminal.clone(), config.goal.is_some());

        // 工厂闭包：每个工作线程在自己线程内调它 boot 一份全新运行时。
        // 返回 (Sim, Runtime, Engine)——Engine 是装配期无状态副本（derive Clone），
        // 跑 run_session 时同时可变借 logic 和不可变借 engine 需要它独立一份。
        let factory = || -> Result<(_, _, _), String> {
            let (sim, rt) = Runtime::boot(&dir)?;
            let engine = rt.rules.clone();
            Ok((sim, rt, engine))
        };

        // 线程数默认机器并行度（run_swarm 内部再取 min(plan, cpu)）
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        // 带 config：greedy 朝 config.goal 走，视图按 include/exclude/relabel/派生量调整。
        // 有声明结局就算结局覆盖（用 config 覆盖后的 terminal 扫声明集合）。
        let results = run_swarm_with_config(factory, &plan, &config, threads)?;
        let (_, rt) = Runtime::boot(&dir)?;
        let report = aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &manifest_must_emit);
        return emit_report(report, &out_path);
    }

    // N=1：单局旧行为（出可重放录像）。
    let out = out_path.unwrap_or_else(|| dir.join("playtest-session.json"));
    // boot 一对全新的 (sim, runtime)：录可重放录像必须从冷启动起录
    let (mut sim, mut rt) = Runtime::boot(&dir)?;
    // 单局也走 config：终止覆盖 + 派生量视图（greedy/lookahead 找目标）。
    let cfg = SessionConfig { max_ticks, seed, terminal: terminal.clone(), playtest: config.clone(), ..Default::default() };
    // run_session 要同时可变借 logic(rt) 和不可变借 engine(rt.rules)——同一对象借不动。
    // Engine 是装配期无状态副本，复制一份只读的传进去最干净（见 Engine 的 derive 注释）。
    let engine = rt.rules.clone();

    // lookahead 走前瞻搜索（每真 tick 投机 horizon 帧选最优），其余走普通策略 run_session。
    // 成本注释：前瞻每真 tick 代价 = |候选动作+不操作| × horizon 个投机 step，远贵于廉价策略，
    // 所以只在显式 --strategy lookahead 时启用，**不进 swarm 默认轮换**（swarm 要跑成百上千局）。
    let result = if strategy_name == "lookahead" {
        run_session_lookahead(&mut sim, &mut rt, &engine, &cfg, &LookaheadConfig { horizon })?
    } else {
        // economy 也能选——单局压一种策略看它怎么跑。
        // greedy 有 config.goal 时朝目标走（playtest.json 声明派生量 + goal），否则退化随机。
        let mut strategy: Box<dyn Strategy> = match strategy_name.as_str() {
            "random" => Box::new(RandomStrategy::new(seed)),
            "greedy" => match &config.goal {
                Some(g) => Box::new(GreedyStrategy::with_goal(seed, g.clone())),
                None => Box::new(GreedyStrategy::new(seed)),
            },
            "economy" => Box::new(EconomyStrategy::new(seed)),
            other => return Err(format!("--strategy 只认 random / greedy / economy / lookahead，拿到 {other:?}")),
        };
        run_session(&mut sim, &mut rt, &engine, strategy.as_mut(), &cfg)?
    };

    let json = serde_json::to_string_pretty(&result.recording).expect("录像可序列化");
    std::fs::write(&out, json).map_err(|e| format!("写录像 {} 失败: {e}", out.display()))?;

    println!(
        "{}",
        serde_json::json!({
            "outcome": format!("{:?}", result.outcome),
            "ticks": result.ticks,
            "out": out.display().to_string(),
        })
    );
    Ok(())
}

/// `vitric gate`：交付门禁。报告（人和机器同一份 JSON）永远打到 stdout；
/// 全部门禁 pass 才退出 0——"交付完成"由这里裁决，不由 agent 自述。
///
/// 门集：check + 通关录像重放 + 断言集（见 gate::run），外加**可选的 playtest 门**——清单
/// 声明 `gates.playtest` 才跑：真跑一遍 playtest swarm（确定可复现）、聚合出报告、逐条核对
/// 声明的契约（能不能通关 / 软锁数 / 不可达结局 / 惰性动作 / 数值崩），把"自动清地板"变成
/// 交付契约。没声明 = 不跑这道门（向后兼容，现有 gate 行为不变）。
fn cmd_gate(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("gate 缺少项目目录参数")?;
    let (report, pass) = vitric_cli::gate::run(&PathBuf::from(dir))?;
    println!("{}", serde_json::to_string_pretty(&report).expect("报告可序列化"));
    if pass {
        Ok(())
    } else {
        Err("交付门禁未通过（fail 项详见上方 JSON 报告）".to_string())
    }
}

/// `vitric team`：班子协同黑板。状态工具不是门——报告打到 stdout 后**永远成功退出**，
/// 有卡点也只是 blocking 提示；交付裁决归 `vitric gate`。
fn cmd_team(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("team 缺少项目目录参数")?;
    let report = vitric_cli::team::run(&PathBuf::from(dir))?;
    println!("{}", serde_json::to_string_pretty(&report).expect("报告可序列化"));
    Ok(())
}

/// `vitric turf`：地盘执法。报告同 gate 风格打到 stdout；有越界文件就退出 1。
fn cmd_turf(args: &[String]) -> Result<(), String> {
    let (report, pass) = vitric_cli::turf::run(args)?;
    println!("{}", serde_json::to_string_pretty(&report).expect("报告可序列化"));
    if pass {
        Ok(())
    } else {
        Err("地盘违规（violations 详见上方 JSON 报告）——跨地盘需求走事件约定提给导演".to_string())
    }
}

fn cmd_run(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("run 缺少项目目录参数")?;
    let mut port: u16 = 6173;
    let mut speed: f64 = 1.0;
    let mut max_ticks: Option<u64> = None;
    let mut record_path: Option<String> = None;
    let mut load_slot: Option<String> = None;
    let mut windowed = false;
    let mut renderer = window::Renderer::Cpu;
    let mut i = 1;
    while i < args.len() {
        let need = |key: &str| format!("{key} 缺少参数值");
        match args[i].as_str() {
            "--window" => {
                windowed = true;
                i += 1;
            }
            "--renderer" => {
                renderer = match args.get(i + 1).ok_or(need("--renderer"))?.as_str() {
                    "cpu" => window::Renderer::Cpu,
                    "gpu" => window::Renderer::Gpu,
                    other => {
                        return Err(format!("--renderer 只认 gpu 或 cpu，拿到 {other:?}"))
                    }
                };
                i += 2;
            }
            "--port" => {
                port = args.get(i + 1).ok_or(need("--port"))?.parse().map_err(|e| format!("--port: {e}"))?;
                i += 2;
            }
            "--speed" => {
                speed = args.get(i + 1).ok_or(need("--speed"))?.parse().map_err(|e| format!("--speed: {e}"))?;
                i += 2;
            }
            "--ticks" => {
                max_ticks = Some(args.get(i + 1).ok_or(need("--ticks"))?.parse().map_err(|e| format!("--ticks: {e}"))?);
                i += 2;
            }
            "--record" => {
                record_path = Some(args.get(i + 1).ok_or(need("--record"))?.clone());
                i += 2;
            }
            "--load" => {
                load_slot = Some(args.get(i + 1).ok_or(need("--load"))?.clone());
                i += 2;
            }
            other => return Err(format!("未知选项 {other:?}。可用: --window --renderer --port --speed --ticks --record --load")),
        }
    }
    // GPU 是窗口呈现路径，没有无头形态——选了 gpu 就等于要开窗
    if matches!(renderer, window::Renderer::Gpu) {
        windowed = true;
    }

    let dir = PathBuf::from(dir);
    let project = Project::load(&dir).map_err(|r| r.to_string())?;
    let (mut sim, mut rt) = Runtime::boot(&dir)?;
    let mut dispatcher = Dispatcher::new(project.schema.clone());
    dispatcher.load_assets(&dir.join("assets"))?;
    // 清单挂了 font：启动期加载（缺失/损坏立刻报错），所有 Text 走矢量路径
    if let Some(font_rel) = &project.manifest.font {
        dispatcher.load_font(&dir.join(font_rel))?;
    }
    dispatcher.set_budgets(project.manifest.budgets.clone());
    dispatcher.ctl.speed = speed;

    // 玩家存档：槽位固定在 <项目>/saves/，约定事件（save-game/load-game）、
    // save/* RPC、--load 共用这一个 SaveStore——同一条代码路径同一套校验
    let saves = SaveStore::new(&dir, &project.manifest.name);
    if let Some(slot) = &load_slot {
        // 录像必须从项目数据冷启动可逐位重放；从存档续玩的会话起点不是冷启动，
        // 录出来的录像 vitric replay 必然在起点就跑偏——明确互斥，不产出废录像
        if record_path.is_some() {
            return Err(
                "--load 与 --record 互斥：录像要求从项目数据冷启动可重放，从存档续玩的\
                 一局录不出可重放的录像。要录像就从头跑，要续玩就去掉 --record"
                    .to_string(),
            );
        }
        // 在 tick 0 之前恢复：start 事件不会重发（存档时刻早已过了 tick 0）
        let snap = saves.read(slot)?;
        sim.restore(&snap, &mut rt).map_err(|e| format!("--load {slot}: 存档恢复失败: {e}"))?;
    }
    dispatcher.set_save_store(saves);

    if record_path.is_some() {
        sim.start_recording();
    }

    let server = ControlServer::start(port)?;
    // 音频：无声卡环境合法降级，横幅明说
    let (mut audio_sink, audio_status) = match audio::Audio::open(dir.join("sounds")) {
        Ok(a) => (Some(a), "ok".to_string()),
        Err(e) => (None, format!("disabled: {e}")),
    };
    // 运行时 LLM：环境变量配齐才启用；没配也是合法状态（llm-ask 会收到显式 llm-error）
    let mut llm = Llm::from_env();
    // 启动横幅走 stdout 单行 JSON：AI 解析端口，人也看得懂
    println!(
        "{}",
        serde_json::json!({
            "vitric": "running",
            "project": project.manifest.name,
            "control": format!("http://127.0.0.1:{}/rpc", server.port),
            "seed": project.manifest.seed,
            "window": windowed,
            "audio": audio_status,
            "llm": llm.banner(),
        })
    );

    if windowed {
        let title = format!("{} — Vitric", project.manifest.name);
        let game =
            window::WindowedGame::new(sim, rt, dispatcher, server, audio_sink, llm, renderer, title);
        let (mut sim, error) = game.run()?;
        finish_recording(&mut sim, record_path)?;
        return match error {
            Some(e) => Err(e),
            None => Ok(()),
        };
    }

    // 主循环：固定步长 + 倍速 + 帧边界处理控制命令
    let mut last = Instant::now();
    let mut acc: f64 = 0.0;
    loop {
        for req in server.drain() {
            let resp = dispatcher.handle(&req.request, &mut sim, &mut rt);
            req.respond(resp);
        }
        if dispatcher.ctl.quit {
            break;
        }
        if let Some(max) = max_ticks {
            if sim.tick >= max {
                break;
            }
        }

        if dispatcher.ctl.paused {
            std::thread::sleep(Duration::from_millis(2));
            last = Instant::now();
            acc = 0.0;
            continue;
        }

        if max_ticks.is_some() {
            // 有限跑：全速不睡觉
            step_once(&mut sim, &mut rt, &mut dispatcher, &mut audio_sink, &mut llm)?;
            continue;
        }

        // 实时跑：按墙钟攒步数（墙钟只决定「跑几步」，永远不进模拟）
        let now = Instant::now();
        acc += now.duration_since(last).as_secs_f64() * dispatcher.ctl.speed;
        last = now;
        let mut budget = 8; // 单帧补步上限，防螺旋死亡
        while acc >= DT && budget > 0 {
            step_once(&mut sim, &mut rt, &mut dispatcher, &mut audio_sink, &mut llm)?;
            acc -= DT;
            budget -= 1;
        }
        if budget == 0 {
            acc = 0.0; // 跟不上就丢，不追帧
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    finish_recording(&mut sim, record_path)?;
    Ok(())
}

fn finish_recording(sim: &mut vitric_sim::Sim, record_path: Option<String>) -> Result<(), String> {
    if let Some(path) = record_path {
        let rec = sim.stop_recording().expect("启动时已开录");
        std::fs::write(&path, serde_json::to_string(&rec).expect("录像可序列化"))
            .map_err(|e| format!("写录像 {path} 失败: {e}"))?;
        println!(
            "{}",
            serde_json::json!({
                "recorded": path,
                "ticks": rec.ticks,
                "final_hash": format!("{:#018x}", rec.final_hash),
            })
        );
    }
    Ok(())
}

pub fn step_once(
    sim: &mut vitric_sim::Sim,
    rt: &mut Runtime,
    dispatcher: &mut Dispatcher,
    audio_sink: &mut Option<audio::Audio>,
    llm: &mut Llm,
) -> Result<(), String> {
    let report = sim.step(rt).map_err(|e| e.to_string())?;
    dispatcher.record_events(report.tick, &report.events);
    let observed = rt.drain_observed();
    audio::handle_sound_events(audio_sink, &observed);
    // LLM：捕获本 tick 的 llm-ask 排队发请求，再收割已完成的回复注入下一 tick
    llm::handle_ask_events(llm, &observed, sim);
    llm::pump_replies(llm, sim);
    dispatcher.record_events(report.tick, &observed);
    // 存档约定事件：save-game 写盘（纯输出副作用，同音频）；load-game 在这里——
    // 帧边界、模拟之外——整体回跳到存档时刻。失败结构化上报 stderr，不崩游戏
    for error in dispatcher.handle_save_load_events(&observed, sim, rt) {
        eprintln!("{}", serde_json::json!({"save_error": error}));
    }
    for failure in dispatcher.check_assertions(sim) {
        // 断言失败实时上报到 stderr（结构化一行），同时存进 assert/failures
        eprintln!("{}", serde_json::json!({"assert_failure": failure}));
    }
    Ok(())
}
