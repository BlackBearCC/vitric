// Vitric 脚本运行时 prelude — 在任何用户脚本之前注入。
"use strict";

globalThis.__systems = [];
globalThis.__fns = {};

globalThis.vitric = {
  // 注册一个系统：vitric.system("名字", {query: [组件...], writes: [组件...]}, (entities, ctx) => {...})
  // query 是实体筛选（系统能读这些组件），writes 必须是 query 的子集（能改哪些）。
  system(name, decl, fn) {
    if (typeof name !== "string" || !name) {
      throw new Error("vitric.system: 第一个参数必须是非空系统名");
    }
    if (!decl || !Array.isArray(decl.query) || decl.query.length === 0) {
      throw new Error("vitric.system(\"" + name + "\"): 第二个参数必须含非空 query 数组，声明系统要处理哪些组件");
    }
    const writes = decl.writes || [];
    for (const w of writes) {
      if (!decl.query.includes(w)) {
        throw new Error("vitric.system(\"" + name + "\"): writes 里的 \"" + w + "\" 不在 query 里。能写的组件必须先在 query 里声明");
      }
    }
    if (typeof fn !== "function") {
      throw new Error("vitric.system(\"" + name + "\"): 第三个参数必须是函数 (entities, ctx) => {...}");
    }
    if (__systems.some((s) => s.name === name)) {
      throw new Error("vitric.system: 系统名 \"" + name + "\" 已注册过，系统名全局唯一");
    }
    __systems.push({ name, query: decl.query, writes, fn });
  },
  // 注册一个可被规则 call 动作调用的函数：vitric.fn("名字", (args, ctx) => {...})
  fn(name, f) {
    if (typeof name !== "string" || !name) {
      throw new Error("vitric.fn: 第一个参数必须是非空函数名");
    }
    if (typeof f !== "function") {
      throw new Error("vitric.fn(\"" + name + "\"): 第二个参数必须是函数 (args, ctx) => {...}");
    }
    __fns[name] = f;
  },
};

// 内置分发器：ctx.ask 把回调名编进 id 前段，<service>-reply 事件经此转发到对应 vitric.fn。
// 接通方式 = 游戏在规则里加一条，把回复事件的 data 当 args 传进来（务必带 with，否则没 id）：
//   { "on": {"event": "llm-reply"}, "do": [{ "call": "__onReply", "with": { "id": "event.id", "text": "event.text" } }] }
// 回调拿到的 reply 就是 { id, text }（出错时 llm-error 事件的 data 是 { id, message }，另接一条规则）。
vitric.fn("__onReply", (args, ctx) => {
  const id = (args && args.id) || "";
  const cb = id.split("#")[0];
  if (!cb) {
    throw new Error("__onReply: 回复缺 id 或 id 不含回调名（应形如 \"回调名#tick#n\"）。这条回复不是 ctx.ask 发出的？");
  }
  const f = __fns[cb];
  if (!f) {
    throw new Error("__onReply: ctx.ask 指定的回调 \"" + cb + "\" 没注册。用 vitric.fn(\"" + cb + "\", (reply, ctx) => {...}) 注册它");
  }
  f(args, ctx);
});

// ---- 确定性铁律：禁掉一切非确定性入口 ----
Math.random = function () {
  throw new Error("Math.random 在 Vitric 里禁用（会破坏确定性回放）。改用系统/函数回调里的 ctx.random()");
};
// Date 整体替换：无参构造 = 读墙钟，和 Date.now 一样会击穿确定性回放。
// 显式传参的构造（new Date(0)）是纯计算，放行。
const __RealDate = Date;
function __clockError(what) {
  return new Error(what + " 在 Vitric 里禁用（会破坏确定性回放）。改用 ctx.tick（60 tick = 1 秒）");
}
globalThis.Date = function Date(...args) {
  if (new.target === undefined) throw __clockError("Date()");
  if (args.length === 0) throw __clockError("new Date()");
  return new __RealDate(...args);
};
globalThis.Date.prototype = __RealDate.prototype;
globalThis.Date.parse = __RealDate.parse.bind(__RealDate);
globalThis.Date.UTC = __RealDate.UTC.bind(__RealDate);
globalThis.Date.now = function () {
  throw __clockError("Date.now");
};

// ---- PCG32（与 Rust 侧 vitric_sim::Pcg32 完全同一算法，随机流跨语言连续）----
const __MULT = 6364136223846793005n;
const __MASK = 0xffffffffffffffffn;

function __pcgNext(s) {
  const old = s.state;
  s.state = (old * __MULT + s.inc) & __MASK;
  const xorshifted = Number((((old >> 18n) ^ old) >> 27n) & 0xffffffffn);
  const rot = Number(old >> 59n);
  return ((xorshifted >>> rot) | (xorshifted << (-rot & 31))) >>> 0;
}

function __pcgF64(s) {
  const hi = BigInt(__pcgNext(s));
  const lo = BigInt(__pcgNext(s));
  return Number((hi << 21n) | (lo >> 11n)) / 9007199254740992; // 2^53
}

