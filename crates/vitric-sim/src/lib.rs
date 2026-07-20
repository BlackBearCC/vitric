//! vitric-sim — deterministic simulation core: fixed timestep, seeded random, input recording and replay.
//!
//! Determinism is Vitric's hardest differentiator: **same seed + same input sequence = same world frame by frame**.
//! Any bug can be reproduced precisely via a recording to the frame before the error. To achieve this:
//! - The timestep is fixed (1/60 second); wall-clock time never enters the simulation;
//! - Random numbers are implemented in-house as PCG32, and the state is snapshotable;
//! - All iteration orders are deterministic (relies on vitric-ecs's ordered storage);
//! - Recordings store inputs + external replies (async content such as LLM, see [`Sim::inject_reply`])
//!   + periodic state hashes, verified point by point during replay; divergence is located immediately.

mod pcg;
mod recording;
mod sim;
mod tween;

pub use pcg::{Pcg32, Substream};
pub use recording::{InputRecord, Recording, ReplyRecord};
pub use sim::{clear_sim_ptr, set_sim_ptr, with_sim_ptr, GameLogic, Sim, SimError, StepReport, DT, TICKS_PER_SECOND};
pub use tween::{tween_value, Ease, EASE_NAMES};
