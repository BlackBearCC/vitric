use serde::{Deserialize, Serialize};

/// 一条录下来的输入。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRecord {
    pub tick: u64,
    pub action: String,
    /// "pressed" | "released"
    pub phase: String,
}

/// 一局游戏的完整录像：种子 + 输入序列 + 校验点。
/// 这就是确定性的全部需要——重放它必然逐帧复现原局。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Recording {
    pub seed: u64,
    pub inputs: Vec<InputRecord>,
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
}
