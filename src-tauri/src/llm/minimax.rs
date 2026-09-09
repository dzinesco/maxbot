//! MiniMax chat-completions provider.
//!
//! Targets the international endpoint at `https://api.minimax.io/v1` by
//! default. Override with `SAND_MINIMAX_BASE_URL` (e.g. for the China
//! region `https://api.minimaxi.com/v1` or a self-hosted proxy). The
//! protocol is OpenAI-compatible, so all the wire-level translation
//! lives in `openai_compat.rs`.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::Stream;
use reqwest::Client;

use super::openai_compat;
use super::provider::{ChatRequest, Provider, ProviderKind};
use super::stream::{StreamChunk, StreamError};

/// Default model. Newest, 1M context, supports tools/vision.
pub const DEFAULT_MODEL: &str = "MiniMax-M3";

#[derive(Clone)]
pub struct MiniMaxProvider {
    pub api_key: Arc<str>,
    pub base_url: Arc<str>,
    pub http: Client,
}

impl MiniMaxProvider {
    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        let http = Client::builder()
            .user_agent("MaxBot/0.1 (https://maxbot.app)")
            .build()
            .expect("reqwest client");
        Self {
            api_key: Arc::from(api_key),
            base_url: Arc::from(base_url),
            http,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl Provider for MiniMaxProvider {
    fn name(&self) -> &'static str {
        "minimax"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::MiniMax
    }

    fn default_model(&self) -> &'static str {
        DEFAULT_MODEL
    }

    async fn stream(
        &self,
        request: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, StreamError>> + Send>>, StreamError> {
        openai_compat::stream_request(
            self.http.clone(),
            self.endpoint(),
            self.api_key.to_string(),
            "MaxBot",
            request,
        )
        .await
    }
}
