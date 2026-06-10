use serde::{Deserialize, Serialize};

/// 一条录下来的输入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRecord {
    pub tick: u64,
    pub action: String,
    /// "pressed" | "released"
    pub phase: String,
}

/// 一条录下来的外部回复（LLM 回复这类异步到达的外部内容）。
/// 与输入同级：它是世界之外进入模拟的**第二条**也是最后一条通道。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyRecord {
    pub tick: u64,
    /// 事件名（约定 "llm-reply" / "llm-error"，但通道本身不限定）。
    pub name: String,
    /// 事件 data（JSON 对象）。
    pub data: serde_json::Value,
}

/// 一局游戏的完整录像：种子 + 输入序列 + 外部回复序列 + 校验点。
/// 这就是确定性的全部需要——重放它必然逐帧复现原局。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Recording {
    pub seed: u64,
    pub inputs: Vec<InputRecord>,
    /// 外部回复（LLM 等）。重放时按 tick 原样重新注入——重放永远不碰网络。
    /// serde(default)：旧录像没有这个字段，等价于「这局没有外部回复」，语义不变。
    #[serde(default)]
    pub replies: Vec<ReplyRecord>,
    /// 周期性状态哈希 (tick, hash)，重放时逐点比对，跑偏立即定位到区间。
    pub checkpoints: Vec<(u64, u64)>,
    /// 录像覆盖的总 tick 数。
    pub ticks: u64,
    /// 结束时的世界状态哈希。
    pub final_hash: u64,
}

impl Recording {
    /// 某 tick 的全部输入（录像按 tick 升序存）。
    pub fn inputs_at(&self, tick: u64) -> impl Iterator<Item = &InputRecord> {
        self.inputs.iter().filter(move |r| r.tick == tick)
    }

    /// 某 tick 的全部外部回复（录像按 tick 升序存）。
    pub fn replies_at(&self, tick: u64) -> impl Iterator<Item = &ReplyRecord> {
        self.replies.iter().filter(move |r| r.tick == tick)
    }
}
