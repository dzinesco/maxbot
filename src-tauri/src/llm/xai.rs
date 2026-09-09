//! xAI (Grok) chat-completions provider.
//!
//! Targets `https://api.x.ai/v1` by default. Wire format is OpenAI
//! chat completions, so all the heavy lifting lives in
//! `openai_compat.rs`. Models: `grok-2-latest`, `grok-2-vision-latest`,
//! `grok-beta`.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::Stream;
use reqwest::Client;

use super::openai_compat;
use super::provider::{ChatRequest, Provider, ProviderKind};
use super::stream::{StreamChunk, StreamError};

pub const DEFAULT_MODEL: &str = "grok-2-latest";

#[derive(Clone)]
pub struct XaiProvider {
    pub api_key: Arc<str>,
    pub base_url: Arc<str>,
    pub model: Arc<str>,
    pub http: Client,
}

impl XaiProvider {
    pub fn with_base_url(api_key: String, base_url: String, model: String) -> Self {
        let http = Client::builder()
            .user_agent("MaxBot/0.1 (https://maxbot.app)")
            .build()
            .expect("reqwest client");
        Self {
            api_key: Arc::from(api_key),
            base_url: Arc::from(base_url),
            model: Arc::from(model),
            http,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl Provider for XaiProvider {
    fn name(&self) -> &'static str {
        "xai"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Xai
    }

    fn default_model(&self) -> &'static str {
        DEFAULT_MODEL
    }

    async fn stream(
        &self,
        mut request: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, StreamError>> + Send>>, StreamError> {
        if request.model.trim().is_empty() {
            request.model = self.model.to_string();
        }
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
