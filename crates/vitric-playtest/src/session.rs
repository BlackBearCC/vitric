//! 单局试玩会话：派生 Scene View → 策略选动作 → 注入 → 步进，直到通关/死亡/超时，
//! 全程录像。一局 = 一段可重放可认证的确定性录像（设计稿四节）。
//!
//! **接口取向说明**：设计稿写的是 `run_session(project_dir, ...)`，但装配运行时的
//! `Runtime::boot` 住在 vitric-cli（cli 依赖 playtest，playtest 不能反向依赖 cli，否则成环）。
//! 所以这里把 boot 留给调用方（CLI 的 cmd_playtest），run_session 接已 boot 好的
//! `(Sim, GameLogic, Engine)`——职责更纯：playtest 只管「喂视图、跑循环、出录像」，
//! 不认识项目目录怎么装配。Engine 单独传是因为派生动作词汇要读规则，而它被 GameLogic
//! 装配体私有持有，拿不到引用。

use std::collections::BTreeMap;

use vitric_sim::{GameLogic, InputRecord, Recording, ReplyRecord, Sim, TICKS_PER_SECOND};
use vitric_rules::Engine;
use serde_json::Value;

use crate::scene_view::{Outcome, SceneView, TerminalSpec};
use crate::strategy::{PlaytestNote, Strategy};

/// 一个数值字段在整局里的轨迹摘要（**增量统计**，不存每 tick 全表）。
///
/// key（在 `numeric_summary` 的 BTreeMap 里）= 数值叶子的路径，形如
/// `hero/Resources.gold`（实体名或 id + 「组件.字段...」）。每 tick 从当前 observation
/// 取所有数值叶子，对各自的 NumericStat 做 O(1) 更新——不保留逐 tick 历史。
///
/// 为什么这么设计：模拟经营找数值崩（设计稿五节「数值崩」）靠的是「这个字段最后跑成多大/
/// 有没有归零/是不是只增不减」这些**摘要**信号，不需要逐 tick 曲线。增量统计把内存压到
/// O(数值字段数) 而非 O(字段数 × tick 数)，几千局也扛得住（设计稿九节性能预算）。
///
/// **不进哈希、不进录像**：和 state_trace/fired_events 一样，是「这局怎么跑的」旁观记录，
/// 是录像的纯函数派生（同录像必出同摘要），自然满足确定性铁律。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NumericStat {
    /// 这个字段第一次被观测到时的值（首帧基线）。
    pub first: f64,
    /// 最后一次观测到的值（末帧——「跑成多大/崩到多小」看它）。
    pub last: f64,
    /// 整局观测到的最小值。
    pub min: f64,
    /// 整局观测到的最大值。
    pub max: f64,
    /// 整局**只增不减**（每次新观测都 ≥ 上一次）——经济跑飞的特征之一。
    pub monotonic_up: bool,
    /// 整局曾触达过 0（资源归零——崩盘软锁的特征之一）。
    pub hit_zero: bool,
    /// 曾观测到非有限值（inf / nan）——数值溢出/除零的硬信号，单独标。
    pub non_finite: bool,
}

impl NumericStat {
    /// 用首次观测值初始化。
    fn start(v: f64) -> NumericStat {
        let finite = v.is_finite();
        NumericStat {
            first: v,
            last: v,
            min: v,
            max: v,
            monotonic_up: true,
            hit_zero: finite && v == 0.0,
            non_finite: !finite,
        }
    }

    /// 增量并入一次新观测（O(1)）：刷新 last/min/max、维护单调/归零/非有限标记。
    fn observe(&mut self, v: f64) {
        if !v.is_finite() {
            // 非有限值单独标，不污染 min/max（NaN 比较全 false 会破坏单调判定）
            self.non_finite = true;
            self.last = v;
            return;
        }
        // 单调：只要某次比上一次小，就不再是「只增不减」
        if v < self.last {
            self.monotonic_up = false;
        }
        self.last = v;
        if v < self.min {
            self.min = v;
        }
        if v > self.max {
            self.max = v;
        }
        if v == 0.0 {
            self.hit_zero = true;
        }
    }
}

/// 从一份 observation（SceneView 投影的 JSON）抽出所有数值叶子，并入 summary（增量）。
///
/// key 路径：`<实体名或id>/<组件>.<字段>[.<子字段>...]`。遍历 observation.entities，
/// 每个实体取 name（无名退化成 id），再钻它的 components 树，遇到数值（含 bool 折成 0/1?
/// 不——只收真数值 number，bool 是 flag 不是数值，不进数值崩分析）就更新对应 NumericStat。
///
/// **确定性**：observation 的实体是按槽位序、组件/字段是 serde_json Map（BTreeMap 序），
/// 遍历序固定；BTreeMap 聚合输出也固定。O(数值叶子数)/tick，不存历史（设计稿九节）。
fn collect_numeric_leaves(observation: &Value, summary: &mut BTreeMap<String, NumericStat>) {
    let Some(entities) = observation.get("entities").and_then(|v| v.as_array()) else {
        return;
    };
    for ent in entities {
        // 实体标识：优先人话名，无名退化成 id（scene_view 保证 id 一定在）
        let label = ent
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| ent.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()));
        let Some(label) = label else { continue };
        let Some(comps) = ent.get("components").and_then(|v| v.as_object()) else {
            continue;
        };
        for (cname, cval) in comps {
            // 路径前缀：实体/组件，字段名在递归里接上
            let prefix = format!("{label}/{cname}");
            walk_numeric(&prefix, cval, summary);
        }
    }
}

/// 递归钻一个组件值，把数值叶子并入 summary。path 是「到这一层」的路径前缀。
fn walk_numeric(path: &str, value: &Value, summary: &mut BTreeMap<String, NumericStat>) {
    match value {
        Value::Number(n) => {
            // 只收真数值；整数也转 f64（数值崩看量级，i64/f64 统一成一个标尺）
            if let Some(f) = n.as_f64() {
                upsert(summary, path, f);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                let child = format!("{path}.{k}");
                walk_numeric(&child, v, summary);
            }
        }
        Value::Array(arr) => {
            // 数组按下标编路径（如 inventory.0 / inventory.1），递归同样规则
            for (i, v) in arr.iter().enumerate() {
                let child = format!("{path}.{i}");
                walk_numeric(&child, v, summary);
            }
        }
        // bool/string/null 不是数值，跳过（bool 是 flag，不进数值崩分析）
        _ => {}
    }
}

/// 有则增量并入、无则以首值初始化（增量统计的 upsert）。
fn upsert(summary: &mut BTreeMap<String, NumericStat>, key: &str, v: f64) {
    summary
        .entry(key.to_string())
        .and_modify(|s| s.observe(v))
        .or_insert_with(|| NumericStat::start(v));
}

/// 一局的配置。
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// 跑满这么多 tick 还没终止就判 Timeout。
    pub max_ticks: u64,
    /// 策略 PCG 的播种（同 seed + 同策略 + 同起点 = 同一局）。
    pub seed: u64,
    /// 哪些事件算终止。
    pub terminal: TerminalSpec,
    /// 每游戏视图覆盖（设计稿一节「自动推 + 可选覆盖」）。默认=空配置（自动推，行为不变）。
    /// session 用它走 `SceneView::derive_with_config`——策略/LLM 看到 include/exclude/relabel/
    /// 派生量调整后的视图（尤其 greedy 靠派生量找目标）。**仍是纯投影**，不进哈希/录像。
    pub playtest: crate::config::PlaytestConfig,
    /// 种子录像里要按 tick 重放的外部回复（LLM 内容等）。默认空=这局没有外部回复（行为不变）。
    /// 种子式探索专用：策略只复现/扰动**输入**，但靠回复才走到的结局得把回复也按原 tick 注回去
    /// ——和 `Sim::replay` 同口径（输入在前、回复在后、再 step），否则基线复现不出靠回复通关的结局。
    /// 截断发散时，调用方只把截断点之前的回复传进来（截断后是 random 发散，没有种子回复）。
    pub seed_replies: Vec<ReplyRecord>,
}

impl Default for SessionConfig {
    fn default() -> SessionConfig {
        SessionConfig {
            max_ticks: 600,
            seed: 0,
            terminal: TerminalSpec::default(),
            playtest: crate::config::PlaytestConfig::default(),
            seed_replies: Vec::new(),
        }
    }
}