function __makeCtx(payload, ops, rng) {
  return {
    dt: payload.dt,
    tick: payload.tick,
    random: () => __pcgF64(rng),
    emit: (name, data) => {
      if (typeof name !== "string" || !name) throw new Error("ctx.emit: 事件名必须是非空字符串");
      ops.push({ op: "emit", name, data: data === undefined ? {} : data });
    },
    spawn: (components, name) => {
      if (!components || typeof components !== "object") {
        throw new Error("ctx.spawn: 第一个参数必须是组件对象，如 {Position: {x:0,y:0}}");
      }
      ops.push({ op: "spawn", components, name: name === undefined ? null : name });
    },
    despawn: (id) => {
      if (typeof id !== "string") throw new Error("ctx.despawn: 参数必须是实体句柄字符串（实体对象上的 e.id）");
      ops.push({ op: "despawn", id });
    },
    // 对外问话的薄封装：发一条 <service>-ask 事件，回复回来时由内置分发器 __onReply
    // 转给名为 onReply 的 vitric.fn。底层仍是裸的 ask/reply 事件 + 自动录回放，确定性不变。
    // “收”那半要游戏在规则里加一条把 <service>-reply 转进 __onReply（见 prelude 顶部 __onReply 注释）。
    ask: (service, prompt, onReply) => {
      if (typeof service !== "string" || !service) throw new Error("ctx.ask: service 必须是非空字符串，如 'llm'");
      if (typeof prompt !== "string") throw new Error("ctx.ask: prompt 必须是字符串");
      if (typeof onReply !== "string" || !onReply) throw new Error("ctx.ask: 第三个参数必须是回复回调的函数名（用 vitric.fn 注册它）");
      if (onReply.indexOf("#") !== -1) throw new Error("ctx.ask: 回调名不能含 '#'（用作 id 分隔符）");
      // 确定性 id：回调名#tick#本次系统内的发射序号。不用 Math.random（已禁）、不依赖跨快照的全局计数。
      const id = onReply + "#" + payload.tick + "#" + ops.length;
      ops.push({ op: "emit", name: service + "-ask", data: { id: id, prompt: prompt } });
      return id;
    },
  };
}

// ---- 数字保真序列化 ----
// QuickJS 的 JSON.stringify 打印 f64 不是最短往返（-7.3666666666666645 会
// 被截成 -7.366666666666664，差一个 ULP），跨边界一来一回精度静默漂移，
// 写检测也会把只读系统误判成越权写。读方向（JSON.parse/strtod）是正确舍入的，
// 打印方向 toString/toPrecision 同源同病，文本路线修不干净——非整数直接走位串。
const __f64view = new DataView(new ArrayBuffer(8));
function __numStr(x) {
  if (!isFinite(x)) throw new Error("数值 " + x + " 无法写进世界（JSON 不支持 NaN/Infinity）");
  if (Number.isInteger(x) && Math.abs(x) < 9007199254740992) return String(x);
  // 非整数不走文本：QuickJS 的 dtoa（toString/toPrecision 同源）不是最短往返，
  // 文本化必丢 ULP。直接导出 IEEE754 位串，Rust 侧逐位还原。
  __f64view.setFloat64(0, x);
  const hi = __f64view.getUint32(0).toString(16).padStart(8, "0");
  const lo = __f64view.getUint32(4).toString(16).padStart(8, "0");
  return '{"$f64":"' + hi + lo + '"}';
}
function __jsonStr(v) {
  switch (typeof v) {
    case "number": return __numStr(v);
    case "string": return JSON.stringify(v);
    case "boolean": return v ? "true" : "false";
    case "undefined": return "null";
    case "object": {
      if (v === null) return "null";
      if (Array.isArray(v)) return "[" + v.map(__jsonStr).join(",") + "]";
      const parts = [];
      for (const k of Object.keys(v)) {
        if (v[k] === undefined) continue;
        parts.push(JSON.stringify(k) + ":" + __jsonStr(v[k]));
      }
      return "{" + parts.join(",") + "}";
    }
    default:
      throw new Error("无法序列化 " + typeof v + " 类型的值");
  }
}

// Rust 侧入口：跑第 idx 个系统
globalThis.__runSystem = function (idx, payloadJson) {
  const sys = __systems[idx];
  const payload = JSON.parse(payloadJson);
  const rng = { state: BigInt(payload.rng.state), inc: BigInt(payload.rng.inc) };
  const ops = [];
  const ctx = __makeCtx(payload, ops, rng);
  sys.fn(payload.entities, ctx);
  return __jsonStr({
    entities: payload.entities,
    ops,
    rng: { state: rng.state.toString(), inc: rng.inc.toString() },
  });
};

// Rust 侧入口：跑规则 call 动作指向的函数
globalThis.__callFn = function (name, payloadJson) {
  const f = __fns[name];
  if (!f) {
    throw new Error(
      "没有注册名为 \"" + name + "\" 的脚本函数。已注册: [" + Object.keys(__fns).join(", ") +
      "]。用 vitric.fn(\"" + name + "\", (args, ctx) => {...}) 注册"
    );
  }
  const payload = JSON.parse(payloadJson);
  const rng = { state: BigInt(payload.rng.state), inc: BigInt(payload.rng.inc) };
  const ops = [];
  const ctx = __makeCtx(payload, ops, rng);
  ctx.self = payload.self; // 触发规则时绑定的实体句柄（可能为 null）
  f(payload.args, ctx);
  return __jsonStr({ ops, rng: { state: rng.state.toString(), inc: rng.inc.toString() } });
};

// Rust 侧入口：枚举注册结果
globalThis.__list = function () {
  return JSON.stringify({
    systems: __systems.map((s) => ({ name: s.name, query: s.query, writes: s.writes })),
    fns: Object.keys(__fns),
  });
};
