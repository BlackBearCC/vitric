// Vitric script runtime prelude — injected before any user script.
"use strict";

globalThis.__systems = [];
globalThis.__fns = {};

globalThis.vitric = {
  // Register a system: vitric.system("name", {query: [components...], writes: [components...]}, (entities, ctx) => {...} [, catch_up_fn])
  // query is the entity filter (the system can read these components); writes must be a subset of query (which it can change).
  // catch_up_fn is optional: (entity, ctx, dormant_ticks) => {...}. Invoked per matching entity when a region thaws after
  // being dormant, so the system can fast-forward entity state by the dormant tick budget instead of leaving it stale.
  // The catch_up_fn uses ctx.getField/ctx.setField (same as a rule `call` fn) — it does NOT receive the entity batch,
  // and writes via setField are NOT subject to the system's `writes` declaration (setField is the deferred-op channel).
  system(name, decl, fn, catch_up_fn) {
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
    // Optional 4th arg: catch_up(entity, ctx, dormant_ticks). null/undefined = system has no catch_up.
    // Any other type is an explicit error (catches `vitric.system(name, decl, fn, someNonFn)` typos).
    if (catch_up_fn !== undefined && catch_up_fn !== null && typeof catch_up_fn !== "function") {
      throw new Error("vitric.system(\"" + name + "\"): 第四个参数（可选 catch_up）必须是函数 (entity, ctx, dormant_ticks) => {...}，或省略/null");
    }
    if (__systems.some((s) => s.name === name)) {
      throw new Error("vitric.system: 系统名 \"" + name + "\" 已注册过，系统名全局唯一");
    }
    __systems.push({ name, query: decl.query, writes, fn, catch_up: catch_up_fn || null });
  },
  // Register a function callable by the rule `call` action: vitric.fn("name", (args, ctx) => {...})
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

// Built-in dispatcher: ctx.ask encodes the callback name into the id prefix; <service>-reply events are forwarded here to the matching vitric.fn.
// Wiring = the game adds a rule that passes the reply event's data as args (must include `with`, otherwise no id):
//   { "on": {"event": "llm-reply"}, "do": [{ "call": "__onReply", "with": { "id": "event.id", "text": "event.text" } }] }
// The reply the callback receives is { id, text } (on error the llm-error event's data is { id, message }, wire up a separate rule).
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

// ---- Determinism law: disable every non-deterministic entry point ----
Math.random = function () {
  throw new Error("Math.random 在 Vitric 里禁用（会破坏确定性回放）。改用系统/函数回调里的 ctx.random()");
};
// Date is replaced wholesale: parameterless construction reads the wall clock and breaks deterministic replay just like Date.now.
// Construction with explicit arguments (new Date(0)) is pure computation; allowed through.
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

// ---- PCG32 (same algorithm as Rust's vitric_sim::Pcg32; the random stream is continuous across languages) ----
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
    // Returns a named deterministic RNG substream: {next(), nextInt(min, max)}.
    // Same (world_seed, name) always produces the same sequence regardless of when this is
    // first called — replay-safe even if region thaw happens at different ticks across runs.
    // The state lives on the Rust Sim (HashMap<String, Substream>), persisted in snapshot and
    // restored exactly; the JS side is stateless, each call crosses the native bridge
    // __randomStreamNext → sim.random_stream(name).next_u32().
    //
    // Why a native bridge instead of a pure-JS PCG like ctx.random: the substream state must
    // persist across JS calls AND enter the snapshot (which lives on Sim, not on a JS object).
    // Crossing the boundary per-draw is the simplest way to keep state and snapshot in lockstep.
    random_stream: (name) => {
      if (typeof name !== "string" || !name) {
        throw new Error("ctx.random_stream: name 必须是非空字符串");
      }
      return {
        // [0, 1) float. u32 / 2^32 — matches Rust's Substream::next_f64.
        // Number(raw) is safe: u32 max (4294967295) < 2^53, no precision loss.
        next: () => {
          const raw = __randomStreamNext(name);
          return Number(raw) / 4294967296;
        },
        // [min, max] closed-interval integer. Mirrors Rust's Pcg32::range_i64:
        //   span = max - min + 1; min + (next_u32() % span)
        nextInt: (min, max) => {
          if (!Number.isInteger(min) || !Number.isInteger(max)) {
            throw new Error("ctx.random_stream.nextInt: min 和 max 必须是整数");
          }
          if (min > max) {
            throw new Error("ctx.random_stream.nextInt: min(" + min + ") > max(" + max + ")");
          }
          const raw = __randomStreamNext(name);
          const u = Number(raw);
          const span = max - min + 1;
          // Number is safe up to 2^53; span here is at most ~2^32 (u32 range), well within.
          // % on f64 is exact for integer operands < 2^53.
          return min + (u % span);
        },
      };
    },
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
    // Write one field of any entity "by name or handle" — "do something to whatever you point at" depends on this:
    // mouse events carry entity (name or handle) + comp; the script decides what was pointed at then writes it via setField.
    // Handle text (e.g. "e3v0") or entity name both work; like spawn/despawn, applied in order during the deterministic ops phase — replays are bit-identical.
    // The component/field must already exist (writing a sub-field does not implicitly create structure). Writes are deferred: a setField in the same round is not visible to a subsequent read.
    setField: (ref, path, value) => {
      if (typeof ref !== "string" || !ref) throw new Error("ctx.setField: 第一个参数必须是实体名字或句柄字符串");
      if (typeof path !== "string" || path.indexOf(".") < 0) throw new Error("ctx.setField: path 必须是 \"组件.字段\" 形式");
      ops.push({ op: "setField", ref: ref, path: path, value: value });
    },
    // Read one field value of an entity (same ref resolution as setField: handle or name).
    // The native __getFieldRaw looks up a single field on the live World directly (O(1)); no longer packs a full world snapshot on every call.
    // Returns undefined when the field/entity is missing; the value read reflects the world at the start of this call (consistent with setField's deferred-commit semantics).
    getField: (ref, path) => {
      if (typeof ref !== "string" || !ref) throw new Error("ctx.getField: 第一个参数必须是实体名字或句柄字符串");
      if (typeof path !== "string" || path.indexOf(".") < 0) throw new Error("ctx.getField: path 必须是 \"组件.字段\" 形式");
      const raw = __getFieldRaw(ref, path);
      return raw === "undefined" ? undefined : JSON.parse(raw);
    },
    // Thin wrapper for outbound questions: emits a <service>-ask event; on reply, the built-in dispatcher __onReply
    // forwards it to the vitric.fn named onReply. Underneath it's still raw ask/reply events + automatic replay recording; determinism is preserved.
    // The "receive" half requires the game to add a rule that routes <service>-reply into __onReply (see the __onReply comment at the top of the prelude).
    ask: (service, prompt, onReply) => {
      if (typeof service !== "string" || !service) throw new Error("ctx.ask: service 必须是非空字符串，如 'llm'");
      if (typeof prompt !== "string") throw new Error("ctx.ask: prompt 必须是字符串");
      if (typeof onReply !== "string" || !onReply) throw new Error("ctx.ask: 第三个参数必须是回复回调的函数名（用 vitric.fn 注册它）");
      if (onReply.indexOf("#") !== -1) throw new Error("ctx.ask: 回调名不能含 '#'（用作 id 分隔符）");
      // Deterministic id: callbackName#tick#emit-index-within-this-system. Doesn't use Math.random (disabled) and doesn't depend on a cross-snapshot global counter.
      const id = onReply + "#" + payload.tick + "#" + ops.length;
      ops.push({ op: "emit", name: service + "-ask", data: { id: id, prompt: prompt } });
      return id;
    },
  };
}