/// 一局的结果：结局 + 用了多少 tick + 这局的录像（可重放）+ 轻量遥测。
///
/// 遥测（state_trace/fired_events）只供聚合器分析，**不进录像、不进哈希**——
/// 它是「这局怎么跑的」的旁观记录，不是「这局是什么」的权威状态。确定性铁律：
/// 同 (策略,seed,起点) 跑出的录像逐字节一致，遥测是录像的纯函数派生，自然也一致。
#[derive(Debug, Clone)]
pub struct SessionResult {
    pub outcome: Outcome,
    pub ticks: u64,
    pub recording: Recording,
    /// 每 tick step 后的世界状态哈希（`world.state_hash()`，已优化过、便宜）。
    /// 长度 = 实际跑的 tick 数；是「状态变没变」的权威信号，软锁聚类靠它判冻结。
    pub state_trace: Vec<u64>,
    /// 整局出现过的事件名（去重，首次出现序）：StepReport.events + logic.drain_observed()。
    /// 用来判「哪些终止/里程碑事件被触发过」「哪些 input 动作引发过规则响应」。
    pub fired_events: Vec<String>,
    /// 数值遥测：每个数值字段路径 → 整局轨迹摘要（增量统计，不存每 tick 全表）。
    /// 给聚合器逮经济跑飞/崩盘/溢出（设计稿五节「数值崩」）。不进哈希/录像。
    pub numeric_summary: BTreeMap<String, NumericStat>,
    /// LLM 档定性 note（清晰度/连续性/选择有效性，设计稿五节「LLM 定性 note」）。
    /// 只有 LLM 策略会产（廉价策略档 drain_notes 默认空）。**不进哈希/录像**——
    /// 它是「LLM 这局看着怎么样」的旁观主观提示，和别的遥测同级，不影响确定性。
    pub notes: Vec<PlaytestNote>,
}

/// 跑一局。`sim`/`logic` 必须是刚 boot 出来、还在 tick 0 的全新一对（要录可重放的录像，
/// 必须从冷启动起录——和 vitric run 的 --record 同一条约束）。`engine` 用来派生动作词汇。
///
/// 循环（每 tick）：派生 Scene View → 若已 done 跳出 → 策略选动作 → inject_input →
/// step → 扫本 tick 事件命中终止则记 outcome 跳出 → 到 max_ticks 记 Timeout。
pub fn run_session(
    sim: &mut Sim,
    logic: &mut dyn GameLogic,
    engine: &Engine,
    strategy: &mut dyn Strategy,
    cfg: &SessionConfig,
) -> Result<SessionResult, String> {
    if sim.is_recording() {
        return Err("run_session 要自己开录：传进来的 sim 不该已在录像中".to_string());
    }
    sim.start_recording();

    // 遥测累加器：state_trace 每 tick 一条，fired_events 去重（用 set 判重 + vec 保序），
    // numeric_summary 增量统计（每 tick 从 observation 取数值叶子更新，不存历史）
    let mut state_trace: Vec<u64> = Vec::new();
    let mut numeric_summary: BTreeMap<String, NumericStat> = BTreeMap::new();
    let mut fired_events: Vec<String> = Vec::new();
    let mut seen_events: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut note_events = |names: &mut dyn Iterator<Item = &str>| {
        for name in names {
            if seen_events.insert(name.to_string()) {
                fired_events.push(name.to_string());
            }
        }
    };

    // LLM 档定性 note 累加器（每 tick 决策后从策略收，非 LLM 策略 drain 永远空）。
    // 不进哈希/录像——它是旁观主观提示，确定性铁律不约束它（LLM 局本就不要求跨次复现）。
    let mut notes: Vec<PlaytestNote> = Vec::new();

    let mut outcome = Outcome::Timeout;
    while sim.tick < cfg.max_ticks {
        // Scene View 是纯投影：只读世界/规则，绝不改 world、不进哈希。
        // 带 config 派生：策略看到 include/exclude/relabel/派生量调整后的视图（greedy 靠派生量找目标）。
        let view = SceneView::derive_with_config(&sim.world, engine, &cfg.terminal, &cfg.playtest);
        if let Some(done) = view.done {
            // 理论上 done 由 step 后扫事件判（见 SceneView::derive 注释），
            // 这里保留分支以防后续阶段让 derive 也能判静态终止。
            outcome = done;
            break;
        }

        // 策略选动作 → 注入（None=本 tick 不操作）
        if let Some(action) = strategy.choose(&view) {
            sim.inject_input(&action.action, &action.phase);
        }
        // 种子回复：和 Sim::replay 同口径——输入注完再注本 tick 的种子回复（顺序固定才逐位一致）。
        // 非种子路径 seed_replies 为空，这层是零开销 no-op，行为不变。
        for reply in &cfg.seed_replies {
            if reply.tick == sim.tick {
                sim.inject_reply(&reply.name, reply.data.clone());
            }
        }
        // 决策后收 note：LLM 策略在 choose 里可能产了 note，每 tick 取走累积
        // （取走即清空，避免重复）。非 LLM 策略默认返空，零开销。
        notes.append(&mut strategy.drain_notes());

        let report = sim.step(logic).map_err(|e| e.to_string())?;

        // 遥测（step 后采）：state_hash 是状态指纹（已优化、便宜，不自己序列化整世界），
        // 事件名累进去重集。遥测只读、不回写世界，不影响录像/哈希。
        state_trace.push(sim.world.state_hash());
        // 数值遥测：step 后对当前世界投影一份观测，抽数值叶子增量并入摘要。
        // 用同款带 config 投影（剔装饰、按槽位序、含派生量），保证 key 路径与策略所见一致；
        // 只取 observation（不用 actions），增量更新 O(数值叶子数)，不存逐 tick 历史。
        let post = SceneView::derive_with_config(&sim.world, engine, &cfg.terminal, &cfg.playtest);
        collect_numeric_leaves(&post.observation, &mut numeric_summary);
        note_events(&mut report.events.iter().map(|e| e.name.as_str()));

        // 扫本 tick 发给逻辑层的事件 + 逻辑层 emit 的事件，命中终止就收
        if let Some(o) = scan_terminal(report.events.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
        let emitted = logic.drain_observed();
        note_events(&mut emitted.iter().map(|e| e.name.as_str()));
        if let Some(o) = scan_terminal(emitted.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
    }

    // 退出循环（done/Timeout）后再收一次 note：LLM 可能在最后一个决策 tick 吐了
    // 还没被 drain 的 note（如「这关到这儿就卡住了，看不懂下一步」）。
    notes.append(&mut strategy.drain_notes());

    let recording = sim.stop_recording().expect("刚 start_recording 过");
    Ok(SessionResult {
        outcome,
        ticks: sim.tick,
        recording,
        state_trace,
        fired_events,
        numeric_summary,
        notes,
    })
}

/// 前瞻规划器配置（设计稿：技巧类游戏专用的「聪明但慢」档）。
///
/// 这是一个**深度 D、束宽 W 的定向束搜索**滚动规划器（MPC）。和旧的单步前瞻（1-ply：
/// 每个真 tick 只挑「这一刻一个动作」、再滚行 horizon 帧打分）相比，束搜索在 snapshot/restore
/// 之间建一棵搜索树：每个节点是一份 `Sim::snapshot`，展开一个节点 = 对每个候选动作 restore
/// 回该节点、注入、`step` **一帧**、再 snapshot 成子节点。这样「先按上再按右」「撞墙后重新
/// 按右爬上去」这类**多 tick 组合 / 连续机动**会自然从最优路径里涌现——因为搜索的每一层都
/// 重新挑动作，最优序列就是「右右右」或「上右」，而不是单步前瞻只能滚行同一动作。
///
/// 这是 Vitric 独有的：靠 `Sim::snapshot`/`Sim::restore` 能精确存读全状态（world+rng+tick+
/// 逻辑态 + 未消化输入/回复），别的引擎非确定、没法精确回滚重试，做不了这种逐层投机搜索。
///
/// **退化关系**：`depth=1` 时这棵树只有根的一层展开（对每个候选走一帧打分选最优），
/// 等价于旧的 1-ply 单步前瞻——保留作退化档。`horizon` 字段并入了 `depth` 的语义
/// （「往前看几帧」= 搜索深度），CLI 的 `--horizon` 直接映射到 `depth`（见 cmd_playtest）。
#[derive(Debug, Clone)]
pub struct LookaheadConfig {
    /// 搜索深度 D：从根往下展开几层 = 往前规划几帧。越大越「有远见」也越慢。
    /// `depth=1` 退化为单步前瞻（1-ply）。默认 8。
    pub depth: u64,
    /// 束宽 W：每一层只保留评分最高的 W 个节点继续往下展开（束剪枝，避免 B^D 爆炸）。
    /// 越大越不容易因为贪心剪枝错过需要「先变差再变好」的机动，但越慢。默认 4。
    pub beam_width: usize,
}

impl Default for LookaheadConfig {
    fn default() -> LookaheadConfig {
        // depth=8 / beam=4：导航/技巧类一般个位数候选 B，单真 tick 的投机步上界 ≈ W×B×D
        // （见 run_session_lookahead 的性能注释），默认值取「够解多 tick 组合又不至于太慢」。
        LookaheadConfig { depth: 8, beam_width: 4 }
    }
}

/// 一个搜索节点的评分（越大越优）。束搜索用它给每个展开出的子节点打分：先比终止
/// 信号（Win > 中性 > Lose），再比触达 Win 的早晚，再比目标派生量（有 goal）或探索新态
/// （无 goal）。用一个可全序比较的结构体承载——束剪枝按它排序取前 W、最终选最优叶子都靠它。
/// 确定性平手规则交给调用处（节点带「根第一动作下标 + 展开序」，平手取靠前的）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct NodeScore {
    /// 终止信号：+1=这条路径已触达 Win，0=没终止，-1=已触达 Lose（Win 优先级最高）。
    terminal: i8,
    /// 触达 Win 的早晚：越早越好。用「上界深度 - 命中那一层的深度」表示（没命中=0），越大越优。
    /// 让「3 步就赢」的路径压过「8 步才赢」的，规划器优先走最短通关。
    win_earliness: i64,
    /// 目标分（有 goal 时）：direction=min 取 -距离，max 取值（越大越优）；无 goal 时恒 0。
    /// 这是节点**当前状态**的目标派生量（不是 rollout 末态）——束搜索每层都重算，朝目标爬。
    goal_score: f64,
    /// 探索分（无 goal 时的弱进展信号）：从根到该节点累计走过多少个新 state_hash（变化次数）。
    /// 有 goal 时此项只当次级平手判据。
    explore: u64,
}

impl NodeScore {
    /// 全序比较：terminal > win_earliness > goal_score > explore（逐项，前者相等才看后者）。
    /// 无 goal 时 goal_score 恒 0，自然退到 explore 主导；NaN 目标分按最差处理（不该出现，
    /// 派生量取不到时上游已折成保底值，这里再兜一层防止比较 panic）。
    fn cmp(&self, other: &NodeScore) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.terminal
            .cmp(&other.terminal)
            .then(self.win_earliness.cmp(&other.win_earliness))
            .then(
                self.goal_score
                    .partial_cmp(&other.goal_score)
                    .unwrap_or(Ordering::Equal),
            )
            .then(self.explore.cmp(&other.explore))
    }
}

