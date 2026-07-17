//! vitric-rules — a "when X then Y" declarative rule engine.
//!
//! Rules are the front door of gameplay: 80% of game logic should be a direct transcription of rules.
//! Deliberately **not Turing-complete**: conditions are only comparisons and AND (an array means "all hold"),
//! no loops, no variables. Logic that can't be expressed falls to the script system (`call` action);
//! this is a design decision, not a defect — to prevent the rule language from growing into a bad programming language.
//!
//! Rule JSON looks like:
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
//! Path syntax: `self.component.field` / `other.…` / `@entity-name.…` / `event.field`.
//! Strings starting with `self.` / `other.` / `@` / `event.` are parsed as references; otherwise they are literals.

mod engine;
mod model;

pub use engine::{Engine, RuleError, ScriptCall, TickOutput};
pub use model::{input_actions, Event, InputAction, Rule, RuleSet, Trigger};
