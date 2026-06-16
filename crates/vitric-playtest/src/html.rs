//! 把 [`Report`] 渲染成一页**给人看、能当 demo 秀**的自包含 HTML（设计稿十一节「报告打磨」）。
//!
//! 裸 JSON 地板报告是给机器和工程师对账用的；这一层把它翻成一页非程序员也读得懂的网页：
//! - **自包含**：CSS 全内联，不引任何外部 CDN/JS 库，离线能开、确定性（同一份 Report 必出同一页）；
//! - **图表手画内联 SVG**（柱状/直方），不引图表库；
//! - **说人话**：开头先讲清几个术语，正文行话全用中文直白说法，英文/字段名只在和 JSON/代码
//!   对照处首次括号备注一次；
//! - **诚实标注**：启发式候选（软锁/惰性动作/数值崩/LLM note）一律标「候选，待人复核」，
//!   不说得像铁定结论。
//!
//! 代表录像仍由 `externalize_recordings` 落进 report-dir，本页只挂相对链接（`RecordingRef.path`）。

use crate::report::{
    DominantStrategy, EndingCoverage, NumericBreakage, OutcomeDistribution, Pacing,
    QualitativeNotes, RecordingRef, Reachability, Report, StuckCluster,
};
use crate::scene_view::Outcome;

/// 把一份报告渲染成一整页自包含 HTML 字符串。`project_name` 只用于标题展示。
pub fn report_to_html(report: &Report, project_name: &str) -> String {
    let mut body = String::new();

    body.push_str(&render_header(&report.outcome_distribution, report.sessions, project_name));
    body.push_str(&render_glossary());
    body.push_str(&render_summary(&report.summary));
    body.push_str(&render_outcomes(&report.outcome_distribution));
    body.push_str(&render_reachability(&report.reachability, report.ending_coverage.as_ref()));
    body.push_str(&render_stuck(&report.stuck_clusters));
    body.push_str(&render_endings(report.ending_coverage.as_ref()));
    body.push_str(&render_inert(&report.inert_actions));
    body.push_str(&render_pacing(&report.pacing));
    body.push_str(&render_numeric(&report.numeric_breakage));
    body.push_str(&render_dominant(&report.dominant_strategy));
    body.push_str(&render_notes(&report.qualitative_notes));

    wrap_page(project_name, &body)
}

/// HTML 转义：防字段名/note 文本里的尖括号引号破坏页面结构。
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// 结局的中文直白说法。
fn outcome_zh(o: Outcome) -> &'static str {
    match o {
        Outcome::Win => "通关",
        Outcome::Lose => "失败",
        Outcome::Timeout => "超时",
    }
}

/// 整页骨架（含全部内联 CSS）。
fn wrap_page(project_name: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"zh-CN\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<title>{} · swarm 试玩报告</title>\n<style>\n{}\n</style>\n</head>\n<body>\n\
<main class=\"wrap\">\n{}\n<footer class=\"foot\">Vitric 确定性引擎 · swarm 自动试玩报告（同一份数据必出同一页）</footer>\n\
</main>\n</body>\n</html>\n",
        esc(project_name),
        CSS,
        body
    )
}

/// 全部内联样式——无外部依赖。
const CSS: &str = r#"
* { box-sizing: border-box; }
body { margin: 0; background: #0f1115; color: #e6e9ef;
  font-family: -apple-system, "PingFang SC", "Microsoft YaHei", Segoe UI, sans-serif; line-height: 1.6; }
