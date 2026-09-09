//! OpenAI chat-completions provider.
//!
//! Targets `https://api.openai.com/v1` by default. Override with
//! `openai_base_url` in Settings (e.g. for OpenRouter or a self-hosted
//! OpenAI-compatible proxy). Wire format is OpenAI chat completions,
//! so all the heavy lifting lives in `openai_compat.rs`.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::Stream;
use reqwest::Client;

use super::openai_compat;
use super::provider::{ChatRequest, Provider, ProviderKind};
use super::stream::{StreamChunk, StreamError};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "gpt-4o";

#[derive(Clone)]
pub struct OpenAiProvider {
    pub api_key: Arc<str>,
    pub base_url: Arc<str>,
    pub model: Arc<str>,
    pub http: Client,
}

impl OpenAiProvider {
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
impl Provider for OpenAiProvider {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Openai
    }

    fn default_model(&self) -> &'static str {
        DEFAULT_MODEL
    }

    async fn stream(
        &self,
        mut request: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, StreamError>> + Send>>, StreamError> {
        // If the caller left the model empty (e.g. a bot that didn't
        // pick one), use this provider's pinned default.
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