// ---- Field reads: ctx.getField goes through the native __getFieldRaw (directly queries the live World, see Rust side) ----
// No more world-snapshot parsing on the JS side: the Rust-registered __getFieldRaw(ref, path) returns that field's JSON string,
// or the literal "undefined" when the entity/field is missing. This way each read is single-field O(1), no full world packing per system/fn.

// ---- Lossless number serialization ----
// QuickJS's JSON.stringify printing of f64 is not shortest-round-trip (-7.3666666666666645 gets
// truncated to -7.366666666666664, off by one ULP); a round trip across the boundary silently drifts precision,
// and write detection would misjudge read-only systems as out-of-scope writes. The read direction (JSON.parse/strtod) is correctly rounded;
// the print direction toString/toPrecision share the same root cause and the text route can't be fully fixed — non-integers go through bit strings directly.
const __f64view = new DataView(new ArrayBuffer(8));
function __numStr(x) {
  if (!isFinite(x)) throw new Error("数值 " + x + " 无法写进世界（JSON 不支持 NaN/Infinity）");
  if (Number.isInteger(x) && Math.abs(x) < 9007199254740992) return String(x);
  // Non-integers don't go through text: QuickJS's dtoa (toString/toPrecision share the same source) is not shortest-round-trip,
  // textualization always loses a ULP. Export the IEEE754 bit string directly; the Rust side restores it bit-by-bit.
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

// Rust-side entry: run the idx-th system
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

// Rust-side entry: run the catch_up fn of system `idx` for a single entity. The entity is
// passed by handle (string like "e3v0"); the catch_up fn reads/writes via ctx.getField/ctx.setField
// (NOT via the entity batch — catch_up is per-entity, not per-query-batch). dormant_ticks is the
// number of ticks the entity's region spent dormant, the budget the fn fast-forwards by.
// Returns the same shape as __runSystem: {ops, rng} — ops are applied to the world by the Rust side.
globalThis.__runCatchUp = function (idx, entityHandle, dormantTicks, payloadJson) {
  const sys = __systems[idx];
  if (!sys.catch_up) {
    throw new Error("系统 " + JSON.stringify(sys.name) + " 没有 catch_up 函数，不应被调度");
  }
  const payload = JSON.parse(payloadJson);
  const rng = { state: BigInt(payload.rng.state), inc: BigInt(payload.rng.inc) };
  const ops = [];
  const ctx = __makeCtx(payload, ops, rng);
  // The entity is exposed to the fn as a handle string (e3v0) — same shape ctx.setField/ctx.getField take.
  sys.catch_up(entityHandle, ctx, dormantTicks);
  return __jsonStr({
    ops,
    rng: { state: rng.state.toString(), inc: rng.inc.toString() },
  });
};

// Rust-side entry: run the function targeted by a rule's `call` action
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
  ctx.self = payload.self; // entity handle bound at rule trigger time (may be null)
  f(payload.args, ctx);
  return __jsonStr({ ops, rng: { state: rng.state.toString(), inc: rng.inc.toString() } });
};

// Rust-side entry: enumerate registration results
globalThis.__list = function () {
  return JSON.stringify({
    systems: __systems.map((s) => ({
      name: s.name,
      query: s.query,
      writes: s.writes,
      // Whether this system declared a catch_up fn (4th arg to vitric.system). The Rust side
      // uses this to skip systems without catch_up when iterating for region thaw catch-up.
      catch_up: !!s.catch_up,
    })),
    fns: Object.keys(__fns),
  });
};
