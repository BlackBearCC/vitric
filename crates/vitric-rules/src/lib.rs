//! vitric-rules — 「当 X 则 Y」声明式规则引擎。
//!
//! 规则是玩法的正门：80% 的游戏逻辑应该是规则直译。
//! 刻意**不图灵完备**：条件只有比较和与（数组即「全部成立」），
//! 没有循环没有变量。写不动的逻辑落到脚本系统（`call` 动作），
//! 这是设计决策不是缺陷——防止规则语言长成一门烂编程语言。
//!
//! 规则 JSON 形如：
//! ```json
//! {
//!   "id": "collect-coin",
//!   "on": {"event": "collision", "between": ["Player", "Coin"]},
//!   "if": [["other.Coin.value", ">", 0]],
//!   "do": [
//!     {"add": "self.Score.value", "by": "other.Coin.value"},
//!     {"despawn": "other"},
//!     {"emit": "coin-collected", "data": {"who": "self"}}
//!   ]
//! }
//! ```
//!
//! 路径语法：`self.组件.字段` / `other.…` / `@实体名.…` / `event.字段`。
//! 字符串以 `self.`/`other.`/`@`/`event.` 开头按引用解析，否则是字面量。

mod engine;
mod model;

pub use engine::{Engine, RuleError, ScriptCall, TickOutput};
pub use model::{Event, Rule, RuleSet, Trigger};
