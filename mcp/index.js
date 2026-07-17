#!/usr/bin/env node
// vitric-mcp — exposes the Vitric control plane as MCP tools.
// The engine has a single interface (HTTP control plane); this is a thin wrapper for MCP clients.
// Env var: VITRIC_BIN = path to the vitric executable (defaults to "vitric" on PATH).

import { spawn } from "node:child_process";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";

const VITRIC_BIN = process.env.VITRIC_BIN || "vitric";
// Role workorders ship with the engine: this file is in <repo>/mcp/, the single source of truth for workorders is <repo>/team/
const TEAM_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../team");

/** The currently running game process started by this server. */
let game = null; // { child, controlUrl, project }

function text(value) {
  const s = typeof value === "string" ? value : JSON.stringify(value, null, 2);
  return { content: [{ type: "text", text: s }] };
}

async function rpc(method, params = {}) {
  if (!game) {
    throw new Error("没有运行中的游戏。先用 vitric_start 启动一个项目");
  }
  const res = await fetch(game.controlUrl, {
    method: "POST",
    body: JSON.stringify({ method, params }),
  });
  const body = await res.json();
  if (!body.ok) {
    throw new Error(body.error);
  }
  return body.result;
}

const server = new McpServer({ name: "vitric", version: "0.1.0" });

server.tool(
  "vitric_check",
  "校验一个 Vitric 项目（schema/场景/规则/脚本/素材）。错误带路径+错误码+修复提示，一次报全。改完数据先跑这个。",
  { project_dir: z.string().describe("项目目录（含 vitric.json）") },
  async ({ project_dir }) => {
    return new Promise((resolve) => {
      const p = spawn(VITRIC_BIN, ["check", project_dir]);
      let out = "", err = "";
      p.stdout.on("data", (d) => (out += d));
      p.stderr.on("data", (d) => (err += d));
      p.on("close", (code) => resolve(text(code === 0 ? out : `校验失败:\n${err}`)));
      p.on("error", (e) => resolve(text(`无法执行 ${VITRIC_BIN}: ${e.message}（用 VITRIC_BIN 环境变量指定路径）`)));
    });
  }
);

/** Run a vitric subprocess, returning its stdout (on success) or stderr (on failure) as-is to the client. */
function runCli(args, { okExit = [0] } = {}) {
  return new Promise((resolve) => {
    const p = spawn(VITRIC_BIN, args);
    let out = "", err = "";
    p.stdout.on("data", (d) => (out += d));
    p.stderr.on("data", (d) => (err += d));
    p.on("close", (code) => resolve(text(okExit.includes(code) ? out : `${out}\n${err}`.trim())));
    p.on("error", (e) => resolve(text(`无法执行 ${VITRIC_BIN}: ${e.message}（用 VITRIC_BIN 环境变量指定路径）`)));
  });
}

server.tool(
  "vitric_team",
  "多 agent 班子协同黑板：各角色（美术/关卡/玩法/音频/文案/QA）交付物健康度 + GDD/schema 合同与门禁声明状态 + 卡点提示（blocking）。只读状态工具，不是门禁。",
  { project_dir: z.string().describe("项目目录（含 vitric.json）") },
  async ({ project_dir }) => runCli(["team", project_dir])
);

server.tool(
  "vitric_role",
  "领取角色工单：返回引擎内置的 team/skills/vitric-<role>/SKILL.md 全文（多 agent 班子里该角色的完整 prompt，含先读清单/地盘/工序/验收门）。给了 project_dir 就把 {PROJECT_DIR} 占位符替换成真实路径，整篇可直接派给 subagent。",
  {
    role: z.enum(["art", "level", "gameplay", "audio", "narrative", "qa"]).describe("角色名"),
    project_dir: z.string().optional().describe("项目目录；给了就替换工单里的 {PROJECT_DIR} 占位符"),
  },
  async ({ role, project_dir }) => {
    const file = path.join(TEAM_DIR, "skills", `vitric-${role}`, "SKILL.md");
    let content;
    try {
      content = await readFile(file, "utf8");
    } catch (e) {
      const available = (await readdir(path.join(TEAM_DIR, "skills")).catch(() => []))
        .map((f) => f.replace(/^vitric-/, ""));
      return text(`读取工单 ${file} 失败: ${e.message}。可用角色: ${available.join(", ")}`);
    }
    if (project_dir) content = content.replaceAll("{PROJECT_DIR}", project_dir);
    return text(content);
  }
);

