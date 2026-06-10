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

// ---- 确定性铁律：禁掉一切非确定性入口 ----
Math.random = function () {
  throw new Error("Math.random 在 Vitric 里禁用（会破坏确定性回放）。改用系统/函数回调里的 ctx.random()");
};
Date.now = function () {
  throw new Error("Date.now 在 Vitric 里禁用（会破坏确定性回放）。改用 ctx.tick（60 tick = 1 秒）");
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
  };
}

// Rust 侧入口：跑第 idx 个系统
globalThis.__runSystem = function (idx, payloadJson) {
  const sys = __systems[idx];
  const payload = JSON.parse(payloadJson);
  const rng = { state: BigInt(payload.rng.state), inc: BigInt(payload.rng.inc) };
  const ops = [];
  const ctx = __makeCtx(payload, ops, rng);
  sys.fn(payload.entities, ctx);
  return JSON.stringify({
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
  return JSON.stringify({ ops, rng: { state: rng.state.toString(), inc: rng.inc.toString() } });
};

// Rust 侧入口：枚举注册结果
globalThis.__list = function () {
  return JSON.stringify({
    systems: __systems.map((s) => ({ name: s.name, query: s.query, writes: s.writes })),
    fns: Object.keys(__fns),
  });
};