/// 束搜索树里的一个**活节点**：一份 snapshot + 它的评分 + 它在根那一层走的第一个动作下标。
///
/// `root_action` 是这个节点祖先链回溯到根时、根那一层选的候选下标（`view.actions[i]`，
/// 或 = 候选数 n 表示「不操作」）——MPC 只执行这第一步，所以每个节点必须一路带着它。
/// `terminated` 标记这条路径已经在某层触达终止（Win/Lose），不再往下展开（剪枝）。
/// `explore` 是从根到此累计的新态计数（无 goal 时的进展信号），随展开向下传递累加。
///
/// **`best_goal` 是关键设计**：节点的目标分用「从根到此**路径上见过的最优**目标值」，
/// 不是「当前状态的目标值」。为什么——技巧类的最优路径常**非单调**（先 up 才能 right：跳起时
/// 离出口的曼哈顿距离反而变大，落到墙那侧才骤降）。若按当前状态打分，束剪枝会把「正在翻墙、
/// 暂时变差」的分支剪掉，束搜索就退化成贪心、卡在墙前。用「路径最优值」给分=乐观估计：一条路
/// 只要在**任一帧**摸到过更近出口的位置就被记功，beam 才会保留这条「先变差再变好」的翻墙路，
/// 等它落到墙那侧 best_goal 骤升，规划器据此把根层第一步定成「该跳」。这正是 1-ply 做不到、
/// 需要规划深度才有的能力。无 goal 时 best_goal 恒 0，退到 explore 主导。
#[derive(Debug, Clone)]
struct BeamNode {
    /// 该节点的 sim 全状态快照（world+rng+tick+逻辑态+未消化输入/回复）。
    snapshot: Value,
    /// 评分（束剪枝排序 / 最终选最优都用它）。其 goal_score = 路径最优目标值（见结构体文档）。
    score: NodeScore,
    /// 这个节点在根层走的第一个动作下标（0..n=view.actions[i]，n=不操作）。
    root_action: usize,
    /// 这条路径是否已触达终止（已终止则不再展开，直接留作候选最优）。
    terminated: bool,
    /// 从根到此累计走过多少个新 state_hash（无 goal 探索信号，往下展开继续累加）。
    explore: u64,
    /// 从根到此**路径上见过的最优目标值**（越大越优；min 目标取 -距离，max 取值）。
    /// 往下展开时与子节点当前状态的目标值取 max 传递——单调不减，记录「这条路最接近过出口几何」。
    best_goal: f64,
}