server.tool(
  "vitric_start",
  "启动一个 Vitric 游戏（无头或开窗口），自动接好控制面。同时只能跑一个，重复调用会先停掉旧的。",
  {
    project_dir: z.string().describe("项目目录"),
    window: z.boolean().optional().describe("true=开窗口（人能看），默认无头"),
    speed: z.number().optional().describe("初始倍速，默认 1.0"),
  },
  async ({ project_dir, window, speed }) => {
    if (game) {
      try { await rpc("sim/quit"); } catch {}
      game.child.kill();
      game = null;
    }
    const args = ["run", project_dir, "--port", "0"];
    if (window) args.push("--window");
    if (speed) args.push("--speed", String(speed));
    const child = spawn(VITRIC_BIN, args, { stdio: ["ignore", "pipe", "pipe"] });
    const banner = await new Promise((resolve, reject) => {
      let buf = "";
      const onData = (d) => {
        buf += d;
        const nl = buf.indexOf("\n");
        if (nl >= 0) {
          child.stdout.off("data", onData);
          try { resolve(JSON.parse(buf.slice(0, nl))); } catch (e) { reject(new Error(`启动横幅解析失败: ${buf}`)); }
        }
      };
      child.stdout.on("data", onData);
      let err = "";
      child.stderr.on("data", (d) => (err += d));
      child.on("exit", (code) => reject(new Error(`vitric 启动即退出(code=${code}): ${err}`)));
      child.on("error", (e) => reject(new Error(`无法执行 ${VITRIC_BIN}: ${e.message}`)));
      setTimeout(() => reject(new Error("启动超时(10s)")), 10000);
    });
    game = { child, controlUrl: banner.control, project: banner.project };
    return text({ started: banner.project, control: banner.control, window: !!window });
  }
);

server.tool(
  "vitric_stop",
  "停止当前游戏进程。",
  {},
  async () => {
    if (!game) return text("没有运行中的游戏");
    try { await rpc("sim/quit"); } catch {}
    game.child.kill();
    const name = game.project;
    game = null;
    return text(`已停止 ${name}`);
  }
);

server.tool(
  "vitric_observe",
  "语义观察当前画面（主观察通道）：可见实体的方位/坐标/颜色/贴图、视觉遮挡、视野外实体的方向距离，附摘要。比截图精准，优先用它。",
  {
    width: z.number().optional(),
    height: z.number().optional(),
  },
  async (args) => text(await rpc("render/describe", args))
);

server.tool(
  "vitric_screenshot",
  "无头截图存成 PNG 文件（兜底验证：怀疑渲染本身有问题时用；平时用 vitric_observe）。",
  { path: z.string().describe("PNG 输出路径"), width: z.number().optional(), height: z.number().optional() },
  async (args) => text(await rpc("render/screenshot", args))
);

server.tool(
  "vitric_step",
  "暂停并确定性单步 N tick（自动先暂停）。返回里带新触发的断言失败。",
  { ticks: z.number().optional().describe("默认 1；60 tick = 1 秒") },
  async ({ ticks }) => {
    await rpc("sim/pause");
    return text(await rpc("sim/step", { ticks: ticks ?? 1 }));
  }
);

server.tool(
  "vitric_input",
  "注入游戏输入（下一 tick 生效）。",
  { action: z.string().describe("动作名，如 right/left/jump"), phase: z.enum(["pressed", "released"]).optional() },
  async (args) => text(await rpc("input/inject", args))
);

server.tool(
  "vitric_world",
  "查/改世界状态。op=entities 列实体(可按组件过滤)；get 查单个实体；set 改字段(过 schema)；spawn/despawn 生成/销毁。",
  {
    op: z.enum(["entities", "get", "set", "spawn", "despawn"]),
    entity: z.string().optional().describe("\"@名字\" 或句柄 \"e3v1\""),
    components: z.union([z.array(z.string()), z.record(z.any())]).optional()
      .describe("entities 时是过滤组件名数组；spawn 时是组件值对象"),
    path: z.string().optional().describe("set 用，如 \"Health.hp\""),
    value: z.any().optional(),
    name: z.string().optional().describe("spawn 的实体名"),
  },
  async ({ op, ...rest }) => text(await rpc(`world/${op}`, rest))
);

server.tool(
  "vitric_assert",
  "管理断言（每 tick 检查，违反自动上报）。op=add/remove/list/failures。",
  {
    op: z.enum(["add", "remove", "list", "failures"]),
    id: z.string().optional(),
    conditions: z.array(z.array(z.any())).optional()
      .describe("add 用：[[\"@player.Health.hp\", \">\", 0], ...]"),
  },
  async ({ op, id, conditions }) =>
    text(await rpc(`assert/${op}`, { id, if: conditions }))
);

server.tool(
  "vitric_time",
  "时间控制。op=pause/resume/speed(带 multiplier)/snapshot/restore(带 snapshot)/hash。",
  {
    op: z.enum(["pause", "resume", "speed", "snapshot", "restore", "hash"]),
    multiplier: z.number().optional(),
    snapshot: z.any().optional(),
  },
  async ({ op, multiplier, snapshot }) =>
    text(await rpc(`sim/${op}`, { multiplier, snapshot }))
);

server.tool(
  "vitric_reload",
  "热重载：把磁盘上改过的规则/脚本/素材换进正在跑的游戏，世界状态不动。失败保持旧逻辑。",
  {},
  async () => text(await rpc("project/reload"))
);

server.tool(
  "vitric_rpc",
  "控制面通用调用（其余工具没覆盖到的方法，如 events/recent、inspect/selection）。",
  { method: z.string(), params: z.record(z.any()).optional() },
  async ({ method, params }) => text(await rpc(method, params ?? {}))
);

const transport = new StdioServerTransport();
await server.connect(transport);