.wrap { max-width: 880px; margin: 0 auto; padding: 32px 20px 64px; }
.hero { background: linear-gradient(135deg, #1b2536, #131722); border: 1px solid #2a3550;
  border-radius: 16px; padding: 28px 28px 24px; margin-bottom: 28px; }
.hero h1 { margin: 0 0 4px; font-size: 18px; font-weight: 600; color: #8aa0c8; }
.hero .rate { font-size: 64px; font-weight: 800; line-height: 1; margin: 8px 0; }
.hero .rate small { font-size: 22px; font-weight: 600; color: #8aa0c8; margin-left: 6px; }
.hero .meta { color: #9fb0cc; font-size: 15px; }
section { background: #161a22; border: 1px solid #232a38; border-radius: 12px;
  padding: 20px 22px; margin-bottom: 18px; }
section > h2 { margin: 0 0 4px; font-size: 17px; color: #cdd6e6; }
section .hint { color: #8a93a6; font-size: 13px; margin: 0 0 14px; }
.empty { color: #6c7587; font-style: italic; }
.bar-row { display: flex; align-items: center; gap: 10px; margin: 6px 0; }
.bar-row .lbl { width: 86px; flex: none; font-size: 14px; color: #c2cbdc; }
.bar-row .val { font-size: 14px; color: #9fb0cc; min-width: 64px; }
table { width: 100%; border-collapse: collapse; font-size: 14px; }
th, td { text-align: left; padding: 7px 10px; border-bottom: 1px solid #232a38; }
th { color: #8a93a6; font-weight: 600; }
code { background: #0d1018; border: 1px solid #232a38; border-radius: 5px;
  padding: 1px 6px; font-size: 13px; color: #e0b487; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }
a { color: #6fb0ff; }
.cand { display: inline-block; background: #3a2a16; color: #f0c27b; border: 1px solid #6b4d1f;
  border-radius: 4px; padding: 0 6px; font-size: 12px; margin-left: 8px; }
.glossary dt { color: #cdd6e6; font-weight: 600; margin-top: 10px; }
.glossary dd { margin: 0 0 2px; color: #aab4c6; font-size: 14px; }
.glossary { margin: 0; }
.summary { color: #d6deea; font-size: 15px; }
svg { display: block; }
"#;

/// 画一条横向 SVG 柱（手画，不引图表库）。`frac` 是 0..1 的填充比例。
fn svg_bar(frac: f64, color: &str) -> String {
    let w = 360.0_f64;
    let fill = (frac.clamp(0.0, 1.0) * w).round();
    format!(
        "<svg width=\"{w:.0}\" height=\"18\" viewBox=\"0 0 {w:.0} 18\" role=\"img\">\
<rect x=\"0\" y=\"3\" width=\"{w:.0}\" height=\"12\" rx=\"3\" fill=\"#232a38\"/>\
<rect x=\"0\" y=\"3\" width=\"{fill:.0}\" height=\"12\" rx=\"3\" fill=\"{color}\"/></svg>"
    )
}

/// 顶部大字报：通关率 + 跑了几局/几种策略。
fn render_header(dist: &OutcomeDistribution, sessions: usize, project_name: &str) -> String {
    format!(
        "<div class=\"hero\">\n<h1>{} · swarm 自动试玩</h1>\n\
<div class=\"rate\">{:.0}%<small>通关率（win_rate）</small></div>\n\
<div class=\"meta\">一共跑了 {} 局：通关 {} · 失败 {} · 超时 {}</div>\n</div>\n",
        esc(project_name),
        dist.win_rate * 100.0,
        sessions,
        dist.win,
        dist.lose,
        dist.timeout
    )
}

/// 「先讲清楚几个词」——每个术语一句大白话定义，后文直接用。
fn render_glossary() -> String {
    let terms: &[(&str, &str)] = &[
        ("swarm（集群试玩）", "让一批自动玩家用不同策略、不同随机种子把这游戏反复玩很多局，看会玩出什么毛病。"),
        ("软锁（stuck）", "游戏卡进了一个再也赢不了、画面也不再变化的死局——人玩到这儿只能重开。"),
        ("数值跑飞（runaway）", "某个数（比如金币）无界地一直涨停不下来，等于经济崩了。"),
        ("数值崩盘（collapse）", "某个资源归零之后世界就冻住了，再也动不了。"),
        ("一招鲜（dominant）", "某个动作或策略碾压其他所有玩法，别的选择都没意义了。"),
        ("不可达结局（unreachable）", "游戏声明了某个结局，但任何玩法都到不了它。"),
        ("惰性动作（inert）", "声明了某个操作，但它其实没用、按了什么都不会发生。"),
        ("前瞻策略（lookahead）", "会「先试走几步再回退、挑最优那条路」的聪明玩法，适合导航/技巧类游戏。"),
        ("节奏（pacing）", "一局玩多久（到通关或失败用了多少 tick）。"),
        ("候选，待人复核", "这一条是机器或 LLM 启发式猜出来的疑点，不是铁定结论，需要人回放确认。"),
    ];
    let mut dl = String::from(
        "<section>\n<h2>先讲清楚几个词</h2>\n\
<p class=\"hint\">下面这些词后文会直接用，先一句话说清。</p>\n<dl class=\"glossary\">\n",
    );
    for (t, d) in terms {
        dl.push_str(&format!("<dt>{}</dt>\n<dd>{}</dd>\n", esc(t), esc(d)));
    }
    dl.push_str("</dl>\n</section>\n");
    dl
}

/// 人话摘要原样展示（聚合器已经写成人话）。
fn render_summary(summary: &str) -> String {
    format!(
        "<section>\n<h2>一句话总结</h2>\n<p class=\"summary\">{}</p>\n</section>\n",
        esc(summary)
    )
}

/// 通关情况：Win/Lose/超时三条 SVG 柱。
fn render_outcomes(dist: &OutcomeDistribution) -> String {
    let total = dist.total.max(1) as f64;
    let row = |lbl: &str, n: usize, color: &str| {
        format!(
            "<div class=\"bar-row\"><span class=\"lbl\">{lbl}</span>{}<span class=\"val\">{n} 局</span></div>\n",
            svg_bar(n as f64 / total, color)
        )
    };
    format!(
        "<section>\n<h2>通关情况</h2>\n<p class=\"hint\">每局最后是赢了、输了、还是跑满上限也没结束（超时 timeout）。</p>\n{}{}{}</section>\n",
        row("通关 win", dist.win, "#46c98b"),
        row("失败 lose", dist.lose, "#e06b6b"),
        row("超时 timeout", dist.timeout, "#d6a14a"),
    )
}

/// 可达性提示：swarm 一局都没通关时的强信号。
fn render_reachability(reach: &Reachability, _ending: Option<&EndingCoverage>) -> String {
    let mut s = String::from("<section>\n<h2>能不能赢</h2>\n");
    if reach.unbeatable_by_swarm {
        s.push_str(
            "<p class=\"hint\">swarm 一局都没通关——声明了能赢，但这批自动玩家谁也赢不到，疑似通关条件根本到不了。<span class=\"cand\">候选，待人复核</span></p>\n",
        );
    } else {
        s.push_str("<p class=\"hint\">至少有玩家通了关，通关路径是走得通的。</p>\n");
    }
    if reach.reached_events.is_empty() {
        s.push_str("<p class=\"empty\">没记录到任何终止/里程碑事件。</p>\n");
    } else {
        let evs: Vec<String> =
            reach.reached_events.iter().map(|e| format!("<code>{}</code>", esc(e))).collect();
        s.push_str(&format!(
            "<p class=\"hint\">玩到过的事件（fired events）：{}</p>\n",
            evs.join(" ")
        ));
    }
    s.push_str("</section>\n");
    s
}

/// 一条代表录像链接（落盘后 path 有值才挂链接）。
fn recording_link(r: &RecordingRef) -> String {
    let label = format!("回放（{} tick · {}）", r.ticks, outcome_zh(r.outcome));
    match &r.path {
        Some(p) => format!("<a href=\"{}\">{}</a>", esc(p), esc(&label)),
        None => format!("<span class=\"empty\">{}（未落盘）</span>", esc(&label)),
    }
}

/// 卡死的地方：软锁簇 + 命中局数 + 录像链接。
fn render_stuck(clusters: &[StuckCluster]) -> String {
    let mut s = String::from(
        "<section>\n<h2>卡死的地方（软锁候选）</h2>\n\
<p class=\"hint\">这些局玩到末尾画面就再也不变、也没到结局——疑似卡进了赢不了的死局。<span class=\"cand\">候选，待人复核</span>（有些游戏合法静止，需回放确认）。</p>\n",
    );
    if clusters.is_empty() {
        s.push_str("<p class=\"empty\">无——没发现卡死的局。</p>\n");
    } else {
        s.push_str("<table>\n<tr><th>死态哈希</th><th>命中局数</th><th>代表策略/种子</th><th>回放</th></tr>\n");
        for c in clusters {
            s.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{} 局</td><td><code>{}</code> / seed {}</td><td>{}</td></tr>\n",
                esc(&c.frozen_hash),
                c.hits,
                esc(&c.sample_strategy),
                c.sample_seed,
                recording_link(&c.representative)
            ));
        }
        s.push_str("</table>\n");
    }
    s.push_str("</section>\n");
    s
}

/// 到不了的结局。
fn render_endings(ending: Option<&EndingCoverage>) -> String {
    let mut s = String::from(
        "<section>\n<h2>到不了的结局（不可达）</h2>\n\
<p class=\"hint\">游戏声明能产出哪些结局、swarm 实际到了哪些，还有哪些声明了却任何玩法都到不了。</p>\n",
    );
    match ending {
        None => s.push_str("<p class=\"empty\">无——这次没算结局覆盖（没传规则信息）。</p>\n"),
        Some(ec) if ec.declared_endings.is_empty() => {
            s.push_str("<p class=\"empty\">无——这游戏没声明任何结局事件。</p>\n");
        }
        Some(ec) => {
            let join = |v: &[String]| {
                if v.is_empty() {
                    "<span class=\"empty\">无</span>".to_string()
                } else {
                    v.iter().map(|e| format!("<code>{}</code>", esc(e))).collect::<Vec<_>>().join(" ")
                }
            };
            s.push_str(&format!(
                "<table>\n<tr><th>声明的结局</th><td>{}</td></tr>\n\
<tr><th>到过的结局</th><td>{}</td></tr>\n\
<tr><th>到不了的结局</th><td>{}</td></tr>\n</table>\n",
                join(&ec.declared_endings),
                join(&ec.reached_endings),
                join(&ec.unreachable_endings),
            ));
            if !ec.unreachable_endings.is_empty() {
                s.push_str("<p class=\"hint\">声明了却到不了，疑似 flag/触发条件写漏。<span class=\"cand\">候选，待人复核</span></p>\n");
            }
        }
    }
    s.push_str("</section>\n");
    s
}

/// 没用的道具/动作（惰性动作）。
fn render_inert(inert: &[String]) -> String {
    let mut s = String::from(
        "<section>\n<h2>没用的动作（惰性候选）</h2>\n\
<p class=\"hint\">这些操作声明了，但 swarm 玩下来没发现它产生任何效果——疑似按了也白按。<span class=\"cand\">候选，待人复核</span>（也可能是合法地不产生可观测效果）。</p>\n",
    );
    if inert.is_empty() {
        s.push_str("<p class=\"empty\">无——没发现明显没用的动作。</p>\n");
    } else {
        let items: Vec<String> =
            inert.iter().map(|a| format!("<code>{}</code>", esc(a))).collect();
        s.push_str(&format!("<p>{}</p>\n", items.join(" ")));
    }
    s.push_str("</section>\n");
    s
}

/// 节奏：到终止的 tick 直方图（手画 SVG 柱）。
fn render_pacing(p: &Pacing) -> String {
    let mut s = String::from(
        "<section>\n<h2>节奏（玩多久）</h2>\n\
<p class=\"hint\">到结局（通关或失败）的局用了多少 tick——越靠左结束越快。超时的局单列、不混进来。</p>\n",
    );
    match (p.terminated_min, p.terminated_median, p.terminated_max) {
        (Some(min), Some(med), Some(max)) => {
            s.push_str(&format!(
                "<p class=\"hint\">最快 {min} tick · 中位 {med} tick · 最慢 {max} tick；超时 {} 局没到结局。</p>\n",
                p.timeout_count
            ));
            let peak = p.histogram.iter().map(|b| b.count).max().unwrap_or(1).max(1) as f64;
            for b in &p.histogram {
                s.push_str(&format!(
                    "<div class=\"bar-row\"><span class=\"lbl\">≤{} tick</span>{}<span class=\"val\">{} 局</span></div>\n",
                    b.upper,
                    svg_bar(b.count as f64 / peak, "#5b8def"),
                    b.count
                ));
            }
        }
        _ => {
            s.push_str(&format!(
                "<p class=\"empty\">无终止局可统计（超时 {} 局）。</p>\n",
                p.timeout_count
            ));
        }
    }
    s.push_str("</section>\n");
    s
}

/// 数值问题：跑飞/崩盘/溢出三类候选。
fn render_numeric(n: &NumericBreakage) -> String {
    let mut s = String::from(
        "<section>\n<h2>数值问题（跑飞 / 崩盘 / 溢出）</h2>\n\
<p class=\"hint\">某个数无界暴涨（跑飞 runaway）、某资源归零后世界冻住（崩盘 collapse）、或出现了 inf/nan（溢出）。<span class=\"cand\">候选，待人复核</span>（合法的强成长曲线也可能像跑飞）。</p>\n",
    );
    if n.runaway.is_empty() && n.collapse.is_empty() && n.non_finite.is_empty() {
        s.push_str("<p class=\"empty\">无——没发现数值跑飞、崩盘或溢出。</p>\n");
        s.push_str("</section>\n");
        return s;
    }
    if !n.runaway.is_empty() {
        s.push_str("<h3 style=\"margin:8px 0 4px;font-size:15px;color:#c2cbdc\">数值跑飞（runaway）</h3>\n");
        s.push_str("<table>\n<tr><th>字段</th><th>命中局数</th><th>最高峰值</th><th>回放</th></tr>\n");
        for r in &n.runaway {
            s.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{} 局</td><td>{:.3e}</td><td>{}</td></tr>\n",
                esc(&r.field),
                r.hits,
                r.peak_max,
                recording_link(&r.representative)
            ));
        }
        s.push_str("</table>\n");
    }
    if !n.collapse.is_empty() {
        s.push_str("<h3 style=\"margin:14px 0 4px;font-size:15px;color:#c2cbdc\">数值崩盘（collapse）</h3>\n");
        s.push_str("<table>\n<tr><th>字段</th><th>命中局数</th><th>回放</th></tr>\n");
        for c in &n.collapse {
            s.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{} 局</td><td>{}</td></tr>\n",
                esc(&c.field),
                c.hits,
                recording_link(&c.representative)
            ));
        }
        s.push_str("</table>\n");
    }
    if !n.non_finite.is_empty() {
        s.push_str("<h3 style=\"margin:14px 0 4px;font-size:15px;color:#c2cbdc\">数值溢出（inf/nan）</h3>\n");
        s.push_str("<table>\n<tr><th>字段</th><th>命中局数</th><th>回放</th></tr>\n");
        for nf in &n.non_finite {
            s.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{} 局</td><td>{}</td></tr>\n",
                esc(&nf.field),
                nf.hits,
                recording_link(&nf.representative)
            ));
        }
        s.push_str("</table>\n");
    }
    s.push_str("</section>\n");
    s
}

/// 一招鲜：各策略表现 + 碾压标记 + 主导动作。
fn render_dominant(d: &DominantStrategy) -> String {
    let mut s = String::from(
        "<section>\n<h2>一招鲜（碾压性策略/动作）</h2>\n\
<p class=\"hint\">各策略各玩了多少局、通关率多少；如果某个策略或某个动作碾压其他所有玩法，别的选择就没意义了。<span class=\"cand\">候选，待人复核</span></p>\n",
    );
    if d.per_strategy.is_empty() {
        s.push_str("<p class=\"empty\">无策略数据。</p>\n");
    } else {
        s.push_str("<table>\n<tr><th>策略</th><th>局数</th><th>通关率</th><th>通关中位 tick</th></tr>\n");
        for st in &d.per_strategy {
            let med = st.median_win_ticks.map(|t| format!("{t}")).unwrap_or_else(|| "—".to_string());
            s.push_str(&format!(
                "<tr><td><code>{}</code></td><td>{}</td><td>{:.0}%</td><td>{}</td></tr>\n",
                esc(&st.strategy),
                st.sessions,
                st.win_rate * 100.0,
                med
            ));
        }
        s.push_str("</table>\n");
    }
    if let Some(name) = &d.dominant {
        s.push_str(&format!(
            "<p class=\"hint\">策略 <code>{}</code> 的通关率碾压其他——疑似一招鲜，选择意义存疑。<span class=\"cand\">候选，待人复核</span></p>\n",
            esc(name)
        ));
    }
    if let Some(da) = &d.dominant_action {
        s.push_str(&format!(
            "<p class=\"hint\">通关几乎全靠动作 <code>{}</code>（占注入 {:.0}%，{} 局通关）——疑似一招鲜，其他选择没意义。<span class=\"cand\">候选，待人复核</span></p>\n",
            esc(&da.action),
            da.share * 100.0,
            da.winning_sessions
        ));
    }
    if d.dominant.is_none() && d.dominant_action.is_none() {
        s.push_str("<p class=\"hint\">没发现碾压性的策略或动作，玩法选择是有意义的。</p>\n");
    }
    s.push_str("</section>\n");
    s
}

/// LLM 的主观提示，按 kind 分组。
fn render_notes(notes: &QualitativeNotes) -> String {
    let kind_zh = |k: &str| match k {
        "clarity" => "看不看得懂（clarity）",
        "continuity" => "前后连不连贯（continuity）",
        "choice" => "选择有没有意义（choice）",
        _ => "其他（other）",
    };
    let mut s = String::from(
        "<section>\n<h2>LLM 的主观提示</h2>\n\
<p class=\"hint\">这一整块是 LLM 拟人玩时的主观感受，不是真人判定，也不进确定性保证——LLM 觉得「看不懂/前后矛盾/选项没意义」，可能对、也可能是它自己没看懂。<span class=\"cand\">候选，待人复核</span></p>\n",
    );
    if notes.clusters.is_empty() {
        s.push_str("<p class=\"empty\">无——这次没有 LLM 提示（没跑 LLM 档或它没提意见）。</p>\n");
    } else {
        s.push_str("<table>\n<tr><th>类型</th><th>提示</th><th>出现次数</th><th>回放</th></tr>\n");
        for c in &notes.clusters {
            s.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{} 次</td><td>{}</td></tr>\n",
                esc(kind_zh(&c.kind)),
                esc(&c.text),
                c.count,
                recording_link(&c.representative)
            ));
        }
        s.push_str("</table>\n");
    }
    s.push_str("</section>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{
        CollapseField, DominantAction, DominantStrategy, EndingCoverage, NonFiniteField,
        NoteCluster, NumericBreakage, OutcomeDistribution, Pacing, QualitativeNotes, Reachability,
        Report, RunawayField, StrategyStats, StuckCluster,
    };
    use crate::report::HistogramBucket;
    use vitric_sim::Recording;

    /// 造一条落盘后的代表录像引用（path 有值，链接挂得上）。
    fn rec_ref(path: &str, ticks: u64, outcome: Outcome) -> RecordingRef {
        RecordingRef {
            path: Some(path.to_string()),
            ticks,
            outcome,
            key: "k".to_string(),
            recording: Recording::default(),
        }
    }

    /// 造一份「啥毛病都有」的报告，用来验各分区都渲得出。
    fn full_report() -> Report {
        Report {
            sessions: 8,
            outcome_distribution: OutcomeDistribution {
                win: 5,
                lose: 2,
                timeout: 1,
                total: 8,
                win_rate: 0.625,
            },
            reachability: Reachability {
                reached_events: vec!["run-complete".to_string(), "game-over".to_string()],
                unbeatable_by_swarm: false,
            },
            ending_coverage: Some(EndingCoverage {
                declared_endings: vec!["win-a".to_string(), "win-b".to_string()],
                reached_endings: vec!["win-a".to_string()],
                unreachable_endings: vec!["win-b".to_string()],
            }),
            stuck_clusters: vec![StuckCluster {
                frozen_hash: "0x00000000000003e7".to_string(),
                hits: 3,
                sample_strategy: "random".to_string(),
                sample_seed: 7,
                representative: rec_ref("stuck-x.json", 600, Outcome::Timeout),
            }],
            pacing: Pacing {
                terminated_min: Some(10),
                terminated_median: Some(40),
                terminated_max: Some(120),
                histogram: vec![
                    HistogramBucket { upper: 32, count: 4 },
                    HistogramBucket { upper: 120, count: 3 },
                ],
                timeout_count: 1,
            },
            inert_actions: vec!["wiggle".to_string()],
            dominant_strategy: DominantStrategy {
                per_strategy: vec![StrategyStats {
                    strategy: "greedy".to_string(),
                    sessions: 4,
                    win_rate: 0.75,
                    median_win_ticks: Some(35),
                }],
                dominant: Some("greedy".to_string()),
                dominant_action: Some(DominantAction {
                    action: "jump".to_string(),
                    share: 0.9,
                    winning_sessions: 5,
                }),
            },
            numeric_breakage: NumericBreakage {
                runaway: vec![RunawayField {
                    field: "treasury/Resources.gold".to_string(),
                    hits: 2,
                    peak_max: 1.5e8,
                    sample_strategy: "economy".to_string(),
                    sample_seed: 3,
                    representative: rec_ref("runaway-gold.json", 600, Outcome::Timeout),
                }],
                collapse: vec![CollapseField {
                    field: "bank/Resources.cash".to_string(),
                    hits: 1,
                    sample_strategy: "economy".to_string(),
                    sample_seed: 5,
                    representative: rec_ref("collapse-cash.json", 600, Outcome::Timeout),
                }],
                non_finite: vec![NonFiniteField {
                    field: "ratio/Resources.x".to_string(),
                    hits: 1,
                    sample_strategy: "random".to_string(),
                    sample_seed: 1,
                    representative: rec_ref("nf-x.json", 200, Outcome::Lose),
                }],
            },
            qualitative_notes: QualitativeNotes {
                total: 2,
                clusters: vec![NoteCluster {
                    kind: "continuity".to_string(),
                    text: "上一幕的角色突然消失了".to_string(),
                    count: 2,
                    sample_tick: 42,
                    sample_strategy: "llm".to_string(),
                    sample_seed: 0,
                    representative: rec_ref("note-cont.json", 88, Outcome::Timeout),
                }],
            },
            summary: "跑了 8 局：通关 5、失败 2、超时 1，通关率 63%。".to_string(),
        }
    }

    /// 造一份「啥毛病都没有」的空报告，验空维度显示「无」。
    fn empty_report() -> Report {
        Report {
            sessions: 4,
            outcome_distribution: OutcomeDistribution {
                win: 4,
                lose: 0,
                timeout: 0,
                total: 4,
                win_rate: 1.0,
            },
            reachability: Reachability {
                reached_events: vec![],
                unbeatable_by_swarm: false,
            },
            ending_coverage: None,
            stuck_clusters: vec![],
            pacing: Pacing {
                terminated_min: Some(10),
                terminated_median: Some(10),
                terminated_max: Some(10),
                histogram: vec![HistogramBucket { upper: 10, count: 4 }],
                timeout_count: 0,
            },
            inert_actions: vec![],
            dominant_strategy: DominantStrategy {
                per_strategy: vec![],
                dominant: None,
                dominant_action: None,
            },
            numeric_breakage: NumericBreakage {
                runaway: vec![],
                collapse: vec![],
                non_finite: vec![],
            },
            qualitative_notes: QualitativeNotes { total: 0, clusters: vec![] },
            summary: "跑了 4 局：通关 4、失败 0、超时 0，通关率 100%。".to_string(),
        }
    }

    #[test]
    fn html_has_skeleton_and_title() {
        let h = report_to_html(&full_report(), "echo");
        assert!(h.starts_with("<!DOCTYPE html>"), "缺 doctype");
        assert!(h.contains("<html"), "缺 <html>");
        assert!(h.contains("</html>"), "缺 </html>");
        assert!(h.contains("<style>"), "CSS 必须内联");
        // 自包含：不引任何外部 CDN/JS 库
        assert!(!h.contains("http://"), "不应引外部 http 资源");
        assert!(!h.contains("https://"), "不应引外部 https 资源");
        assert!(!h.contains("<script"), "不应引 JS");
        assert!(h.contains("echo"), "标题含项目名");
    }

    #[test]
    fn html_has_win_rate_number() {
        let h = report_to_html(&full_report(), "echo");
        // 63% 通关率（0.625 四舍五入到整数百分比）
        assert!(h.contains("63%"), "缺通关率数字: 应含 63%");
    }

    #[test]
    fn html_has_glossary_section() {
        let h = report_to_html(&full_report(), "echo");
        assert!(h.contains("先讲清楚几个词"), "缺术语定义区");
        assert!(h.contains("软锁"), "术语区缺软锁定义");
        assert!(h.contains("一招鲜"), "术语区缺一招鲜定义");
        assert!(h.contains("前瞻策略"), "术语区缺前瞻定义");
    }

    #[test]
    fn html_has_all_section_titles() {
        let h = report_to_html(&full_report(), "echo");
        for title in [
            "通关情况",
            "卡死的地方",
            "到不了的结局",
            "没用的动作",
            "节奏",
            "数值问题",
            "一招鲜",
            "LLM 的主观提示",
        ] {
            assert!(h.contains(title), "缺分区标题: {title}");
        }
    }

    #[test]
    fn html_has_stuck_and_recording_link() {
        let h = report_to_html(&full_report(), "echo");
        assert!(h.contains("0x00000000000003e7"), "缺软锁死态哈希");
        // 录像相对链接挂上
        assert!(h.contains("href=\"stuck-x.json\""), "缺软锁代表录像链接");
        assert!(h.contains("href=\"runaway-gold.json\""), "缺跑飞代表录像链接");
    }

    #[test]
    fn html_has_svg_chart_no_library() {
        let h = report_to_html(&full_report(), "echo");
        assert!(h.contains("<svg"), "图表必须用内联 SVG");
        assert!(h.contains("<rect"), "SVG 柱用 rect 手画");
    }

    #[test]
    fn html_honest_candidate_marking() {
        let h = report_to_html(&full_report(), "echo");
        // 启发式候选都标「候选，待人复核」
        assert!(h.contains("候选，待人复核"), "缺诚实候选标注");
    }

    #[test]
    fn html_empty_dimensions_show_wu() {
        let h = report_to_html(&empty_report(), "clean");
        // 空维度显示「无」，不留空白
        assert!(h.contains(">无"), "空维度应显示「无」: {}", &h[..200]);
        // 没卡死/没惰性动作/没数值问题/没 LLM note 都应有「无」字样
        let wu_count = h.matches('无').count();
        assert!(wu_count >= 4, "空维度「无」太少: {wu_count}");
    }

    #[test]
    fn html_escapes_note_text() {
        let mut r = full_report();
        r.qualitative_notes.clusters[0].text = "<script>alert(1)</script>".to_string();
        let h = report_to_html(&r, "x");
        assert!(!h.contains("<script>alert(1)</script>"), "note 文本必须转义");
        assert!(h.contains("&lt;script&gt;"), "应转义成实体");
    }
}
