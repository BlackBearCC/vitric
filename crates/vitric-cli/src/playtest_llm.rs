//! playtest LLM 档的真实客户端（设计稿十一节第 5 阶段）——把 vitric-playtest 定义的抽象
//! [`LlmClient`] 接到真 LLM 端点（OpenAI 兼容 chat/completions）。
//!
//! **依赖方向**：抽象 trait 在 vitric-playtest，真实现住这儿（vitric-cli 已依赖 playtest，
//! 不成环）。装配只认 `VITRIC_LLM_*` 环境变量（和运行时 LLM 同一套，密钥不落盘）；没配齐
//! 就**报明确错误**，由 `cmd_playtest` 的 `--llm` 拒绝运行——不静默跳过（设计稿「失败显式暴露」）。
//!
//! 同步阻塞调用（复用 `llm::complete_sync`）：LLM 档本就慢、单独限流、不在游戏帧里
//! （设计稿九节），同步最直白。无 `--llm` 时这模块根本不构造，普通 playtest 不碰网络。

use vitric_playtest::LlmClient;

use crate::llm::{complete_sync, LlmConfig};

/// 真 LLM 客户端：持有端点配置，每次 `complete` 同步发一次 HTTP。
pub struct PlaytestLlmClient {
    cfg: LlmConfig,
}

impl PlaytestLlmClient {
    /// 按 `VITRIC_LLM_*` 环境变量装配。三个变量缺任一 → 返明确错误
    /// （拿去给 `--llm` 当「未配 VITRIC_LLM_URL/KEY/MODEL」的报错，不静默跳过）。
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
