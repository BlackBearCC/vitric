//! Real client for the playtest LLM tier (design doc §11 phase 5) — connects the abstract
//! [`LlmClient`] defined by vitric-playtest to a real LLM endpoint (OpenAI-compatible
//! chat/completions).
//!
//! **Dependency direction**: the abstract trait lives in vitric-playtest; the real
//! implementation lives here (vitric-cli already depends on playtest, no cycle). Assembly
//! reads only `VITRIC_LLM_*` environment variables (same set as the runtime LLM, no secrets
//! on disk); if not fully configured, it **returns an explicit error** and `cmd_playtest`'s
//! `--llm` refuses to run — no silent skip (design doc: "failures surface explicitly").
//!
//! Synchronous blocking call (reuses `llm::complete_sync`): the LLM tier is inherently slow,
//! separately rate-limited, and not in the game frame loop (design doc §9), so synchronous
//! is the most straightforward. Without `--llm` this module is never constructed; ordinary
//! playtests don't touch the network.

use vitric_playtest::LlmClient;

use crate::llm::{complete_sync, LlmConfig};

/// Real LLM client: holds endpoint configuration; each `complete` sends one synchronous HTTP call.
pub struct PlaytestLlmClient {
    cfg: LlmConfig,
}

impl PlaytestLlmClient {
    /// Assemble from `VITRIC_LLM_*` environment variables. Any of the three missing → explicit
    /// error (used by `--llm` as the "VITRIC_LLM_URL/KEY/MODEL not configured" message; no
    /// silent skip).
    pub fn from_env() -> Result<PlaytestLlmClient, String> {
        let cfg = LlmConfig::from_env()
            .map_err(|_| "未配 VITRIC_LLM_URL/KEY/MODEL：playtest --llm 需要这三个环境变量配齐才能跑 LLM 档".to_string())?;
        Ok(PlaytestLlmClient { cfg })
    }
}

impl LlmClient for PlaytestLlmClient {
    fn complete(&self, prompt: &str) -> Result<String, String> {
        complete_sync(&self.cfg, prompt)
    }
}
