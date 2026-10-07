//! Non-streaming Chat Completions + function tool calls; no execution tools.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key_env: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets_file: Option<std::path::PathBuf>,
    pub timeout_secs: u64,
    pub max_model_calls: usize,
    pub max_tool_rounds: usize,
    pub max_tool_calls: usize,
    pub max_tokens: u32,
    pub thinking: bool,
    pub allow_insecure_http: bool,
}
impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: "off".into(),
            base_url: "https://api.deepseek.com".into(),
            model: String::new(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            secrets_file: None,
            timeout_secs: 30,
            max_model_calls: 4,
            max_tool_rounds: 3,
            max_tool_calls: 12,
            max_tokens: 2048,
            thinking: false,
            allow_insecure_http: false,
        }
    }
}
impl ModelConfig {
    pub fn parse(value: &Value) -> Result<Self> {
        let mut value = value.clone();
        if value["provider"] == "ollama" {
            let fields = value
                .as_object_mut()
                .context("invalid model configuration")?;
            fields
                .entry("base_url")
                .or_insert(json!("http://127.0.0.1:11434/v1"));
            fields.entry("api_key_env").or_insert(json!(""));
            fields.entry("timeout_secs").or_insert(json!(120));
        }
        let config: Self = if value.is_null() {
            Self::default()
        } else {
            serde_json::from_value(value.clone()).context("invalid model configuration")?
        };
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(
                self.provider.as_str(),
                "off" | "deepseek" | "compatible" | "ollama"
            ),
            "unknown model provider"
        );
        ensure!(
            (1..=300).contains(&self.timeout_secs)
                && (1..=16).contains(&self.max_model_calls)
                && (1..=12).contains(&self.max_tool_rounds)
                && (1..=64).contains(&self.max_tool_calls)
                && (1..=8192).contains(&self.max_tokens),
            "model limits out of range"
        );
        if self.provider != "off" {
            crate::team::validate_url(&self.base_url, self.allow_insecure_http)?;
            ensure!(
                !self.model.is_empty() && self.model.len() <= 128,
                "model name is required"
            );
        }
        if !self.api_key_env.is_empty() {
            crate::team::validate_env_name(&self.api_key_env)?;
        }
        ensure!(
            self.provider != "deepseek" || !self.api_key_env.is_empty(),
            "DeepSeek requires an API key environment variable"
        );
        Ok(())
    }
}
pub trait LanguageModel {
    fn complete(
        &self,
        messages: &[Value],
        tools: &[Value],
    ) -> impl std::future::Future<Output = Result<Value>>;
}
pub struct HttpModel {
    config: ModelConfig,
    client: reqwest::Client,
    key: Option<String>,
}
impl HttpModel {
    /// Explicit readiness probe: verifies calling a function and consuming its response.
    /// It has no file/command tools and never dispatches Codex.
    pub async fn probe_tools(&self) -> Result<()> {
        let nonce = uuid::Uuid::new_v4().to_string();
        let tools = vec![
            json!({"type":"function","function":{"name":"connection_probe",
            "description":"Check a local tool roundtrip. Call with the requested nonce.",
            "parameters":{"type":"object","properties":{"nonce":{"type":"string"}},"required":["nonce"],"additionalProperties":false}}}),
        ];
        let mut messages = vec![
            json!({"role":"user","content":format!("Call connection_probe exactly once with nonce {nonce}. After its tool response, answer with exactly the returned receipt; do not call another tool.")}),
        ];
        let response = self.complete(&messages, &tools).await?;
        let calls = response["tool_calls"]
            .as_array()
            .context("model_tool_probe_no_call; select a model supporting tools")?;
        ensure!(calls.len() == 1, "model_tool_probe_invalid_calls");
        let call = &calls[0];
        let id = call["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .context("model_tool_probe_missing_id")?;
        ensure!(
            call["type"] == "function" && call["function"]["name"] == "connection_probe",
            "model_tool_probe_wrong_tool"
        );
        let args: Value = serde_json::from_str(
            call["function"]["arguments"]
                .as_str()
                .context("model_tool_probe_invalid_arguments")?,
        )?;
        ensure!(
            args == json!({"nonce":nonce}),
            "model_tool_probe_wrong_arguments"
        );
        let receipt = uuid::Uuid::new_v4().to_string();
        messages.push(response.clone());
        messages.push(json!({"role":"tool","tool_call_id":id,"content":json!({"receipt":receipt}).to_string()}));
        let final_response = self.complete(&messages, &tools).await?;
        ensure!(
            final_response
                .get("tool_calls")
                .is_none_or(|v| v.is_null() || v.as_array().is_some_and(|a| a.is_empty()))
                && final_response["content"]
                    .as_str()
                    .is_some_and(|s| s.trim() == receipt),
            "model_tool_probe_did_not_consume_result"
        );
        Ok(())
    }

    pub fn new(config: ModelConfig) -> Result<Self> {
        config.validate()?;
        ensure!(config.provider != "off", "model_disabled");
        let key = if let Some(path) = &config.secrets_file {
            Some(crate::secrets::read_field(path, &config.api_key_env)?)
        } else if config.api_key_env.is_empty() {
            None
        } else {
            Some(
                std::env::var(&config.api_key_env)
                    .ok()
                    .filter(|k| !k.is_empty())
                    .context("missing_model_key")?,
            )
        };
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            config,
            client,
            key,
        })
    }
}
impl LanguageModel for HttpModel {
    async fn complete(&self, messages: &[Value], tools: &[Value]) -> Result<Value> {
        self.complete_with_requirement(messages, tools, false).await
    }
}
impl HttpModel {
    /// Owner chat must persist an answer/effect before the turn may finish.
    pub async fn complete_with_requirement(
        &self,
        messages: &[Value],
        tools: &[Value],
        require_tool: bool,
    ) -> Result<Value> {
        let mut body = json!({"model":self.config.model,"messages":messages,"tools":tools,"stream":false,"max_tokens":self.config.max_tokens});
        if require_tool {
            body["tool_choice"] = json!("required");
        }
        if self.config.provider == "deepseek" {
            body["thinking"] = json!({"type":if self.config.thinking{"enabled"}else{"disabled"}});
        }
        let mut req = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.config.base_url.trim_end_matches('/')
            ))
            .json(&body);
        if let Some(key) = &self.key {
            req = req.bearer_auth(key);
        }
        let response = req.send().await.map_err(|e| {
            anyhow::anyhow!(if e.is_timeout() {
                "model_timeout"
            } else {
                "model_unavailable"
            })
        })?;
        let status = response.status().as_u16();
        ensure!(
            (200..300).contains(&status),
            "{}",
            match status {
                401 | 403 => "model_auth_failed",
                402 | 429 => "model_quota_limited",
                _ => "model_http_failed",
            }
        );
        let value = crate::team::response_json(response)
            .await
            .map_err(|_| anyhow::anyhow!("model_invalid_response"))?;
        if let Some(key) = &self.key {
            ensure!(
                !value.to_string().contains(key),
                "model_sensitive_response_rejected"
            );
        }
        let choice = &value["choices"][0];
        let message = &choice["message"];
        ensure!(
            message["role"] == "assistant"
                && matches!(
                    choice["finish_reason"].as_str(),
                    Some("stop" | "tool_calls")
                ),
            "model_invalid_response"
        );
        ensure!(
            message["content"].is_null() || message["content"].is_string(),
            "model_invalid_response"
        );
        let mut kept = json!({"role":"assistant","content":message["content"]});
        for field in ["reasoning_content", "tool_calls"] {
            if let Some(v) = message.get(field) {
                kept[field] = v.clone();
            }
        }
        // Preserve reasoning_content for every subsequent tool round, including
        // thinking-mode responses. Never put API keys or HTTP error bodies in logs.
        Ok(kept)
    }
}
