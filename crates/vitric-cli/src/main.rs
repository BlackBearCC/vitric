//! vitric — 命令行入口。
//!
//! 子命令：
//! - `vitric check <项目目录>`            校验项目（schema/场景/规则/脚本），出报告
//! - `vitric run <项目目录> [选项]`        无头运行 + AI 控制面
//! - `vitric replay <项目目录> <录像.json>` 重放录像并校验确定性
//! - `vitric gate <项目目录>`              交付门禁：check + 通关录像重放 + 断言集，全过才出证书
//! - `vitric assets <项目目录> [选项]`      全项目 PNG 统一色板（AI 出图规整成一个调）
//!
//! assets 选项：
//!   --colors <N>     色板颜色数（默认 32）
//!   --height <H>     高于 H 的图先最近邻缩到高 H（保持宽高比）
//!   --palette-lock   跳过提取，按项目已有 palette.json 量化（新素材入伙老色板）
//!   --normals        给没有 _n 配对的 PNG 生成法线贴图（程序化，确定性；与色板选项互斥）
//!   --normals-ai     法线生成改走豆包 Ark Seedream 图生图（需要环境变量 ARK_API_KEY；
//!                    模型 VITRIC_NORMALS_MODEL，默认 doubao-seedream-5-0-260128）
//!
//! run 选项：
//!   --port <N>       控制面端口（默认 6173，0=自动分配）
//!   --speed <X>      初始倍速（默认 1.0）
//!   --ticks <N>      跑满 N tick 后自动退出（CI/脚本用，全速不限速）
//!   --record <文件>   退出时把录像写到文件
//!   --renderer <gpu|cpu> 窗口呈现路径（默认 cpu=softbuffer；gpu=wgpu，自带开窗）
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
use vitric_control::{ControlServer, Dispatcher};
use vitric_data::Project;
use vitric_sim::{Recording, DT};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(|s| s.as_str()) {
        Some("check") => cmd_check(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        Some("replay") => cmd_replay(&args[1..]),
        Some("gate") => cmd_gate(&args[1..]),
        Some("assets") => vitric_cli::assets_cmd::run(&args[1..]),
        _ => {
            eprintln!("用法: vitric <check|run|replay|gate|assets> <项目目录> [选项]\n详见 vitric 仓库 docs/");
            std::process::exit(2);
        }
    };
    if let Err(message) = result {
        // 错误走 stderr 且保持结构化前缀，AI 和人都好解析
        eprintln!("vitric 错误: {message}");
        std::process::exit(1);
    }
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

/// `vitric gate`：交付门禁。报告（人和机器同一份 JSON）永远打到 stdout；
/// 全部门禁 pass 才退出 0——"交付完成"由这里裁决，不由 agent 自述。
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

fn cmd_run(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("run 缺少项目目录参数")?;
    let mut port: u16 = 6173;
    let mut speed: f64 = 1.0;
    let mut max_ticks: Option<u64> = None;
    let mut record_path: Option<String> = None;
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
            other => return Err(format!("未知选项 {other:?}。可用: --window --renderer --port --speed --ticks --record")),
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
    for failure in dispatcher.check_assertions(sim) {
        // 断言失败实时上报到 stderr（结构化一行），同时存进 assert/failures
        eprintln!("{}", serde_json::json!({"assert_failure": failure}));
    }
    Ok(())
}
