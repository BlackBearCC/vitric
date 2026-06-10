//! Runtime — 把项目数据、规则引擎、脚本引擎装配成一台能跑的游戏。

use std::path::Path;

use serde_json::Value;

use vitric_data::Project;
use vitric_rules::{Engine, Event, RuleSet, ScriptCall};
use vitric_script::ScriptEngine;
use vitric_sim::{GameLogic, Pcg32, Sim};

/// 游戏逻辑装配体：规则为正门，脚本兜复杂逻辑。
///
/// 每 tick 的执行顺序（固定，确定性的一部分）：
/// 1. 规则消化本 tick 事件（输入/碰撞/上一 tick 脚本发的事件）；
/// 2. 规则产生的 `call` 逐个调脚本函数；
/// 3. 脚本系统按注册顺序各跑一遍；
/// 4. 脚本 emit 的事件进下一 tick 的收件箱。
pub struct Runtime {
    pub rules: Engine,
    pub scripts: ScriptEngine,
    /// 项目根目录（热重载从这里重读磁盘）。
    root: Option<std::path::PathBuf>,
    /// 脚本上一 tick 发出的事件，本 tick 交给规则。
    carryover: Vec<Event>,
    /// 本 tick 规则/脚本 emit 的全部事件副本，主循环取走送进控制面事件日志。
    observed: Vec<Event>,
}

impl Runtime {
    /// 从已加载的项目装配运行时（规则语义校验、脚本求值都在这里发生）。
    pub fn build(project: &Project) -> Result<Runtime, String> {
        // 规则：多个文件合并成一个规则集
        let mut all = RuleSet::default();
        for (file, doc) in &project.rules {
            let set = RuleSet::parse(doc, file).map_err(|r| r.to_string())?;
            all.rules.extend(set.rules);
        }
        let rules = Engine::new(all, project.schema.clone());

        // 脚本
        let mut scripts = ScriptEngine::new(project.schema.clone()).map_err(|e| e.to_string())?;
        for (file, src) in &project.scripts {
            scripts.load(file, src).map_err(|e| e.to_string())?;
        }

        Ok(Runtime { rules, scripts, root: None, carryover: Vec::new(), observed: Vec::new() })
    }

    /// 加载项目 + 装配 + 实例化入口场景，给出可以直接跑的 (Sim, Runtime)。
    pub fn boot(dir: &Path) -> Result<(Sim, Runtime), String> {
        let project = Project::load(dir).map_err(|r| r.to_string())?;
        let mut runtime = Runtime::build(&project)?;
        runtime.root = Some(dir.to_path_buf());
        let mut sim = Sim::new(project.manifest.seed);
        vitric_data::instantiate_scene(project.entry_scene(), &project.schema, &mut sim.world)
            .map_err(|r| r.to_string())?;
        Ok((sim, runtime))
    }
}

impl GameLogic for Runtime {
    fn on_tick(
        &mut self,
        world: &mut vitric_ecs::World,
        events: Vec<Event>,
        rng: &mut Pcg32,
        tick: u64,
    ) -> Result<(), String> {
        let mut inbox = std::mem::take(&mut self.carryover);
        inbox.extend(events);

        // 1. 规则
        let out = self.rules.process_tick(world, inbox).map_err(|e| e.to_string())?;
        self.observed.extend(out.emitted);

        // 2. 规则 -> 脚本函数调用
        for ScriptCall { function, args, self_entity } in out.calls {
            let so = self
                .scripts
                .call_fn(&function, &args, self_entity, world, rng, tick)
                .map_err(|e| e.to_string())?;
            self.observed.extend(so.events.iter().cloned());
            self.carryover.extend(so.events);
        }

        // 3. 脚本系统
        let so = self.scripts.run_systems(world, rng, tick).map_err(|e| e.to_string())?;
        self.observed.extend(so.events.iter().cloned());
        self.carryover.extend(so.events);

        Ok(())
    }

    /// 取走本 tick 规则/脚本发出的事件（控制面观测用）。
    fn drain_observed(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.observed)
    }

    /// 热重载：从磁盘重读规则+脚本，整体重建后原子替换；
    /// 任何一步失败都保持旧逻辑不动（不会半死不活）。
    /// 注意：schema/场景改动不在热重载范围（它们定义世界形状，改了要重启）。
    fn reload(&mut self) -> Result<serde_json::Value, String> {
        let root = self.root.clone().ok_or("该运行时没有项目目录，无法热重载")?;
        let project = Project::load(&root).map_err(|r| r.to_string())?;
        let fresh = Runtime::build(&project)?;
        self.rules = fresh.rules;
        self.scripts = fresh.scripts;
        // carryover 里是纯数据事件，跨重载安全，保留不丢
        Ok(serde_json::json!({
            "reloaded": ["rules", "scripts"],
            "note": "schema/场景的改动不走热重载，需要重启进程",
            "rules": self.rules.rules.rules.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
            "systems": self.scripts.systems.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
            "fns": self.scripts.fns.clone(),
        }))
    }
}

/// `vitric check`：只验数据不开跑。返回人类/AI 可读的完整报告。
pub fn check(dir: &Path) -> Result<Value, String> {
    let project = Project::load(dir).map_err(|r| r.to_string())?;
    let runtime = Runtime::build(&project)?;
    // 实例化入口场景到一次性世界，把落地期错误也暴露出来
    let mut sim = Sim::new(project.manifest.seed);
    vitric_data::instantiate_scene(project.entry_scene(), &project.schema, &mut sim.world)
        .map_err(|r| r.to_string())?;
    // 素材：加载即校验（坏图/超尺寸），再查场景引用的图都在
    let assets = vitric_render::Assets::load_dir(&dir.join("assets"))?;
    let mut missing = Vec::new();
    for id in sim.world.query(&["Sprite"]) {
        if let Ok(image) = sim.world.get_field(id, "Sprite.image") {
            if let Some(name) = image.as_str().filter(|s| !s.is_empty()) {
                if assets.image(name).is_none() {
                    missing.push(format!(
                        "实体 {}{} 引用了不存在的素材 {name:?}",
                        id,
                        sim.world.name_of(id).map(|n| format!("({n})")).unwrap_or_default(),
                    ));
                }
            }
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "素材引用校验失败:\n  {}\n现有素材: [{}]",
            missing.join("\n  "),
            assets.names().join(", ")
        ));
    }
    Ok(serde_json::json!({
        "project": project.manifest.name,
        "scenes": project.scenes.keys().collect::<Vec<_>>(),
        "rules": runtime.rules.rules.rules.iter().map(|r| r.id.clone()).collect::<Vec<_>>(),
        "systems": runtime.scripts.systems.iter().map(|s| serde_json::json!({
            "name": s.name, "query": s.query, "writes": s.writes,
        })).collect::<Vec<_>>(),
        "fns": runtime.scripts.fns,
        "entities": sim.world.entities().len(),
        "assets": {
            "count": assets.count(),
            "decoded_kb": assets.total_bytes() / 1024,
        },
        "initial_hash": format!("{:#018x}", sim.world.state_hash()),
    }))
}