/// 跑一局**定向束搜索滚动规划器**（设计稿：让技巧类游戏——平台/导航/解谜——被 swarm
/// 玩起来，不被随机策略误报 unbeatable）。和 [`run_session`] 同样出可重放录像、同样收遥测，
/// 区别只在「每真 tick 怎么选动作」：把旧的单步前瞻换成**深度 D、束宽 W 的束搜索**。
///
/// 每个真 tick（滚动地平线 / MPC：每真 tick 重新规划，只执行第一步）：
///
/// - **根**：`root = sim.snapshot(logic)` 存当前全状态，作为搜索树的根。
/// - **建树**：从根逐层展开。展开一个节点 = 对每个候选动作（`view.actions` 各一个 + 末尾一个
///   「不操作」）`restore` 回该节点、注入、`sim.step` **一帧**、再 `snapshot` 成子节点；子节点记下
///   「它在根层走的第一个动作」+ 用目标派生量算的评分（命中 Win 终止给最高分并剪枝、Lose 给最低分并剪枝）。
/// - **束剪枝**：每一层把展开出的子节点按评分排序、只保留最优的若干个继续往下展开（束搜索，避免
///   B^D 爆炸；保根动作多样性见 [`prune_beam_diverse`]）；已终止的节点不再展开，留作候选最优。
/// - **选首步**：展开到深度 **D**（或全束节点都终止）后，在所有探索到的节点里选评分最优的，取它的
///   `root_action`——那就是这个真 tick 要执行的动作。
/// - **执行**：真注入选中的动作，正常 `sim.step` 推进一个真 tick（**只有这一步进真录像**）。
///
/// 「先按上再按右」「撞墙后重新按右爬上去」这类**多 tick 组合 / 连续机动**自然从最优路径里
/// 涌现：搜索每一层都重新挑动作，最优序列就是「上右」或「右右右」，根层的第一个动作正是
/// 这串机动的第一步。`depth=1` 退化为单步前瞻（只展开根一层、选最优一帧），保留作退化档。
///
/// **确定性铁律**：节点展开顺序（候选按下标升序）、剪枝排序（按 `NodeScore::cmp`，平手取
/// 「根动作下标更小、其次展开更早」的）、最终选最优的平手规则全确定 → 同 (项目,seed,depth,
/// beam,起点) 出同一决策序列。投机步全程在 snapshot/restore 之间精确回滚、**绝不进真录像**。
/// `restore` 会清掉 sim 进行中的 recorder——而每真 tick 都要 restore 投机，所以**不能用
/// `sim.start_recording`/`stop_recording`**（第 2 个真 tick 一 restore 就把第 1 tick 起的录像
/// 清空）。这里改成**手工攒一个 `Recording`**：自己按「注入那一刻的 sim.tick」记 input/reply、
/// 每 60 真 tick（与 sim 内部 CHECKPOINT_INTERVAL 同口径）记一条 checkpoint、起点也记一条
/// `(0, 初始 hash)`、收尾填 final_hash/ticks。口径和 `Sim::step` 内部录制完全一致，所以这份
/// 手工录像照样可被 `Sim::replay` 逐位复现。只有真注入的动作才录。
///
/// **性能上界**：每个真 tick 的投机 `sim.step` 次数 ≤ **W × (B+1) × D**（B=候选动作数，+1 是
/// 「不操作」，W=束宽，D=深度；第一层只有 1 个根、之后每层至多 W 个节点各展开 B+1 个子）——
/// **线性于深度**（束搜索把 (B+1)^D 的指数爆炸压成线性）。导航/技巧类 B 一般个位数，是 opt-in
/// 的「聪明但慢」档。默认 swarm 掺的前瞻用小 depth（见 swarm.rs `DEFAULT_SWARM_LOOKAHEAD_DEPTH`）。
///
/// **目标量来源**：`cfg.playtest.goal`（派生量 + min/max）。**无 goal 退化**：节点评分先看
/// 「是否更早触达 Win」，没有 Win 时只能靠「到该节点走过多少新态」这种弱进展信号——没有目标量
/// 时无法判「哪个方向更近终点」，本就比有 goal 时弱得多。
pub fn run_session_lookahead(
    sim: &mut Sim,
    logic: &mut dyn GameLogic,
    engine: &Engine,
    cfg: &SessionConfig,
    look: &LookaheadConfig,
) -> Result<SessionResult, String> {
    if sim.is_recording() {
        return Err("run_session_lookahead 要自己开录：传进来的 sim 不该已在录像中".to_string());
    }
    if look.depth == 0 {
        return Err("lookahead depth 必须 ≥ 1".to_string());
    }
    if look.beam_width == 0 {
        return Err("lookahead beam_width 必须 ≥ 1".to_string());
    }
    // 投机搜索（expand_one）不重放 seed_replies。若种子录像带外部回复而投机不注入它们，
    // 搜索树就建在「那条回复没发生」的错误未来上，选出的动作按错误世界规划（不影响 replay
    // 安全——真步进照常注入、录像照常对得上——但规划质量错）。当前没有调用方给前瞻传
    // seed_replies（种子探索走 run_session 不走前瞻），所以这里硬报错把哑雷变响雷：谁要给
    // 前瞻接种子回复，必须先在 expand_one 的投机步里按 tick 注入，而不是默默跑出错规划。
    if !cfg.seed_replies.is_empty() {
        return Err("run_session_lookahead 暂不支持 seed_replies：投机搜索不会重放它们，\
             规划会建在错误的世界上。要给前瞻接种子回复，先在 expand_one 的投机步里按 tick 注入"
            .to_string());
    }

    // 遥测累加器（与 run_session 同口径）
    let mut state_trace: Vec<u64> = Vec::new();
    let mut numeric_summary: BTreeMap<String, NumericStat> = BTreeMap::new();
    let mut fired_events: Vec<String> = Vec::new();
    let mut seen_events: std::collections::HashSet<String> = std::collections::HashSet::new();

    // ---- 手工攒录像（不用 sim 的 recorder，因为每真 tick 都要 restore 投机会清掉它）----
    // 口径必须和 Sim::step / Sim::start_recording 一致，replay 才校验得过（见函数文档）。
    let mut recording = Recording {
        seed: sim.seed(),
        checkpoints: vec![(sim.tick, sim.world.state_hash())],
        ..Default::default()
    };

    let mut outcome = Outcome::Timeout;

    while sim.tick < cfg.max_ticks {
        let view = SceneView::derive_with_config(&sim.world, engine, &cfg.terminal, &cfg.playtest);
        if let Some(done) = view.done {
            outcome = done;
            break;
        }

        // ---- 束搜索规划（全程在 snapshot/restore 之间，不进手工录像）----
        // 根 = 当前真状态的快照。投机全程在它和后续 restore 之间，绝不污染真轨迹。
        let root = sim.snapshot(logic);
        let best_root_action =
            plan_beam(sim, logic, engine, cfg, look, &view, &root)?;
        // 回到真状态：投机结束，下面才是真 tick。
        sim.restore(&root, logic)?;

        let n = view.actions.len();
        // 真注入选中的动作（不操作=不注入），手工记进录像（tick = 注入那一刻的 sim.tick）。
        let inject_tick = sim.tick;
        if best_root_action < n {
            let a = &view.actions[best_root_action];
            sim.inject_input(&a.action, &a.phase);
            recording.inputs.push(InputRecord {
                tick: inject_tick,
                action: a.action.clone(),
                phase: a.phase.clone(),
            });
        }
        // 种子回复（和 run_session 同口径；前瞻一般不带，留通道一致）。同样手工记进录像。
        for reply in &cfg.seed_replies {
            if reply.tick == inject_tick {
                sim.inject_reply(&reply.name, reply.data.clone());
                recording.replies.push(ReplyRecord {
                    tick: inject_tick,
                    name: reply.name.clone(),
                    data: reply.data.clone(),
                });
            }
        }
        let report = sim.step(logic).map_err(|e| e.to_string())?;

        // 周期性 checkpoint：和 Sim::step 同口径（step 后 tick 是 60 的倍数就记一条）。
        if sim.tick.is_multiple_of(TICKS_PER_SECOND) {
            recording.checkpoints.push((sim.tick, sim.world.state_hash()));
        }

        // 遥测（真 step 后采）
        state_trace.push(sim.world.state_hash());
        let post = SceneView::derive_with_config(&sim.world, engine, &cfg.terminal, &cfg.playtest);
        collect_numeric_leaves(&post.observation, &mut numeric_summary);
        for name in report.events.iter().map(|e| e.name.as_str()) {
            if seen_events.insert(name.to_string()) {
                fired_events.push(name.to_string());
            }
        }
        if let Some(o) = scan_terminal(report.events.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
        let emitted = logic.drain_observed();
        for name in emitted.iter().map(|e| e.name.as_str()) {
            if seen_events.insert(name.to_string()) {
                fired_events.push(name.to_string());
            }
        }
        if let Some(o) = scan_terminal(emitted.iter(), &cfg.terminal) {
            outcome = o;
            break;
        }
    }

    // 收尾：和 Sim::stop_recording 同口径填末态。
    recording.ticks = sim.tick;
    recording.final_hash = sim.world.state_hash();
    Ok(SessionResult {
        outcome,
        ticks: sim.tick,
        recording,
        state_trace,
        fired_events,
        numeric_summary,
        // 前瞻是廉价（非 LLM）策略档：不产定性 note
        notes: Vec::new(),
    })
}

/// 在给定的根状态（`root` 快照）上跑一轮**深度 D、束宽 W 的束搜索**，返回根那一层该走的
/// 第一个动作下标（0..n=`view.actions[i]`，n=不操作）。MPC 只用这一步。
///
/// 调用约定：进来时 sim 在某个真状态、`root` 是它的快照；本函数全程在 snapshot/restore 之间
/// 投机（结束时 sim 停在某个投机末态，调用方负责事后 `restore(root)` 回真状态）。
///
/// 树展开（确定性顺序）：
/// - 第 0 层：以根为唯一活节点；展开它 = 对每个候选动作（下标升序，末尾「不操作」）restore 回根、
///   注入、step 一帧、snapshot 成子节点；根层每个候选定下它自己的 `root_action`（= 该候选下标）。
/// - 第 k 层（k≥1）：对上一层束里的每个活节点同样展开（子节点继承父的 `root_action`）。
/// - 每层展开完，把所有新子节点按 `NodeScore::cmp` 降序排（平手取根动作下标更小、其次展开更早），
///   取前 W 个作为下一层的束。已终止（命中 Win/Lose）的节点不展开，直接进「候选最优池」。
/// - 跑满 D 层或束空（全终止）为止。最后在「候选最优池 + 末层束」里选评分最优的，回它的 `root_action`。
fn plan_beam(
    sim: &mut Sim,
    logic: &mut dyn GameLogic,
    engine: &Engine,
    cfg: &SessionConfig,
    look: &LookaheadConfig,
    view: &SceneView,
    root: &Value,
) -> Result<usize, String> {
    let n = view.actions.len(); // 候选 = n 个动作 + 1 个「不操作」(下标 n)
    // 候选最优池：收集「沿途最优」节点——每层展开后的全部子节点都参与「选最终最优」，
    // 这样即使 D 层都没人通关，也能选「目标分最高的那个中间节点」的根动作（朝目标爬）。
    // 用单个 best 累加（严格更优才换 + 平手保守取靠前），不必存全部节点。
    let mut best: Option<BeamNode> = None;
    // 当前层的束（活节点，下一层从它们展开）。初始为「根」这一虚拟节点（还没走动作）。
    // 用 root 快照 + 占位 score/root_action 表示根；根的 root_action 不会被选中
    //（根层展开时每个候选才定真正的 root_action），这里给 n 占位。
    // 根节点的 best_goal 用根当前状态的目标值起头（之后子节点路径取 max 单调累进）。
    // sim 此刻就在根状态（plan_beam 进来时还没 restore 别处），直接算。
    let root_goal = node_goal_score(sim, engine, cfg);
    let mut beam: Vec<BeamNode> = vec![BeamNode {
        snapshot: root.clone(),
        score: NodeScore { terminal: 0, win_earliness: 0, goal_score: root_goal, explore: 0 },
        root_action: n,
        terminated: false,
        explore: 0,
        best_goal: root_goal,
    }];

    let depth = look.depth;
    for layer in 0..depth {
        // 这一层展开出的全部子节点（待剪枝）。
        let mut children: Vec<BeamNode> = Vec::new();
        for node in &beam {
            // 已终止的不展开——但它本身是合法候选最优（如「这条路 3 步就赢了」）。
            if node.terminated {
                consider_best(&mut best, node);
                continue;
            }
            for cand in 0..=n {
                // restore 到这个父节点，注入候选动作（cand==n 即不操作），走一帧 snapshot 成子。
                sim.restore(&node.snapshot, logic)?;
                if cand < n {
                    let a = &view.actions[cand];
                    sim.inject_input(&a.action, &a.phase);
                }
                // 根层（layer==0）每个候选定它自己的 root_action；更深层继承父的。
                let root_action = if layer == 0 { cand } else { node.root_action };
                let ctx = ExpandCtx {
                    root_action,
                    parent_explore: node.explore,
                    parent_best_goal: node.best_goal,
                    layer,
                };
                let child = expand_one(sim, logic, engine, cfg, &ctx)?;
                consider_best(&mut best, &child);
                children.push(child);
            }
        }
        // 束剪枝（保根动作多样性）：见 prune_beam_diverse 的算法/为什么。
        beam = prune_beam_diverse(children, look.beam_width);
        if beam.is_empty() {
            break; // 全束都终止了（已进 best），不必再往下展开
        }
    }
    // 末层束里的节点也参与「选最终最优」（它们是「看了 D 层最有希望」的状态）。
    for node in &beam {
        consider_best(&mut best, node);
    }

    // best 一定有值：根层至少展开出 1 个子节点（n≥0，候选至少含「不操作」）。
    let best = best.ok_or("束搜索未展开出任何节点（不应发生：至少有「不操作」候选）")?;
    Ok(best.root_action)
}

/// 束剪枝（**保根动作多样性的束搜索**）：从这一层展开出的全部子节点里挑出下一层的束。
///
/// 不是朴素的「全局评分前 W」，而是**先保证每个根动作各留一条最优线，再用全局次优填满到 W**：
/// - 第 1 趟：按评分降序遍历，每个 `root_action` 第一次出现时收下它（= 该根动作的最优后继线）。
///   这保证「翻墙要先跳」这种**早分叉、当前暂时变差**的根动作（jump）不会被 4 条「走右撞墙」的
///   同根动作线挤掉——否则贪心启发（曼哈顿距离）会把 jump 线在第 1 层就剪光，束搜索退化成贪心、
///   永远卡在墙前（实测：朴素 top-W 即便 depth=40 也一次都不跳）。
/// - 第 2 趟：若名额还没满 W，再按评分降序把剩下没收的子节点补进来（给最有希望的根动作多几条线
///   深入探索）。
///
/// 这是「diverse beam search」的标准做法：用根动作分桶强制多样性，避免最优线被同质分支淹没。
/// 代价上界随之变成 `≤ max(W, 根动作数) × (B+1) × D`（根动作数=B+1，所以 ≈ (B+1)²×D，仍线性于
/// 深度）。导航/技巧类 B 个位数，可控。
///
/// **确定性**：入参 `children` 是按 (父在上层束的序, 候选下标升序) 生成的；本函数先稳定排序
/// （评分降序，平手根动作下标小者在前，再平手保持生成序），再分桶——同输入出同一束，全程确定。
fn prune_beam_diverse(mut children: Vec<BeamNode>, beam_width: usize) -> Vec<BeamNode> {
    if children.is_empty() {
        return children;
    }
    // 稳定排序：评分降序；平手时根动作下标小的在前；再平手保持生成序（展开更早的靠前）。
    children.sort_by(|a, b| b.score.cmp(&a.score).then(a.root_action.cmp(&b.root_action)));

    let mut chosen: Vec<BeamNode> = Vec::with_capacity(beam_width.max(1));
    let mut seen_roots: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    let mut taken = vec![false; children.len()];

    // 第 1 趟：每个根动作留它的最优后继线（按已排序的评分序，第一次见到就收）。
    for (i, c) in children.iter().enumerate() {
        if seen_roots.insert(c.root_action) {
            chosen.push(c.clone());
            taken[i] = true;
        }
    }
    // 第 2 趟：名额没满 W 就按评分序补全局次优（已收过的跳过）。
    if beam_width > chosen.len() {
        for (i, c) in children.iter().enumerate() {
            if chosen.len() >= beam_width {
                break;
            }
            if !taken[i] {
                chosen.push(c.clone());
                taken[i] = true;
            }
        }
    }
    chosen
}

/// 展开一个子节点时从父继承/由位置决定的上下文（打包成结构体，免得 expand_one 参数爆表）。
struct ExpandCtx {
    /// 这个子节点在根层走的第一个动作下标（根层=候选自己，更深层=继承父的）。
    root_action: usize,
    /// 从根到父累计的新态数（本帧若产生新 state_hash 再 +1 传给子）。
    parent_explore: u64,
    /// 从根到父路径上见过的最优目标值（与本帧当前目标值取 max 传给子）。
    parent_best_goal: f64,
    /// 当前展开的是第几层（0-based）：命中 Win 时据此换算 `win_earliness`（越早越大）。
    layer: u64,
}

/// 从「已 restore 到父节点 + 注入了候选动作」的状态出发，走**一帧** `sim.step`，把结果
/// 打包成一个子 [`BeamNode`]：扫这一帧的终止事件、算节点评分、snapshot 成子状态。
/// 调用前 sim 必须已 restore + inject；本函数推进一帧（调用方负责后续 restore 回别的节点）。
fn expand_one(
    sim: &mut Sim,
    logic: &mut dyn GameLogic,
    engine: &Engine,
    cfg: &SessionConfig,
    ctx: &ExpandCtx,
) -> Result<BeamNode, String> {
    let before = sim.world.state_hash();
    let report = sim.step(logic).map_err(|e| e.to_string())?;
    // 终止扫描：step 事件 + 逻辑 emit 的事件（同 run_session 口径，Win 优先于 Lose）。
    let mut hit: Option<Outcome> = scan_terminal(report.events.iter(), &cfg.terminal);
    let emitted = logic.drain_observed();
    if hit.is_none() {
        hit = scan_terminal(emitted.iter(), &cfg.terminal);
    }

    // 探索弱信号：这一帧若走到新 state_hash 就在父的累计上 +1。
    let after = sim.world.state_hash();
    let explore = ctx.parent_explore + if after != before { 1 } else { 0 };

    let mut terminal: i8 = 0;
    let mut win_earliness: i64 = 0;
    let mut terminated = false;
    match hit {
        Some(Outcome::Win) => {
            terminal = 1;
            terminated = true;
            // 命中越早（layer 越小）→ win_earliness 越大。上界用大常数让它恒为正且单调。
            // 这里 layer 是「触达 Win 的那一层」的下标；+1 让第 0 层命中也 < 满分。
            win_earliness = (cfg_depth_bound(cfg) as i64).max(ctx.layer as i64 + 1) - ctx.layer as i64;
        }
        Some(Outcome::Lose) => {
            terminal = -1;
            terminated = true;
        }
        Some(Outcome::Timeout) | None => {}
    }

    // 路径最优目标值：父的最优值与本帧当前状态目标值取 max（单调不减）。用它当节点 goal_score
    // ——技巧类最优路径非单调（翻墙时当前距离暂时变大），按「路径见过的最优」打分才不会把翻墙
    // 中途的分支误剪（见 BeamNode 文档）。无 goal 时 node_goal_score 恒 0，best_goal 也恒 0。
    let cur_goal = node_goal_score(sim, engine, cfg);
    let best_goal = ctx.parent_best_goal.max(cur_goal);

    let score = NodeScore { terminal, win_earliness, goal_score: best_goal, explore };
    let snapshot = sim.snapshot(logic);
    Ok(BeamNode { snapshot, score, root_action: ctx.root_action, terminated, explore, best_goal })
}

/// `win_earliness` 的上界基准：取「至少和最深一层一样大」的常数，保证越早命中的 Win 分越高。
/// 简单用一个够大的固定上界（搜索深度通常 ≤ 几十）；只要它 ≥ 任何可能的 layer 即可。
fn cfg_depth_bound(_cfg: &SessionConfig) -> u64 {
    // 1<<20 远大于任何现实搜索深度——保证 (bound - layer) 单调递减且恒正，越早赢分越高。
    1 << 20
}

/// 算一个节点当前状态的目标分（越大越优）：有 goal 时按 direction 取 -距离 / 值；
/// 取不到派生量给最差保底；无 goal 恒 0（退到 explore 主导）。
fn node_goal_score(sim: &Sim, engine: &Engine, cfg: &SessionConfig) -> f64 {
    match &cfg.playtest.goal {
        Some(g) => {
            let view = SceneView::derive_with_config(&sim.world, engine, &cfg.terminal, &cfg.playtest);
            let val = view
                .observation
                .get("derived")
                .and_then(|d| d.get(&g.quantity))
                .and_then(|v| v.as_f64());
            match val {
                Some(v) => match g.direction {
                    // min：距离越小越优 → 取负距离（越大越优，和 NodeScore 口径一致）
                    crate::config::GoalDirection::Min => -v,
                    crate::config::GoalDirection::Max => v,
                },
                // 取不到目标量（派生量为 null 等）：给最差保底，不让这个节点被选中
                None => f64::NEG_INFINITY,
            }
        }
        None => 0.0,
    }
}

/// 用一个候选节点更新「全局最优」：严格更优才替换；**平手保守取靠前**
///（根动作下标更小者优先，再相等保留先到的=展开更早的）。确定性平手规则的单一出口。
fn consider_best(best: &mut Option<BeamNode>, node: &BeamNode) {
    let replace = match best {
        None => true,
        Some(b) => {
            use std::cmp::Ordering;
            match node.score.cmp(&b.score) {
                Ordering::Greater => true,
                Ordering::Less => false,
                // 评分平手：根动作下标更小的优先（确定）；再相等保留已有（先到先得）
                Ordering::Equal => node.root_action < b.root_action,
            }
        }
    };
    if replace {
        *best = Some(node.clone());
    }
}

/// 一组事件里第一个命中终止的结局（按事件序，确定性）。
fn scan_terminal<'a>(
    events: impl Iterator<Item = &'a vitric_rules::Event>,
    terminal: &TerminalSpec,
) -> Option<Outcome> {
    for e in events {
        if let Some(o) = terminal.classify(&e.name) {
            return Some(o);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vitric_data::Schema;
    use vitric_rules::{Event, RuleSet};
    use vitric_sim::Pcg32;

    use crate::scene_view::Action;
    use crate::strategy::RandomStrategy;

    fn empty_engine() -> Engine {
        let schema = Schema::parse(&json!({"components": {}}), "s.json").unwrap();
        Engine::new(RuleSet::parse(&json!({"rules": []}), "r.json").unwrap(), schema)
    }

    /// 在第 N tick 发一个终止事件的最小逻辑（drain_observed 把它交给会话扫描）。
    struct EmitAt {
        at: u64,
        event: String,
        pending: Vec<Event>,
    }
    impl GameLogic for EmitAt {
        fn on_tick(
            &mut self,
            _world: &mut vitric_ecs::World,
            _events: Vec<Event>,
            _rng: &mut Pcg32,
            tick: u64,
        ) -> Result<(), String> {
            // step 在 on_tick 后才 +1，所以这里的 tick 是「即将完成的那一帧」编号
            if tick == self.at {
                self.pending.push(Event::new(&self.event, json!({})));
            }
            Ok(())
        }
        fn drain_observed(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }
    }

    /// 永不终止的逻辑：用来验证 Timeout。
    struct NeverEnds;
    impl GameLogic for NeverEnds {
        fn on_tick(
            &mut self,
            _: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn run_session_collects_win_on_terminal_event() {
        let mut sim = Sim::new(1);
        let mut logic = EmitAt { at: 3, event: "game-won".to_string(), pending: vec![] };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 100, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Win);
        assert_eq!(res.ticks, 4, "tick 3 那帧发的 game-won，step 后 tick=4 时收到");
    }

    #[test]
    fn run_session_collects_lose_on_terminal_event() {
        let mut sim = Sim::new(1);
        let mut logic = EmitAt { at: 2, event: "game-over".to_string(), pending: vec![] };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 100, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Lose);
    }

    #[test]
    fn lookahead_rejects_seed_replies() {
        // 前瞻投机搜索不重放 seed_replies，会建在错误未来上规划——所以见非空就硬报错，
        // 而不是默默跑出错规划（code review #1：把哑雷变响雷）。
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let cfg = SessionConfig {
            max_ticks: 100,
            seed: 0,
            seed_replies: vec![ReplyRecord { tick: 1, name: "oracle".to_string(), data: json!({}) }],
            ..Default::default()
        };
        let err = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig::default())
            .unwrap_err();
        assert!(err.contains("seed_replies"), "应明确报 seed_replies 不支持：{err}");
    }

    #[test]
    fn run_session_times_out_and_records() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 50, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Timeout);
        assert_eq!(res.ticks, 50);
        assert_eq!(res.recording.ticks, 50);
        // 空 engine 没有动作词汇，策略选不出动作 → 录像无输入，但 checkpoint 仍在
        assert!(!res.recording.checkpoints.is_empty());
    }

    #[test]
    fn run_session_is_deterministic_byte_for_byte() {
        let run = || {
            let mut sim = Sim::new(1);
            let mut logic = NeverEnds;
            let eng = action_engine();
            let mut strat = RandomStrategy::new(123);
            let cfg = SessionConfig { max_ticks: 80, seed: 123, ..Default::default() };
            run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap()
        };
        let a = run();
        let b = run();
        assert_eq!(a.outcome, b.outcome);
        assert_eq!(a.ticks, b.ticks);
        // 录像逐字节一致
        let ja = serde_json::to_string(&a.recording).unwrap();
        let jb = serde_json::to_string(&b.recording).unwrap();
        assert_eq!(ja, jb, "同 (策略,seed,起点) 两次跑录像必须逐字节一致");
    }

    /// 带 input 词汇的引擎：让随机策略真能注入动作，录像里有 inputs。
    fn action_engine() -> Engine {
        let schema = Schema::parse(
            &json!({"components": {"Velocity": {"fields": {"x": {"type": "number"}}}}}),
            "s.json",
        )
        .unwrap();
        let rules = RuleSet::parse(
            &json!({"rules": [
                {"id": "left", "on": {"event": "input", "filter": {"action": "left", "phase": "pressed"}},
                 "do": [{"emit": "noop", "data": {}}]},
                {"id": "right", "on": {"event": "input", "filter": {"action": "right", "phase": "pressed"}},
                 "do": [{"emit": "noop", "data": {}}]}
            ]}),
            "r.json",
        )
        .unwrap();
        Engine::new(rules, schema)
    }

    /// 一个每 tick 吐一条 note 的假策略（不碰 LLM，只验 session 的 note 收集通道）。
    struct NotingStrategy {
        tick: u64,
        pending: Vec<crate::strategy::PlaytestNote>,
    }
    impl crate::strategy::Strategy for NotingStrategy {
        fn choose(&mut self, _view: &SceneView) -> Option<Action> {
            // 每个决策 tick 攒一条 note，模拟 LLM 策略在 choose 里产 note
            self.pending.push(crate::strategy::PlaytestNote {
                tick: self.tick,
                kind: "clarity".to_string(),
                text: format!("第 {} tick 看不懂", self.tick),
            });
            self.tick += 1;
            None
        }
        fn drain_notes(&mut self) -> Vec<crate::strategy::PlaytestNote> {
            std::mem::take(&mut self.pending)
        }
    }

    #[test]
    fn run_session_collects_notes_from_strategy() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = NotingStrategy { tick: 0, pending: vec![] };
        let cfg = SessionConfig { max_ticks: 5, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        // 5 个决策 tick 各产一条 note，全被 session 收进 notes
        assert_eq!(res.notes.len(), 5, "每 tick 一条 note 应全部收齐: {:?}", res.notes);
        assert_eq!(res.notes[0].tick, 0);
        assert_eq!(res.notes[4].tick, 4);
        assert!(res.notes[0].text.contains("看不懂"));
    }

    #[test]
    fn run_session_notes_empty_for_non_noting_strategy() {
        // 普通策略（random）不产 note → notes 为空（note 通道是 LLM 档专属）
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 10, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert!(res.notes.is_empty(), "非 LLM 策略不产 note");
    }

    #[test]
    fn run_session_collects_state_trace_one_per_tick() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 40, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        // 每跑一个 tick 采一条 state_hash，长度必须等于实际 tick 数
        assert_eq!(res.state_trace.len(), res.ticks as usize);
        assert_eq!(res.state_trace.len(), 40);
    }

    /// 每 tick 都 emit 同一个非终止事件——用来验证 fired_events 去重。
    struct EmitEveryTick {
        event: String,
        pending: Vec<Event>,
    }
    impl GameLogic for EmitEveryTick {
        fn on_tick(
            &mut self,
            _: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            self.pending.push(Event::new(&self.event, json!({})));
            Ok(())
        }
        fn drain_observed(&mut self) -> Vec<Event> {
            std::mem::take(&mut self.pending)
        }
    }

    #[test]
    fn run_session_collects_fired_events_deduped() {
        let mut sim = Sim::new(1);
        // 每 tick 发同名事件 20 次，fired_events 应只收一次（去重）
        let mut logic = EmitEveryTick { event: "milestone".to_string(), pending: vec![] };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        // milestone 不是终止事件 → 不会停，跑满
        let cfg = SessionConfig { max_ticks: 20, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert!(res.fired_events.contains(&"milestone".to_string()), "{:?}", res.fired_events);
        // 去重：发了 20 次，fired_events 里只一份
        assert_eq!(res.fired_events.iter().filter(|n| *n == "milestone").count(), 1);
    }

    #[test]
    fn run_session_with_actions_records_inputs() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = action_engine();
        let mut strat = RandomStrategy::new(5);
        let cfg = SessionConfig { max_ticks: 200, seed: 5, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        assert_eq!(res.outcome, Outcome::Timeout);
        assert!(!res.recording.inputs.is_empty(), "随机策略应注入过一些输入");
        // 注入的动作都在词汇表里
        for inp in &res.recording.inputs {
            assert!(["left", "right"].contains(&inp.action.as_str()), "意外动作 {inp:?}");
        }
        let _ = Action { action: "x".into(), phase: "pressed".into() }; // 引用 Action 类型
    }

    // ---- 数值遥测（NumericStat / numeric_summary）单元测试 ----

    #[test]
    fn numeric_stat_observe_tracks_min_max_last_monotonic_zero() {
        // 100 → 50 → 200 → 0：min=0 max=200 last=0；中途下降过 → 非单调；触达过 0
        let mut s = NumericStat::start(100.0);
        s.observe(50.0);
        s.observe(200.0);
        s.observe(0.0);
        assert_eq!(s.first, 100.0);
        assert_eq!(s.last, 0.0);
        assert_eq!(s.min, 0.0);
        assert_eq!(s.max, 200.0);
        assert!(!s.monotonic_up, "中途降过就不是只增不减");
        assert!(s.hit_zero, "触达过 0");
        assert!(!s.non_finite);
    }

    #[test]
    fn numeric_stat_monotonic_up_stays_true_when_only_grows() {
        let mut s = NumericStat::start(1.0);
        for v in [2.0, 4.0, 8.0, 16.0] {
            s.observe(v);
        }
        assert!(s.monotonic_up, "只增不减");
        assert_eq!(s.max, 16.0);
        assert!(!s.hit_zero);
    }

    #[test]
    fn numeric_stat_flags_non_finite() {
        let mut s = NumericStat::start(1.0);
        s.observe(f64::INFINITY);
        assert!(s.non_finite, "inf 必须标 non_finite");
        // 非有限值不污染 min/max（仍是有限观测的范围）
        assert_eq!(s.max, 1.0);
    }

    #[test]
    fn collect_numeric_leaves_extracts_nested_paths() {
        // observation 仿 SceneView 投影结构：entities[].{name,components.{Comp.{field}}}
        let obs = json!({"entities": [
            {"name": "hero", "id": "0v0", "components": {
                "Resources": {"gold": 12.0, "wood": 3},
                "Stats": {"hp": 100}
            }},
            {"id": "1v0", "components": {"Tally": {"n": 7}}}  // 无名 → 用 id 当 label
        ]});
        let mut sum: BTreeMap<String, NumericStat> = BTreeMap::new();
        collect_numeric_leaves(&obs, &mut sum);
        assert!(sum.contains_key("hero/Resources.gold"), "{:?}", sum.keys().collect::<Vec<_>>());
        assert!(sum.contains_key("hero/Resources.wood"));
        assert!(sum.contains_key("hero/Stats.hp"));
        assert!(sum.contains_key("1v0/Tally.n"), "无名实体用 id 当 label");
        assert_eq!(sum["hero/Resources.gold"].first, 12.0);
    }

    #[test]
    fn collect_numeric_leaves_skips_non_numeric() {
        // bool/string 不进数值摘要（flag 不是数值）
        let obs = json!({"entities": [
            {"name": "w", "components": {"State": {"sealed": true, "label": "x", "count": 5}}}
        ]});
        let mut sum: BTreeMap<String, NumericStat> = BTreeMap::new();
        collect_numeric_leaves(&obs, &mut sum);
        assert!(sum.contains_key("w/State.count"));
        assert!(!sum.contains_key("w/State.sealed"), "bool 不收");
        assert!(!sum.contains_key("w/State.label"), "string 不收");
    }

    /// 一个每 tick 把某实体字段 ×2 的逻辑（直接改 world，模拟经济跑飞）。
    struct DoublerLogic {
        ent: String,
    }
    impl GameLogic for DoublerLogic {
        fn on_tick(
            &mut self,
            world: &mut vitric_ecs::World,
            _: Vec<Event>,
            _: &mut Pcg32,
            _: u64,
        ) -> Result<(), String> {
            // 找到命名实体，把 Bank.gold ×2（float，避免 i64 checked_add 溢出报错）
            let id = world.entity_names().find(|(n, _)| *n == self.ent).map(|(_, id)| id);
            if let Some(id) = id {
                if let Ok(v) = world.get_component(id, "Bank") {
                    let cur = v.get("gold").and_then(|g| g.as_f64()).unwrap_or(0.0);
                    let _ = world.set_component(id, "Bank", json!({"gold": cur * 2.0}));
                }
            }
            Ok(())
        }
    }

    fn bank_world_sim() -> Sim {
        let mut sim = Sim::new(1);
        let id = sim.world.spawn_named("hero").unwrap();
        sim.world.set_component(id, "Bank", json!({"gold": 1.0})).unwrap();
        sim
    }

    #[test]
    fn run_session_numeric_summary_catches_runaway_growth() {
        let mut sim = bank_world_sim();
        let mut logic = DoublerLogic { ent: "hero".to_string() };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 30, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        let stat = res.numeric_summary.get("hero/Bank.gold").expect("应采到 hero/Bank.gold");
        assert!(stat.monotonic_up, "只 ×2 → 只增不减");
        // 30 tick 翻倍：远超初值的千倍（2^30 ≈ 1e9）
        assert!(stat.max > 1e6, "翻倍 30 次应跑飞到 >1e6，实际 {}", stat.max);
        assert!(stat.last > stat.first * 1000.0, "末值 ≫ 首值");
    }

    #[test]
    fn run_session_numeric_summary_is_incremental_not_full_history() {
        // 摘要只存每个字段一条 NumericStat，与 tick 数无关（增量，不存历史）
        let mut sim = bank_world_sim();
        let mut logic = DoublerLogic { ent: "hero".to_string() };
        let eng = empty_engine();
        let mut strat = RandomStrategy::new(0);
        let cfg = SessionConfig { max_ticks: 20, seed: 0, ..Default::default() };
        let res = run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap();
        // 只一个数值字段 → 摘要里恰好一条，不随 tick 膨胀
        assert_eq!(res.numeric_summary.len(), 1);
    }

    #[test]
    fn run_session_numeric_summary_is_deterministic() {
        let run = || {
            let mut sim = bank_world_sim();
            let mut logic = DoublerLogic { ent: "hero".to_string() };
            let eng = empty_engine();
            let mut strat = RandomStrategy::new(9);
            let cfg = SessionConfig { max_ticks: 15, seed: 9, ..Default::default() };
            run_session(&mut sim, &mut logic, &eng, &mut strat, &cfg).unwrap().numeric_summary
        };
        assert_eq!(run(), run(), "同输入两次跑数值摘要必须逐项一致");
    }

    // ---- 前瞻搜索（run_session_lookahead）单元测试 ----

    #[test]
    fn lookahead_rejects_zero_depth() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let cfg = SessionConfig { max_ticks: 10, seed: 0, ..Default::default() };
        let err = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 0, beam_width: 4 }).unwrap_err();
        assert!(err.contains("depth"), "{err}");
    }

    #[test]
    fn lookahead_rejects_zero_beam_width() {
        let mut sim = Sim::new(1);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let cfg = SessionConfig { max_ticks: 10, seed: 0, ..Default::default() };
        let err = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 4, beam_width: 0 }).unwrap_err();
        assert!(err.contains("beam_width"), "{err}");
    }

    /// 手工攒的录像必须能被 Sim::replay 逐位复现——锁死「投机的 restore 不污染录像、
    /// tick/checkpoint 口径和 sim 内部一致」这两件事。用 action_engine（有 left/right 词汇，
    /// 候选非空 → 投机每真 tick 都 restore 多次）+ EmitAt（tick 70 发 game-won，跨 60 边界
    /// 才有周期 checkpoint）逼出 checkpoint 对齐。
    #[test]
    fn lookahead_recording_replays_bit_for_bit() {
        let mut sim = Sim::new(1);
        let mut logic = EmitAt { at: 70, event: "game-won".to_string(), pending: vec![] };
        let eng = action_engine();
        let cfg = SessionConfig { max_ticks: 300, seed: 0, ..Default::default() };
        let res = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 5, beam_width: 4 }).unwrap();
        assert_eq!(res.outcome, Outcome::Win);
        assert_eq!(res.ticks, 71, "tick 70 发的 game-won，step 后 tick=71 收到");
        // 跨过 tick 60 → 必有起点 (0,_) + 周期 (60,_) 两条 checkpoint
        assert!(res.recording.checkpoints.len() >= 2, "应有起点+周期 checkpoint: {:?}", res.recording.checkpoints);
        assert_eq!(res.recording.checkpoints[0].0, 0, "起点 checkpoint tick=0");
        assert_eq!(res.recording.ticks, 71);
        assert_eq!(res.recording.seed, 1, "录像 seed = sim.seed");

        // 关键：从冷启动重放这份手工录像，逐校验点 + final_hash 必须全对上
        let mut sim2 = Sim::new(1);
        let mut logic2 = EmitAt { at: 70, event: "game-won".to_string(), pending: vec![] };
        sim2.replay(&res.recording, &mut logic2).expect("手工攒的前瞻录像必须可重放且逐位一致");
        assert_eq!(sim2.world.state_hash(), res.recording.final_hash);
    }

    #[test]
    fn lookahead_is_deterministic_byte_for_byte() {
        let run = || {
            let mut sim = Sim::new(2);
            let mut logic = EmitAt { at: 50, event: "game-won".to_string(), pending: vec![] };
            let eng = action_engine();
            let cfg = SessionConfig { max_ticks: 200, seed: 2, ..Default::default() };
            run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 6, beam_width: 4 }).unwrap()
        };
        let a = run();
        let b = run();
        assert_eq!(a.outcome, b.outcome);
        assert_eq!(a.ticks, b.ticks);
        let ja = serde_json::to_string(&a.recording).unwrap();
        let jb = serde_json::to_string(&b.recording).unwrap();
        assert_eq!(ja, jb, "同 (项目,seed,depth,beam) 两次前瞻录像必须逐字节一致");
    }

    /// 起手就 done / max_ticks=0：一个真 tick 都不跑，仍出一份合法空录像（起点 checkpoint + ticks=0）。
    #[test]
    fn lookahead_zero_ticks_yields_valid_empty_recording() {
        let mut sim = Sim::new(3);
        let mut logic = NeverEnds;
        let eng = empty_engine();
        let cfg = SessionConfig { max_ticks: 0, seed: 0, ..Default::default() };
        let res = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 4, beam_width: 4 }).unwrap();
        assert_eq!(res.ticks, 0);
        assert_eq!(res.recording.ticks, 0);
        assert!(res.recording.inputs.is_empty());
        assert_eq!(res.recording.checkpoints, vec![(0, res.recording.final_hash)]);
        // 空录像也可重放（起点即终点）
        let mut sim2 = Sim::new(3);
        sim2.replay(&res.recording, &mut NeverEnds).expect("空前瞻录像也可重放");
    }

    /// depth=1 退化为单步前瞻：每候选只前瞻 1 帧选最优。这里用「tick 2 发 game-won」的逻辑，
    /// depth=1 在 tick 1 那帧（step 后 tick=2 收到 win）也能收到终止——验证退化档照常跑、出录像。
    #[test]
    fn lookahead_depth_one_is_degenerate_one_ply() {
        let mut sim = Sim::new(1);
        let mut logic = EmitAt { at: 2, event: "game-won".to_string(), pending: vec![] };
        let eng = action_engine();
        let cfg = SessionConfig { max_ticks: 50, seed: 0, ..Default::default() };
        let res = run_session_lookahead(&mut sim, &mut logic, &eng, &cfg, &LookaheadConfig { depth: 1, beam_width: 4 }).unwrap();
        assert_eq!(res.outcome, Outcome::Win, "depth=1 退化档也该收到终止事件");
        assert_eq!(res.ticks, 3, "tick 2 发的 game-won，step 后 tick=3 收到");
        // depth=1 照样手工攒可重放录像
        let mut sim2 = Sim::new(1);
        let mut logic2 = EmitAt { at: 2, event: "game-won".to_string(), pending: vec![] };
        sim2.replay(&res.recording, &mut logic2).expect("depth=1 录像可重放");
    }

    // ---- prune_beam_diverse：保根动作多样性 + 确定性 ----

    /// 造一个最小 BeamNode（snapshot 用 Null 占位——prune 只读 score/root_action）。
    fn node(root_action: usize, goal: f64, explore: u64) -> BeamNode {
        BeamNode {
            snapshot: Value::Null,
            score: NodeScore { terminal: 0, win_earliness: 0, goal_score: goal, explore },
            root_action,
            terminated: false,
            explore,
            best_goal: goal,
        }
    }

    #[test]
    fn prune_keeps_one_line_per_root_action_even_when_width_smaller() {
        // 6 个子：root 0 有 3 条（含全局最高分），root 1、root 2 各一条（分较低）。
        // beam_width=2 < 不同根数 3 → 第 1 趟仍保证每个根各留它的最优一条（3 条），不被 root 0 的
        // 三条挤掉。证明多样性：root 1、root 2 的线不会因为 root 0 分高而被剪光。
        let children = vec![
            node(0, 10.0, 5), // root0 最优
            node(0, 9.0, 4),
            node(0, 8.0, 3),
            node(1, 2.0, 9), // root1 唯一
            node(2, 1.0, 1), // root2 唯一
        ];
        let kept = prune_beam_diverse(children, 2);
        let roots: std::collections::BTreeSet<usize> = kept.iter().map(|n| n.root_action).collect();
        assert!(roots.contains(&0) && roots.contains(&1) && roots.contains(&2),
            "每个根动作都该留一条线，实际根 {roots:?}");
        // 每个根只留它的最优一条（root0 留 10.0 那条）
        let root0: Vec<_> = kept.iter().filter(|n| n.root_action == 0).collect();
        assert_eq!(root0.len(), 1, "每根第 1 趟只留一条");
        assert_eq!(root0[0].score.goal_score, 10.0, "root0 留的是它的最优");
    }

    #[test]
    fn prune_fills_remaining_slots_with_global_best_when_width_allows() {
        // 3 个根，beam_width=5 > 根数：第 1 趟留 3 条（每根最优），第 2 趟再补 2 条全局次优。
        let children = vec![
            node(0, 10.0, 0),
            node(0, 9.0, 0), // root0 次优——第 2 趟该补进来
            node(1, 8.0, 0),
            node(1, 7.0, 0), // root1 次优——第 2 趟该补进来
            node(2, 1.0, 0),
        ];
        let kept = prune_beam_diverse(children, 5);
        assert_eq!(kept.len(), 5, "名额够就补满");
        // 补的是全局次优（9.0、8.0 而非 1.0 之外的更低）——按分序补
        let goals: Vec<f64> = kept.iter().map(|n| n.score.goal_score).collect();
        assert!(goals.contains(&9.0) && goals.contains(&7.0), "第 2 趟按分序补次优: {goals:?}");
    }

    #[test]
    fn prune_is_deterministic_and_tiebreaks_by_root_action() {
        // 全同分平手：稳定排序 + 根动作小者优先 → 输出顺序确定（root 升序）。
        let mk = || vec![node(2, 5.0, 0), node(0, 5.0, 0), node(1, 5.0, 0)];
        let a = prune_beam_diverse(mk(), 3);
        let b = prune_beam_diverse(mk(), 3);
        let ra: Vec<usize> = a.iter().map(|n| n.root_action).collect();
        let rb: Vec<usize> = b.iter().map(|n| n.root_action).collect();
        assert_eq!(ra, rb, "同输入两次剪枝顺序必须一致");
        assert_eq!(ra, vec![0, 1, 2], "平手按根动作下标升序");
    }
}
