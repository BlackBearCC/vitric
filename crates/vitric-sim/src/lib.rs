//! vitric-sim — 确定性模拟核心：固定步长、种子随机、输入录制与重放。
//!
//! 确定性是 Vitric 最硬的差异化：**同种子 + 同输入序列 = 逐帧相同的世界**。
//! 任何 bug 都能拿录像精确重放到出错前一帧。为此：
//! - 时间步长固定（1/60 秒），不吃墙钟；
//! - 随机数自实现 PCG32，状态可快照；
//! - 一切迭代顺序确定（依赖 vitric-ecs 的有序存储）；
//! - 录像存输入 + 外部回复（LLM 等异步内容，见 [`Sim::inject_reply`]）
//!   + 周期性状态哈希，重放时逐点校验，跑偏立刻定位。

mod pcg;
mod recording;
mod sim;

pub use pcg::Pcg32;
pub use recording::{InputRecord, Recording, ReplyRecord};
pub use sim::{GameLogic, Sim, SimError, StepReport, DT, TICKS_PER_SECOND};
