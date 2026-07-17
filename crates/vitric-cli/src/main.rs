//! vitric — CLI entry point.
//!
//! Subcommands:
//! - `vitric check <project-dir>`            verify project (schema/scene/rule/script), produce report
//! - `vitric run <project-dir> [options]`    headless run + AI control plane
//! - `vitric replay <project-dir> <recording.json>` replay recording and verify determinism
//! - `vitric playtest <project-dir> [options]` in-process automatic playtest: single session produces a replayable recording (default), --sessions N runs a parallel batch and aggregates a floor report, --seed-recording seed-style exploration (perturb a certificate recording to catch unreachable endings / sequence soft-locks)
//! - `vitric balance <project-dir> [options]` auto balance: tune one numeric knob (--knob file#json-pointer), use an agent cluster to playtest repeatedly, binary search to a knob value that puts the clear rate within --target-clear-rate (non-monotonic degraded linear scan fallback). Never modifies user files (changes happen in a temporary copy that is deleted afterward)
//! - `vitric gate <project-dir>`             delivery gate: check + clear recording replay + assertion set + optional playtest gate (only runs when manifest declares gates.playtest: actually runs swarm and asserts passing), certificate issued only when all pass
//! - `vitric bundle <project-dir> [options]` release bundle: after gate PASS, attach the project into an engine copy and produce a self-contained single file (no release without certificate)
//! - `vitric assets <project-dir> [options]` unify palette for all project PNGs (regularize AI-generated images into one tone)
//! - `vitric team <project-dir>`             multi-agent team coordination blackboard: per-role deliverable health + gate/contract status (read-only, always exits 0)
//! - `vitric turf <project-dir> --role <role> <changed-files...>`  turf enforcement: changed files exceeding the role's turf means exit 1
//!
//! assets options:
//!   --colors <N>     palette color count (default 32)
//!   --height <H>     images taller than H are first nearest-neighbor scaled down to height H (preserving aspect ratio)
//!   --palette-lock   skip extraction, quantize against the project's existing palette.json (new assets join the old palette)
//!   --normals        generate normal maps for PNGs without _n pairing (procedural, deterministic; mutually exclusive with palette options)
//!   --normals-ai     normal generation goes through Doubao Ark Seedream image-to-image (requires env var ARK_API_KEY;
//!                    model VITRIC_NORMALS_MODEL, default doubao-seedream-5-0-260128)
//!   --frames <dir>   frame import pipeline: turns a set of sequence images (png, natural sort) into optimized animation
//!                    assets — adjacent frame dedup (record dwell) + transparent-edge crop (record offset) + atlas packing +
//!                    unify palette + write animations.json + BC7 compress atlas. Clip name taken from directory name.
//!                    Convert video to sequence images with ffmpeg first (no built-in decoder). Mutually exclusive with palette/normals;
//!                    accepts --colors (palette count for the whole set) and --no-compress (skip BC7 compression).
//!   --no-compress    (--frames only) skip offline BC7 compression, only produce uncompressed RGBA8 atlas
//!
//! run options:
//!   --port <N>       control plane port (default 6173, 0=auto-assign)
//!   --speed <X>      initial speed multiplier (default 1.0)
//!   --ticks <N>      auto-exit after running N ticks (for CI/scripts, full speed no throttling)
//!   --record <file>  write recording to file on exit
//!   --load <slot>    immediately restore from <project>/saves/<slot>.json on startup to continue playing (mutually exclusive with --record)
//!   --renderer <gpu|cpu> window presentation path (default cpu=softbuffer; gpu=wgpu, opens its own window)
//!
//! bundle options:
//!   --out <file>     output path (default <project-name>-<platform>[.exe])
//!   --engine <exe>   attach to the specified engine binary (cross-platform bundling: e.g. on Linux, point at a cross-compiled Windows artifact for a Windows bundle)
//!
//! Release bundle (exe tail contains an embedded project, see src/bundle.rs) startup behavior:
//!   no args          player double-click: unpack and run in a window (CPU rendering)
//!   run-embedded …   run the embedded project, same options as run are passed through (--ticks 5 headless smoke / --renderer gpu)
//!   other args       normal CLI (release bundle is also a complete engine)
//!
//! Runtime LLM is enabled via env vars: VITRIC_LLM_URL / VITRIC_LLM_KEY / VITRIC_LLM_MODEL (see src/llm.rs).

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
        Some("balance") => vitric_cli::balance::run(&args[1..]),
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
        // Errors go to stderr with a structured prefix, easy for both AI and humans to parse
        eprintln!("vitric 错误: {message}");
        std::process::exit(1);
    }
}

