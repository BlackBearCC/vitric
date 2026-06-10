//! vitric — 命令行入口。
//!
//! 子命令：
//! - `vitric check <项目目录>`            校验项目（schema/场景/规则/脚本），出报告
//! - `vitric run <项目目录> [选项]`        无头运行 + AI 控制面
//! - `vitric replay <项目目录> <录像.json>` 重放录像并校验确定性
//!
//! run 选项：
//!   --port <N>     控制面端口（默认 6173，0=自动分配）
//!   --speed <X>    初始倍速（默认 1.0）
//!   --ticks <N>    跑满 N tick 后自动退出（CI/脚本用，全速不限速）
//!   --record <文件> 退出时把录像写到文件

use std::path::PathBuf;
use std::time::{Duration, Instant};

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
        _ => {
            eprintln!("用法: vitric <check|run|replay> <项目目录> [选项]\n详见 vitric 仓库 docs/");
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

fn cmd_run(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("run 缺少项目目录参数")?;
    let mut port: u16 = 6173;
    let mut speed: f64 = 1.0;
    let mut max_ticks: Option<u64> = None;
    let mut record_path: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        let need = |key: &str| format!("{key} 缺少参数值");
        match args[i].as_str() {
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
            other => return Err(format!("未知选项 {other:?}。可用: --port --speed --ticks --record")),
        }
    }

    let dir = PathBuf::from(dir);
    let project = Project::load(&dir).map_err(|r| r.to_string())?;
    let (mut sim, mut rt) = Runtime::boot(&dir)?;
    let mut dispatcher = Dispatcher::new(project.schema.clone());
    dispatcher.ctl.speed = speed;

    if record_path.is_some() {
        sim.start_recording();
    }

    let server = ControlServer::start(port)?;
    // 启动横幅走 stdout 单行 JSON：AI 解析端口，人也看得懂
    println!(
        "{}",
        serde_json::json!({
            "vitric": "running",
            "project": project.manifest.name,
            "control": format!("http://127.0.0.1:{}/rpc", server.port),
            "seed": project.manifest.seed,
        })
    );

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
            step_once(&mut sim, &mut rt, &mut dispatcher)?;
            continue;
        }

        // 实时跑：按墙钟攒步数（墙钟只决定「跑几步」，永远不进模拟）
        let now = Instant::now();
        acc += now.duration_since(last).as_secs_f64() * dispatcher.ctl.speed;
        last = now;
        let mut budget = 8; // 单帧补步上限，防螺旋死亡
        while acc >= DT && budget > 0 {
            step_once(&mut sim, &mut rt, &mut dispatcher)?;
            acc -= DT;
            budget -= 1;
        }
        if budget == 0 {
            acc = 0.0; // 跟不上就丢，不追帧
        }
        std::thread::sleep(Duration::from_millis(1));
    }

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

fn step_once(
    sim: &mut vitric_sim::Sim,
    rt: &mut Runtime,
    dispatcher: &mut Dispatcher,
) -> Result<(), String> {
    let report = sim.step(rt).map_err(|e| e.to_string())?;
    dispatcher.record_events(report.tick, &report.events);
    dispatcher.record_events(report.tick, &rt.drain_observed());
    for failure in dispatcher.check_assertions(sim) {
        // 断言失败实时上报到 stderr（结构化一行），同时存进 assert/failures
        eprintln!("{}", serde_json::json!({"assert_failure": failure}));
    }
    Ok(())
}
