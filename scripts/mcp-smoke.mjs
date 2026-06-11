// MCP server 冒烟测试：用真实 stdio 协议驱动一遍核心工具链。
// 用法：node scripts/mcp-smoke.mjs（在 mcp/ 目录下跑则用 ../，CI 也这么调）

import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const child = spawn("node", [path.join(repo, "mcp/index.js")], {
  env: { ...process.env, VITRIC_BIN: path.join(repo, "target/debug/vitric") },
  stdio: ["pipe", "pipe", "inherit"],
});

let buf = "";
const pending = new Map();
child.stdout.on("data", (d) => {
  buf += d;
  let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    const msg = JSON.parse(line);
    if (msg.id !== undefined && pending.has(msg.id)) pending.get(msg.id)(msg);
  }
});

let nextId = 1;
function send(method, params) {
  const id = nextId++;
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  return new Promise((res, rej) => {
    pending.set(id, res);
    setTimeout(() => rej(new Error(`${method} 超时`)), 15000);
  });
}
const t = (r) => JSON.parse(r.result.content[0].text);

function assert(cond, msg) {
  if (!cond) {
    console.error(`冒烟失败: ${msg}`);
    process.exit(1);
  }
}

const init = await send("initialize", {
  protocolVersion: "2024-11-05",
  capabilities: {},
  clientInfo: { name: "smoke", version: "0" },
});
assert(init.result.serverInfo.name === "vitric", "initialize");
child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized", params: {} }) + "\n");

const tools = await send("tools/list", {});
assert(tools.result.tools.length >= 14, `工具数 ${tools.result.tools.length}`);

const example = path.join(repo, "examples/coin-run");
const check = await send("tools/call", { name: "vitric_check", arguments: { project_dir: example } });
assert(t(check).project === "coin-run", "check");

// 班子工具：vitric_role 取美术工单（非空 + 真的是带地盘纪律的工单 + 占位符已替换）
const role = await send("tools/call", { name: "vitric_role", arguments: { role: "art", project_dir: example } });
const roleText = role.result.content[0].text;
assert(roleText.length > 0 && roleText.includes("地盘"), "vitric_role 工单缺地盘节");
assert(!roleText.includes("{PROJECT_DIR}") && roleText.includes(example), "vitric_role 占位符未替换");

// vitric_team 协同黑板：JSON 可解析且各角色计数在场
const team = await send("tools/call", { name: "vitric_team", arguments: { project_dir: example } });
assert(t(team).roles.art.assets > 0 && t(team).roles.gameplay.rules > 0, "vitric_team 计数");

await send("tools/call", { name: "vitric_start", arguments: { project_dir: example } });
await send("tools/call", { name: "vitric_input", arguments: { action: "right" } });
const step = await send("tools/call", { name: "vitric_step", arguments: { ticks: 60 } });
assert(t(step).tick >= 60, "step");
const player = await send("tools/call", { name: "vitric_world", arguments: { op: "get", entity: "@player" } });
assert(t(player).components.Score.value === 3, `通关分数应为 3，实际 ${t(player).components.Score.value}`);
const obs = await send("tools/call", { name: "vitric_observe", arguments: {} });
assert(t(obs).text.includes("相机"), "observe");
await send("tools/call", { name: "vitric_stop", arguments: {} });

console.log("MCP 冒烟通过：工具", tools.result.tools.length, "个，coin-run 通关验证 OK");
child.kill();
process.exit(0);
