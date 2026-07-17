use serde::{Deserialize, Serialize};

/// A recorded input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputRecord {
    pub tick: u64,
    pub action: String,
    /// "pressed" | "released"
    pub phase: String,
}

/// A recorded external reply (async-arriving external content such as an LLM reply).
/// Same level as inputs: it is the **second** and last channel for content outside the world to enter the simulation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyRecord {
    pub tick: u64,
    /// Event name (convention: "llm-reply" / "llm-error", but the channel itself doesn't enforce).
    pub name: String,
    /// Event data (JSON object).
    pub data: serde_json::Value,
}

/// A complete recording of a run: seed + input sequence + external reply sequence + checkpoints.
/// This is everything determinism needs — replaying it must reproduce the original run frame by frame.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Recording {
    pub seed: u64,
    pub inputs: Vec<InputRecord>,
    /// External replies (LLM, etc.). Re-injected at the original tick during replay — replay never touches the network.
    /// serde(default): old recordings without this field are equivalent to "this run has no external replies"; semantics unchanged.
    #[serde(default)]
    pub replies: Vec<ReplyRecord>,
    /// Periodic state hashes (tick, hash), compared point by point during replay; divergence is located to the interval immediately.
    pub checkpoints: Vec<(u64, u64)>,
    /// Total number of ticks the recording covers.
    pub ticks: u64,
    /// World state hash at the end.
    pub final_hash: u64,
}

impl Recording {
    /// All inputs at a given tick (the recording stores them in ascending tick order).
    pub fn inputs_at(&self, tick: u64) -> impl Iterator<Item = &InputRecord> {
        self.inputs.iter().filter(move |r| r.tick == tick)
    }

    /// All external replies at a given tick (the recording stores them in ascending tick order).
    pub fn replies_at(&self, tick: u64) -> impl Iterator<Item = &ReplyRecord> {
        self.replies.iter().filter(move |r| r.tick == tick)
    }
}
