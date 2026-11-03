use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde_json::{json, Value};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Router {
    http: reqwest::Client,
    open_until: Arc<Mutex<HashMap<String, Instant>>>,
    recent_429: Arc<Mutex<Vec<String>>>,
}

pub struct Completion {
    pub text: String,
    pub provider: String,
}

impl Router {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            open_until: Arc::new(Mutex::new(HashMap::new())),
            recent_429: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn recent_429(&self) -> Vec<String> {
        self.recent_429.lock().await.clone()
    }

    pub fn providers() -> Vec<serde_json::Value> {
        [
            ("groq", "GROQ_API_KEY"),
            ("gemini", "GEMINI_API_KEY"),
            ("openrouter", "OPENROUTER_API_KEY"),
            ("mistral", "MISTRAL_API_KEY"),
            ("zai", "ZAI_API_KEY"),
            ("cloudflare", "CLOUDFLARE_API_TOKEN"),
            ("sambanova", "SAMBANOVA_API_KEY"),
            ("cohere", "COHERE_API_KEY"),
        ]
        .into_iter()
        .map(|(name, key)| json!({"name": name, "set": secret(key).is_ok()}))
        .collect()
    }

    pub async fn complete(&self, system: &str, user: &str) -> anyhow::Result<Completion> {
        let chain = std::env::var("JARVIS_LLM_CHAIN").unwrap_or_else(|_| {
            "groq,gemini,openrouter,mistral,zai,cloudflare,sambanova,cohere".into()
        });
        let mut last = String::from("brak providera");
        for name in chain.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(until) = self.open_until.lock().await.get(name).copied() {
                if Instant::now() < until {
                    last = format!("{name}: przerwa po 429");
                    continue;
                }
            }
            match self.one(name, system, user).await {
                Ok(text) => {
                    return Ok(Completion {
                        text,
                        provider: name.to_string(),
                    })
                }
                Err(err) => {
                    let msg = err.to_string();
                    tracing::warn!(provider = name, error = %msg, "provider odrzucony");
                    if msg.contains("429") {
                        self.open_until
                            .lock()
                            .await
                            .insert(name.to_string(), Instant::now() + Duration::from_secs(60));
                        let mut recent = self.recent_429.lock().await;
                        recent.push(format!("{name} 429"));
                        if recent.len() > 12 {
                            recent.remove(0);
                        }
                    }
                    last = format!("{name}: {msg}");
                }
            }
        }
        anyhow::bail!(last)
    }

    pub async fn complete_vision(&self, prompt: &str, image_b64: &str, mime: &str) -> anyhow::Result<String> {
        let key = secret("GEMINI_API_KEY")?;
        let model = std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".into());
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={key}"
        );
        let body = json!({
            "contents": [{
                "parts": [
                    {"text": prompt},
                    {"inline_data": {"mime_type": mime, "data": image_b64}}
                ]
            }]
        });
        let response = self.http.post(url).json(&body).send().await?;
        let status = response.status();
        let value: Value = response.json().await?;
        if status.as_u16() == 429 {
            anyhow::bail!("429");
        }
        if !status.is_success() {
            anyhow::bail!(value.to_string());
        }
        Ok(value["candidates"][0]["content"]["parts"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    async fn one(&self, name: &str, system: &str, user: &str) -> anyhow::Result<String> {
        let (url, key, model) = match name {
            "groq" => (
                "https://api.groq.com/openai/v1/chat/completions".to_string(),
                secret("GROQ_API_KEY")?,
                std::env::var("GROQ_MODEL").unwrap_or_else(|_| "llama-3.3-70b-versatile".into()),
            ),
            "openrouter" => (
                "https://openrouter.ai/api/v1/chat/completions".to_string(),
                secret("OPENROUTER_API_KEY")?,
                std::env::var("OPENROUTER_MODEL")
                    .unwrap_or_else(|_| "openrouter/free".into()),
            ),
            "mistral" => (
                "https://api.mistral.ai/v1/chat/completions".to_string(),
                secret("MISTRAL_API_KEY")?,
                std::env::var("MISTRAL_MODEL")
                    .unwrap_or_else(|_| "mistral-small-latest".into()),
            ),
            "zai" => (
                "https://api.z.ai/api/paas/v4/chat/completions".to_string(),
                secret("ZAI_API_KEY")?,
                std::env::var("ZAI_MODEL").unwrap_or_else(|_| "glm-4.5-flash".into()),
            ),
            "sambanova" => (
                "https://api.sambanova.ai/v1/chat/completions".to_string(),
                secret("SAMBANOVA_API_KEY")?,
                std::env::var("SAMBANOVA_MODEL")
                    .unwrap_or_else(|_| "Meta-Llama-3.3-70B-Instruct".into()),
            ),
            "gemini" => return self.gemini(system, user).await,
            "cloudflare" => return self.cloudflare(system, user).await,
            "cohere" => return self.cohere(system, user).await,
            other => anyhow::bail!("nieznany provider {other}"),
        };
        let body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ]
        });
        let response = self
            .http
            .post(url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        if status.as_u16() == 429 {
            anyhow::bail!("429");
        }
        let value: Value = response.json().await.unwrap_or(json!({}));
        if !status.is_success() {
            anyhow::bail!(value.to_string());
        }
        Ok(value["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    async fn gemini(&self, system: &str, user: &str) -> anyhow::Result<String> {
        let key = secret("GEMINI_API_KEY")?;
        let model = std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-2.5-flash".into());
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions"
        );
        let body = json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ]
        });
        let response = self
            .http
            .post(url)
            .bearer_auth(key)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        if status.as_u16() == 429 {
            anyhow::bail!("429");
        }
        let value: Value = response.json().await.unwrap_or(json!({}));
        if !status.is_success() {
            anyhow::bail!(value.to_string());
        }
        Ok(value["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    async fn cloudflare(&self, system: &str, user: &str) -> anyhow::Result<String> {
        let token = secret("CLOUDFLARE_API_TOKEN")?;
        let account = secret("CLOUDFLARE_ACCOUNT_ID")?;
        let model = std::env::var("CLOUDFLARE_MODEL")
            .unwrap_or_else(|_| "@cf/meta/llama-3.1-8b-instruct".into());
        let url = format!("https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/{model}");
        let response = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(&json!({"messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user}
            ]}))
            .send()
            .await?;
        let status = response.status();
        if status.as_u16() == 429 {
            anyhow::bail!("429");
        }
        let value: Value = response.json().await.unwrap_or(json!({}));
        if !status.is_success() {
            anyhow::bail!(value.to_string());
        }
        Ok(value["result"]["response"].as_str().unwrap_or("").to_string())
    }

    async fn cohere(&self, system: &str, user: &str) -> anyhow::Result<String> {
        let key = secret("COHERE_API_KEY")?;
        let model = std::env::var("COHERE_MODEL").unwrap_or_else(|_| "command-r".into());
        let response = self
            .http
            .post("https://api.cohere.com/v2/chat")
            .bearer_auth(key)
            .json(&json!({
                "model": model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user}
                ]
            }))
            .send()
            .await?;
        let status = response.status();
        if status.as_u16() == 429 {
            anyhow::bail!("429");
        }
        let value: Value = response.json().await.unwrap_or(json!({}));
        if !status.is_success() {
            anyhow::bail!(value.to_string());
        }
        Ok(value["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string())
    }

    pub async fn search(&self, query: &str) -> anyhow::Result<String> {
        let base = std::env::var("SEARXNG_URL").unwrap_or_else(|_| "http://searxng:8080".into());
        let url = format!("{base}/search?q={}&format=json", urlencoding(query));
        let response = self.http.get(url).send().await?;
        let value: Value = response.json().await.unwrap_or(json!({}));
        let mut lines = Vec::new();
        if let Some(results) = value["results"].as_array() {
            for item in results.iter().take(5) {
                let title = item["title"].as_str().unwrap_or("");
                let url = item["url"].as_str().unwrap_or("");
                let content = item["content"].as_str().unwrap_or("");
                lines.push(format!("- {title} ({url}) {content}"));
            }
        }
        if lines.is_empty() {
            Ok("Brak wyników.".into())
        } else {
            Ok(lines.join("\n"))
        }
    }
}

fn secret(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .context("brak klucza")
}

fn urlencoding(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