fn usage_and_exit() -> ! {
    eprintln!("用法: vitric <check|run|replay|playtest|balance|gate|bundle|assets|team|turf> <项目目录> [选项]\n详见 vitric 仓库 docs/");
    std::process::exit(2);
}

/// No-arg startup. Release bundle (exe tail contains an embedded project) = player double-click: unpack and run the embedded project in a window
/// (CPU rendering, runs anywhere; for GPU use `run-embedded --renderer gpu`); normal engine = print usage.
fn cmd_default() -> Result<(), String> {
    if let Some(dir) = vitric_cli::bundle::extract_self()? {
        return cmd_run(&[dir.display().to_string(), "--window".to_string()]);
    }
    usage_and_exit();
}

/// `run-embedded [run options]`: release bundle only — extract the embedded project and pass to cmd_run, options passed through as-is
/// (headless smoke `--ticks 5`, player GPU `--renderer gpu` both come in here).
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
    // Replay mode does not assemble LLM: llm-ask events have no listener, llm-reply/llm-error all come from
    // the recording's replies channel — replay never touches the network, reproduces the LLM-bearing session bit-for-bit offline
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

/// `vitric playtest`: in-process automatic single-session playtest. boot project → derive a scene view for the strategy →
/// loop stepping until clear/death/timeout → produce a replayable recording. This is the foundation for agent cluster playtesting
/// (design draft phase 1): assembly lives in this crate (Runtime::boot), loop/view/strategy live in
/// vitric-playtest, this file wires the two sides together.
///
/// Options:
///   --strategy <random|greedy|economy|lookahead>  strategy (default random; only for single-session N=1).
///                                lookahead=beam-search rolling planner (specialized for skill-based games, slow but smart, can solve multi-tick
///                                combinations / continuous maneuvers; not in swarm default rotation)
///   --horizon <K>                lookahead beam search **depth** (how many frames to plan ahead; default 8, K=1 degrades to single-step lookahead).
///                                only used with --strategy lookahead
///   --beam <W>                   lookahead beam search **beam width** (W best nodes retained per layer; default 4).
///                                only used with --strategy lookahead
///   --seed <N>                   strategy PCG seed (default 0; in swarm mode increments from this)
///   --max-ticks <N>              timeout upper bound (default 600)
///   --sessions <N>               run N sessions as a swarm and aggregate a report (default 1=single-session legacy behavior)
///   --llm <N>                    additionally run N LLM humanoid sessions (qualitative notes go into report qualitative_notes);
///                                requires VITRIC_LLM_URL/KEY/MODEL all set; if not all set, report a clear error (do not silently skip)
///   --seed-recording <recording.json> seed-style exploration: take this recording as a seed, perturb its input sequence and run N sessions
///   --out <path>                 N=1 writes a recording; N>1 / seed exploration / --llm writes a full report JSON
///   --report-dir <dir>           representative recording drop directory (default <project>/playtest-report/); report body only references relative paths
///   --html <path>                render the report as a single-page human-readable self-contained HTML at this path (inline CSS/SVG, no external deps);
///                                representative recording still drops to --report-dir, HTML references it via a relative link. Single-session (N=1) does not emit HTML
///                                (it only produces one recording, no aggregatable report)
///
/// **Per-game view override** (design draft section 1, section 11 item 6): auto-loads the project root `playtest.json` (use if present,
/// otherwise default config=auto-derived view, behavior unchanged) — can pick components / rename / declare derived quantities (distance / alias / count) / give
/// greedy a target (drive a derived quantity toward min/max) / override terminal event names. Representative recordings of dimensions like stuck/runaway in the report
/// are no longer inlined into the report body; each is dropped as a separate json file into report-dir, and the report only references relative paths (clean readable body).
///
/// Behavior tiers (priority top-down):
/// - `--llm N>0`: swarm (sessions strategy-tier sessions) + N LLM-tier humanoid sessions, merged and aggregated into a report. LLM sessions'
///   notes go into `qualitative_notes` (subjective hints, pending human review), their chosen inputs go into recordings (replayable); the strategy-tier portion remains deterministic.
///
/// The other three tiers:
/// - With `--seed-recording`: **seed-style exploration** (design draft phase 3) — load the recording as a seed,
///   `perturb_plan` generates N mutation scripts (drop/swap/substitute/truncate rotation + truncate-then-random divergence),
///   `run_seed_swarm` runs them in parallel, aggregated report contains `ending_coverage` (which declared endings are unreachable). `--sessions`=number of mutations.
/// - Not given but `--sessions>1`: swarm — four strategies random+greedy+coverage+economy rotate × incrementing seed
///   (economy is specialized for finding numeric breakage in simulation/management games, report contains `numeric_breakage`: runaway economy / collapse / overflow).
///   **Projects that declare a goal** (playtest.json has goal) automatically replace the last few sessions (~25%) with lookahead search
///   (lookahead), so navigation/skill-based games actually get played and are not misreported as unbeatable by random strategies; without a declared goal the
///   default set is completely unchanged (backward compatible). No new option needed, default behavior automatically gets smarter.
/// - Not given and `--sessions=1`: single-session legacy behavior, produces one replayable recording.
///
/// Each session boots its own runtime inside its own thread and runs in parallel (QuickJS is not Send, runtime never crosses threads).
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
    // --horizon now refers to the beam search **depth** (how many frames to plan ahead); old flag name kept for backward compatibility, default 8.
    // depth=1 degrades to single-step lookahead (1-ply).
    let mut horizon: u64 = 8;
    // --beam beam width (how many best nodes per layer continue to expand), only used with --strategy lookahead, default 4.
    let mut beam: usize = 4;
    let mut seed: u64 = 0;
    let mut max_ticks: u64 = 600;
    let mut sessions: u64 = 1;
    let mut llm_sessions: u64 = 0;
    let mut out_path: Option<PathBuf> = None;
    let mut seed_recording: Option<PathBuf> = None;
    let mut report_dir: Option<PathBuf> = None;
    let mut html_path: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        let need = |key: &str| format!("{key} 缺少参数值");
        match args[i].as_str() {
            "--html" => {
                html_path = Some(PathBuf::from(args.get(i + 1).ok_or(need("--html"))?));
                i += 2;
            }
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
            "--beam" => {
                beam = args.get(i + 1).ok_or(need("--beam"))?.parse().map_err(|e| format!("--beam: {e}"))?;
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
                return Err(format!("未知选项 {other:?}。可用: --strategy --horizon --beam --seed --max-ticks --sessions --llm --seed-recording --out --report-dir --html"))
            }
        }
    }

    // Auto-load the project root playtest.json (use if present, otherwise default config=auto-derived view, behavior unchanged).
    // Parse errors report clearly with the path (vitric check style), do not silently skip.
    let config = PlaytestConfig::load(&dir)?.unwrap_or_default();
    // Authoritative clear events declared in the project manifest: gates.playthroughs[].must_emit. Script/LLM games
    // (e.g. echo's run-complete) have win events that are not in the generic default TerminalSpec; they are merged in via this,
    // otherwise they are misjudged as "no one can clear" and ending_coverage is empty. boot already loaded it internally; loading once more here to get
    // the manifest is cheap (just reading JSON), and saves us from threading must_emit all the way through boot.
    let manifest_must_emit: Vec<String> = match vitric_data::Project::load(&dir) {
        Ok(project) => project
            .manifest
            .gates
            .as_ref()
            .map(|g| g.playthroughs.iter().map(|p| p.must_emit.clone()).collect())
            .unwrap_or_default(),
        // If the manifest cannot be read, do not hard-error here (boot itself will report) — degrade to an empty set, equivalent to no declared gates.
        Err(_) => Vec::new(),
    };

    // Terminal spec: playtest.json's terminal override (default set if absent) takes effect first,
    // then the manifest must_emit is appended to the win set (additive dedup, does not replace the override result).
    let terminal = match &config.terminal {
        Some(ovr) => TerminalSpec::default().apply_override(ovr),
        None => TerminalSpec::default(),
    }
    .with_manifest_must_emit(&manifest_must_emit);
    // Representative recording drop directory: default <project>/playtest-report/. Report body only references relative paths, each recording is its own file.
    let report_dir = report_dir.unwrap_or_else(|| dir.join("playtest-report"));

    // Project name (for HTML title display): take the last segment of the project directory; if unavailable, fall back to "project".
    let project_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();

    // Unified report emission contract: externalize representative recordings to report-dir (backfill relative paths) → print clean JSON to stdout →
    // if --out, also save a full report JSON → (if --html given) write a human-readable HTML report to that path. Recordings are not inlined into
    // the report body (design draft section 11 item 6); HTML only references the representative recording via a relative link (path is backfilled).
    let emit_report = |mut report: vitric_playtest::Report,
                       out_path: &Option<PathBuf>|
     -> Result<(), String> {
        report.externalize_recordings(&report_dir)?;
        let json = serde_json::to_string_pretty(&report).expect("报告可序列化");
        if let Some(out) = out_path {
            std::fs::write(out, &json)
                .map_err(|e| format!("写报告 {} 失败: {e}", out.display()))?;
        }
        if let Some(html_out) = &html_path {
            let html = vitric_playtest::report_to_html(&report, &project_name);
            std::fs::write(html_out, &html)
                .map_err(|e| format!("写 HTML 报告 {} 失败: {e}", html_out.display()))?;
        }
        println!("{json}");
        Ok(())
    };

    // --llm N>0: additionally run N LLM humanoid sessions in the swarm (design draft phase 5). LLM sessions' notes go into the report's
    // qualitative_notes, their chosen inputs go into recordings (replayable). Install a real client (if VITRIC_LLM_* are not all set,
    // report a clear error directly, do not silently skip). LLM tier is inherently slow, run serially, then merge into the strategy-tier result set for aggregation.
    if llm_sessions > 0 {
        let client: Arc<dyn LlmClient> = Arc::new(
            vitric_cli::playtest_llm::PlaytestLlmClient::from_env()?,
        );
        let factory = || -> Result<(_, _, _), String> {
            let (sim, rt) = Runtime::boot(&dir)?;
            let engine = rt.rules.clone();
            Ok((sim, rt, engine))
        };
        // The same factory closure has to feed two functions (run_swarm takes it by value and is no longer usable afterward), so share a reference:
        // &F where F: Fn is still Fn + Sync, both sides use a reference, no duplicated boot logic.
        let factory_ref = &factory;
        // Strategy tier: sessions cheap-strategy sessions (rotation + incrementing seed), same contract as without --llm.
        // Projects with a declared goal automatically mix in a few lookahead sessions (default_plan checks has_goal internally).
        let plan = default_plan(sessions, seed, max_ticks, terminal.clone(), config.goal.is_some());
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        // With config: greedy drives toward config.goal, view is adjusted via include/exclude/relabel/derived quantities
        let mut results = run_swarm_with_config(factory_ref, &plan, &config, threads)?;
        // LLM tier: N humanoid sessions, results are merged into the same set fed to the aggregator. Goal description is assembled from config.goal (if present, gives the LLM a direction)
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

        // If endings are declared, also compute ending coverage (narrative projects especially need this)
        let (_, rt) = Runtime::boot(&dir)?;
        let report = aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &manifest_must_emit);
        return emit_report(report, &out_path);
    }

    // With --seed-recording: seed-style exploration (phase 3). Load the seed recording → perturb_plan generates
    // N mutations → ScriptedStrategy (truncate-then-random divergence) runs in parallel → aggregate, including ending_coverage.
    if let Some(seed_path) = &seed_recording {
        let rec_text = std::fs::read_to_string(seed_path)
            .map_err(|e| format!("读取种子录像 {} 失败: {e}", seed_path.display()))?;
        let seed_rec: Recording =
            serde_json::from_str(&rec_text).map_err(|e| format!("种子录像解析失败: {e}"))?;

        // Mutation count = --sessions (the 0th is the baseline = original seed). --seed is reused as the perturb PCG seed,
        // the truncated script's random divergence uses seed+1 to offset (different source from the perturb PCG).
        let plan = perturb_plan(&seed_rec, sessions as usize, seed);
        let factory = || -> Result<(_, _, _), String> {
            let (sim, rt) = Runtime::boot(&dir)?;
            let engine = rt.rules.clone();
            Ok((sim, rt, engine))
        };
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        // Seed replies follow the seed: perturbation only changes inputs, replies are injected back at their original ticks as-is (run_seed_swarm filters by truncation internally).
        let results = run_seed_swarm(
            factory,
            &plan,
            &seed_rec.replies,
            max_ticks,
            terminal.clone(),
            seed.wrapping_add(1),
            threads,
        )?;

        // Ending coverage needs to scan the rule-declared ending set, so boot a separate read-only Engine to feed the aggregator
        let (_, rt) = Runtime::boot(&dir)?;
        let report = aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &manifest_must_emit);
        return emit_report(report, &out_path);
    }

    // N>1: swarm batch + aggregated report
    if sessions > 1 {
        // Plan: four strategies rotate × incrementing seed, to make up N sessions. Each spec carries (strategy, seed, max_ticks, terminal),
        // a session's outcome is determined solely by spec + config (iron law of determinism) — serial and parallel runs give the same result.
        // For projects with a declared goal (playtest.json), default_plan automatically replaces the last few sessions with lookahead search,
        // so navigation/skill-based games are not misreported as unbeatable by random strategies; without a declared goal the default set is completely unchanged (backward compatible).
        let plan = default_plan(sessions, seed, max_ticks, terminal.clone(), config.goal.is_some());

        // Factory closure: each worker thread calls it inside its own thread to boot a fresh runtime.
        // Returns (Sim, Runtime, Engine) — Engine is an assembly-time stateless copy (derive Clone),
        // running run_session needs to mutably borrow logic and immutably borrow engine at the same time, so engine must be a separate copy.
        let factory = || -> Result<(_, _, _), String> {
            let (sim, rt) = Runtime::boot(&dir)?;
            let engine = rt.rules.clone();
            Ok((sim, rt, engine))
        };

        // Thread count defaults to machine parallelism (run_swarm internally takes min(plan, cpu))
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        // With config: greedy drives toward config.goal, view is adjusted via include/exclude/relabel/derived quantities.
        // If endings are declared, compute ending coverage (scan the declared set using the config-overridden terminal).
        let results = run_swarm_with_config(factory, &plan, &config, threads)?;
        let (_, rt) = Runtime::boot(&dir)?;
        let report = aggregate_with_endings_and_declared(&results, &rt.rules, &terminal, &manifest_must_emit);
        return emit_report(report, &out_path);
    }

    // N=1: single-session legacy behavior (produces a replayable recording).
    let out = out_path.unwrap_or_else(|| dir.join("playtest-session.json"));
    // Boot a fresh (sim, runtime) pair: recording a replayable recording must start from a cold boot
    let (mut sim, mut rt) = Runtime::boot(&dir)?;
    // Single session also uses config: terminal override + derived-quantity view (greedy/lookahead finds the target).
    let cfg = SessionConfig { max_ticks, seed, terminal: terminal.clone(), playtest: config.clone(), ..Default::default() };
    // run_session needs to mutably borrow logic(rt) and immutably borrow engine(rt.rules) at the same time — cannot borrow the same object both ways.
    // Engine is an assembly-time stateless copy; cloning a read-only copy to pass in is the cleanest approach (see Engine's derive comment).
    let engine = rt.rules.clone();

    // lookahead uses a beam-search rolling planner (each real tick builds a depth D=horizon, beam width W=beam search tree to pick the best first step),
    // the rest use the ordinary strategy run_session. Cost note: beam search cost per real tick ≤ W×(|candidate actions|+1)×D speculative steps,
    // far more expensive than cheap strategies, so only enabled with explicit --strategy lookahead, **not in swarm default rotation** (swarm runs hundreds or thousands of sessions).
    let result = if strategy_name == "lookahead" {
        run_session_lookahead(&mut sim, &mut rt, &engine, &cfg, &LookaheadConfig { depth: horizon, beam_width: beam })?
    } else {
        // economy is also selectable — single-session stress one strategy and see how it runs.
        // greedy drives toward the target when config.goal is present (playtest.json declares derived quantity + goal), otherwise degrades to random.
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

/// `vitric gate`: delivery gate. The report (same JSON for humans and machines) always goes to stdout;
/// all gates must pass to exit 0 — "delivery complete" is decided here, not by agent self-report.
///
/// Gate set: check + clear recording replay + assertion set (see gate::run), plus **optional playtest gate** — only runs when the manifest
/// declares `gates.playtest`: actually runs a playtest swarm (verifiable reproducibility), aggregates a report, checks each
/// declared contract (can clear / soft-lock count / unreachable endings / lazy actions / numeric breakage), turning "auto-clear the floor" into
/// a delivery contract. Not declared = this gate is not run (backward compatible, existing gate behavior unchanged).
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

/// `vitric team`: team coordination blackboard. The status tool is not a gate — after the report goes to stdout it **always succeeds and exits**,
/// blockages are only blocking hints; delivery verdict is up to `vitric gate`.
fn cmd_team(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("team 缺少项目目录参数")?;
    let report = vitric_cli::team::run(&PathBuf::from(dir))?;
    println!("{}", serde_json::to_string_pretty(&report).expect("报告可序列化"));
    Ok(())
}

/// `vitric turf`: turf enforcement. Report goes to stdout in the same style as gate; any out-of-bounds file means exit 1.
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
    // GPU is a window presentation path, no headless form — choosing gpu means opening a window
    if matches!(renderer, window::Renderer::Gpu) {
        windowed = true;
    }

    let dir = PathBuf::from(dir);
    let project = Project::load(&dir).map_err(|r| r.to_string())?;
    let (mut sim, mut rt) = Runtime::boot(&dir)?;
    let mut dispatcher = Dispatcher::new(project.schema.clone());
    dispatcher.load_assets(&dir.join("assets"))?;
    // Manifest declares a font: load it at startup (missing/corrupt errors immediately), all Text goes through the vector path
    if let Some(font_rel) = &project.manifest.font {
        dispatcher.load_font(&dir.join(font_rel))?;
    }
    dispatcher.set_budgets(project.manifest.budgets.clone());
    dispatcher.ctl.speed = speed;

    // Player saves: slots are fixed under <project>/saves/, convention events (save-game/load-game),
    // save/* RPC and --load all share this one SaveStore — same code path, same validation
    let saves = SaveStore::new(&dir, &project.manifest.name);
    if let Some(slot) = &load_slot {
        // A recording must be bit-for-bit replayable from a cold boot of the project data; a session continued from a save is not a cold boot,
        // the resulting recording would diverge at the very start in vitric replay — explicitly mutually exclusive, do not produce a broken recording
        if record_path.is_some() {
            return Err(
                "--load 与 --record 互斥：录像要求从项目数据冷启动可重放，从存档续玩的\
                 一局录不出可重放的录像。要录像就从头跑，要续玩就去掉 --record"
                    .to_string(),
            );
        }
        // Restore before tick 0: the start event will not be re-emitted (the save's tick 0 has long passed)
        let snap = saves.read(slot)?;
        sim.restore(&snap, &mut rt).map_err(|e| format!("--load {slot}: 存档恢复失败: {e}"))?;
    }
    dispatcher.set_save_store(saves);

    if record_path.is_some() {
        sim.start_recording();
    }

    let server = ControlServer::start(port)?;
    // Audio: legitimate degradation in environments without a sound card, banner states this clearly
    let (mut audio_sink, audio_status) = match audio::Audio::open(dir.join("sounds")) {
        Ok(a) => (Some(a), "ok".to_string()),
        Err(e) => (None, format!("disabled: {e}")),
    };
    // Runtime LLM: only enabled when env vars are all set; not configured is also a legitimate state (llm-ask receives an explicit llm-error)
    let mut llm = Llm::from_env();
    // Startup banner goes to stdout as a single-line JSON: AI parses the port, humans can also read it
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

    // Main loop: fixed step + speed multiplier + frame-boundary handling of control commands
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
            // Bounded run: full speed, no sleeping
            step_once(&mut sim, &mut rt, &mut dispatcher, &mut audio_sink, &mut llm)?;
            continue;
        }

        // Real-time run: accumulate step count by wall clock (wall clock only decides "how many steps", never enters the simulation)
        let now = Instant::now();
        acc += now.duration_since(last).as_secs_f64() * dispatcher.ctl.speed;
        last = now;
        let mut budget = 8; // per-frame catch-up step cap, prevents spiral of death
        while acc >= DT && budget > 0 {
            step_once(&mut sim, &mut rt, &mut dispatcher, &mut audio_sink, &mut llm)?;
            acc -= DT;
            budget -= 1;
        }
        if budget == 0 {
            acc = 0.0; // drop frames if behind, do not chase
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
    // LLM: capture this tick's llm-ask, queue requests, then harvest completed replies and inject them into the next tick
    llm::handle_ask_events(llm, &observed, sim);
    llm::pump_replies(llm, sim);
    dispatcher.record_events(report.tick, &observed);
    // Save convention events: save-game writes to disk (pure output side effect, like audio); load-game happens here —
    // frame boundary, outside the simulation — jump back to the save point as a whole. Failures are reported to stderr in structured form, do not crash the game
    for error in dispatcher.handle_save_load_events(&observed, sim, rt) {
        eprintln!("{}", serde_json::json!({"save_error": error}));
    }
    for failure in dispatcher.check_assertions(sim) {
        // Assertion failures are reported to stderr in real time (one structured line), and also stored into assert/failures
        eprintln!("{}", serde_json::json!({"assert_failure": failure}));
    }
    Ok(())
}
