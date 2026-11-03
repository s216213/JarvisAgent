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
    catalog: Arc<Mutex<HashMap<String, (Instant, Vec<String>)>>>,
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
            catalog: Arc::new(Mutex::new(HashMap::new())),
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
        let mut detail = None;
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
                    last = format!("{name}: {}", short(&msg));
                    if !msg.contains("brak klucza") {
                        detail = Some(last.clone());
                    }
                }
            }
        }
        anyhow::bail!(detail.unwrap_or(last))
    }

    pub async fn complete_vision(&self, prompt: &str, image_b64: &str, mime: &str) -> anyhow::Result<String> {
        let key = secret("GEMINI_API_KEY")?;
        let models = self.candidates("gemini").await;
        let mut last = String::from("brak modelu");
        for model in models.into_iter().take(4) {
            let url = format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"
            );
            let response = self
                .http
                .post(url)
                .header("x-goog-api-key", &key)
                .json(&json!({
                    "contents": [{
                        "parts": [
                            {"text": prompt},
                            {"inline_data": {"mime_type": mime, "data": image_b64}}
                        ]
                    }]
                }))
                .send()
                .await?;
            let status = response.status();
            let value: Value = response.json().await.unwrap_or(json!({}));
            if status.as_u16() == 429 {
                anyhow::bail!("429");
            }
            if status.is_success() {
                self.promote("gemini", &model).await;
                return Ok(value["candidates"][0]["content"]["parts"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_string());
            }
            let msg = value.to_string();
            tracing::warn!(provider = "gemini", %model, error = %short(&msg), "model odrzucony");
            last = format!("{model}: {}", short(&msg));
            if !skippable(status.as_u16(), &msg) {
                anyhow::bail!(last);
            }
        }
        anyhow::bail!(last)
    }

    async fn one(&self, name: &str, system: &str, user: &str) -> anyhow::Result<String> {
        match name {
            "groq" | "gemini" | "openrouter" | "mistral" | "zai" | "sambanova" | "cloudflare"
            | "cohere" => self.chat(name, system, user).await,
            other => anyhow::bail!("nieznany provider {other}"),
        }
    }

    async fn chat(&self, provider: &str, system: &str, user: &str) -> anyhow::Result<String> {
        let _ = credentials(provider)?;
        let models = self.candidates(provider).await;
        if models.is_empty() {
            anyhow::bail!("brak modelu");
        }
        let mut last = String::from("brak modelu");
        let mut limited = false;
        for model in models.into_iter().take(4) {
            match self.complete_with(provider, &model, system, user).await {
                Ok(text) => {
                    self.promote(provider, &model).await;
                    return Ok(text);
                }
                Err(err) => {
                    let msg = err.to_string();
                    tracing::warn!(provider, %model, error = %short(&msg), "model odrzucony");
                    if msg.contains("429") {
                        self.demote(provider, &model).await;
                        limited = true;
                        last = format!("{model}: 429");
                        continue;
                    }
                    last = format!("{model}: {}", short(&msg));
                    if !msg.starts_with("pomiń ") {
                        anyhow::bail!(last);
                    }
                    self.demote(provider, &model).await;
                    last = msg.trim_start_matches("pomiń ").to_string();
                }
            }
        }
        if limited {
            anyhow::bail!("429");
        }
        anyhow::bail!(last)
    }

    async fn complete_with(
        &self,
        provider: &str,
        model: &str,
        system: &str,
        user: &str,
    ) -> anyhow::Result<String> {
        let (status, value) = match provider {
            "cloudflare" => {
                let (token, account) = credentials(provider)?;
                let url = format!(
                    "https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/{model}"
                );
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
                (response.status(), response.json().await.unwrap_or(json!({})))
            }
            "cohere" => {
                let (key, _) = credentials(provider)?;
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
                (response.status(), response.json().await.unwrap_or(json!({})))
            }
            _ => {
                let (key, _) = credentials(provider)?;
                let response = self
                    .http
                    .post(chat_url(provider)?)
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
                (response.status(), response.json().await.unwrap_or(json!({})))
            }
        };
        if status.as_u16() == 429 {
            anyhow::bail!("429");
        }
        if !status.is_success() {
            let body = value.to_string();
            if skippable(status.as_u16(), &body) {
                anyhow::bail!("pomiń {model}: {}", short(&body));
            }
            anyhow::bail!(short(&body));
        }
        let text = match provider {
            "cloudflare" => value["result"]["response"].as_str().unwrap_or(""),
            "cohere" => value["message"]["content"][0]["text"].as_str().unwrap_or(""),
            _ => value["choices"][0]["message"]["content"].as_str().unwrap_or(""),
        };
        Ok(text.to_string())
    }

    async fn candidates(&self, provider: &str) -> Vec<String> {
        let mut models = Vec::new();
        if let Some(pinned) = optional_env(&format!("{}_MODEL", model_prefix(provider))) {
            models.push(pinned);
        }
        for model in self.discovered(provider).await {
            if !models.iter().any(|have| have == &model) {
                models.push(model);
            }
            if models.len() == 4 {
                break;
            }
        }
        if models.is_empty() {
            fallback(provider)
        } else {
            models
        }
    }

    async fn discovered(&self, provider: &str) -> Vec<String> {
        {
            let cache = self.catalog.lock().await;
            if let Some((at, models)) = cache.get(provider) {
                if at.elapsed() < Duration::from_secs(15 * 60) && !models.is_empty() {
                    return models.clone();
                }
            }
        }
        match self.fetch_catalog(provider).await {
            Ok(models) if !models.is_empty() => {
                self.catalog
                    .lock()
                    .await
                    .insert(provider.to_string(), (Instant::now(), models.clone()));
                models
            }
            other => {
                if let Err(err) = &other {
                    tracing::warn!(provider, error = %short(&err.to_string()), "katalog modeli niedostępny");
                }
                self.catalog
                    .lock()
                    .await
                    .get(provider)
                    .map(|(_, models)| models.clone())
                    .filter(|models| !models.is_empty())
                    .unwrap_or_else(|| fallback(provider))
            }
        }
    }

    async fn fetch_catalog(&self, provider: &str) -> anyhow::Result<Vec<String>> {
        let (key, account) = credentials(provider)?;
        let request = if provider == "gemini" {
            self.http
                .get("https://generativelanguage.googleapis.com/v1beta/models")
                .header("x-goog-api-key", key)
                .query(&[("pageSize", "100")])
        } else if provider == "cloudflare" {
            self.http
                .get(format!(
                    "https://api.cloudflare.com/client/v4/accounts/{account}/ai/models/search"
                ))
                .bearer_auth(key)
                .query(&[("per_page", "50")])
        } else {
            self.http.get(catalog_url(provider)?).bearer_auth(key)
        };
        let response = request.send().await?;
        let status = response.status();
        let value: Value = response.json().await.unwrap_or(json!({}));
        if !status.is_success() {
            anyhow::bail!(short(&value.to_string()));
        }
        let models = rank_models(&value);
        if models.is_empty() {
            anyhow::bail!("pusty katalog");
        }
        Ok(models)
    }

    async fn promote(&self, provider: &str, model: &str) {
        self.move_model(provider, model, true).await;
    }

    async fn demote(&self, provider: &str, model: &str) {
        self.move_model(provider, model, false).await;
    }

    async fn move_model(&self, provider: &str, model: &str, front: bool) {
        let mut cache = self.catalog.lock().await;
        if let Some((_, models)) = cache.get_mut(provider) {
            if let Some(pos) = models.iter().position(|item| item == model) {
                let chosen = models.remove(pos);
                if front {
                    models.insert(0, chosen);
                } else {
                    models.push(chosen);
                }
            }
        }
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

fn optional_env(name: &str) -> Option<String> {
    secret(name).ok()
}

fn credentials(provider: &str) -> anyhow::Result<(String, String)> {
    let key = secret(match provider {
        "groq" => "GROQ_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        "openrouter" => "OPENROUTER_API_KEY",
        "mistral" => "MISTRAL_API_KEY",
        "zai" => "ZAI_API_KEY",
        "sambanova" => "SAMBANOVA_API_KEY",
        "cloudflare" => "CLOUDFLARE_API_TOKEN",
        "cohere" => "COHERE_API_KEY",
        other => anyhow::bail!("nieznany provider {other}"),
    })?;
    let account = if provider == "cloudflare" {
        secret("CLOUDFLARE_ACCOUNT_ID")?
    } else {
        String::new()
    };
    Ok((key, account))
}

fn model_prefix(provider: &str) -> &'static str {
    match provider {
        "groq" => "GROQ",
        "gemini" => "GEMINI",
        "openrouter" => "OPENROUTER",
        "mistral" => "MISTRAL",
        "zai" => "ZAI",
        "sambanova" => "SAMBANOVA",
        "cloudflare" => "CLOUDFLARE",
        "cohere" => "COHERE",
        _ => "JARVIS",
    }
}

fn chat_url(provider: &str) -> anyhow::Result<&'static str> {
    Ok(match provider {
        "groq" => "https://api.groq.com/openai/v1/chat/completions",
        "gemini" => "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
        "openrouter" => "https://openrouter.ai/api/v1/chat/completions",
        "mistral" => "https://api.mistral.ai/v1/chat/completions",
        "zai" => "https://api.z.ai/api/paas/v4/chat/completions",
        "sambanova" => "https://api.sambanova.ai/v1/chat/completions",
        other => anyhow::bail!("brak czatu {other}"),
    })
}

fn catalog_url(provider: &str) -> anyhow::Result<&'static str> {
    Ok(match provider {
        "groq" => "https://api.groq.com/openai/v1/models",
        "openrouter" => "https://openrouter.ai/api/v1/models",
        "mistral" => "https://api.mistral.ai/v1/models",
        "zai" => "https://api.z.ai/api/paas/v4/models",
        "sambanova" => "https://api.sambanova.ai/v1/models",
        "cohere" => "https://api.cohere.com/v1/models?endpoint=chat",
        other => anyhow::bail!("brak katalogu {other}"),
    })
}

fn fallback(provider: &str) -> Vec<String> {
    match provider {
        "gemini" => vec!["gemini-3.8-flash".into(), "gemini-2.0-flash".into()],
        "groq" => vec!["llama-3.3-70b-versatile".into()],
        "openrouter" => vec!["openrouter/free".into()],
        "mistral" => vec!["mistral-small-latest".into()],
        "zai" => vec!["glm-4.5-flash".into()],
        "sambanova" => vec!["Meta-Llama-3.3-70B-Instruct".into()],
        "cloudflare" => vec![
            "@cf/meta/llama-3.3-70b-instruct".into(),
            "@cf/meta/llama-3.1-8b-instruct".into(),
        ],
        "cohere" => vec!["command-a-03-2025".into(), "command-r".into()],
        _ => Vec::new(),
    }
}

struct Listed {
    id: String,
    created: i64,
}

fn rank_models(value: &Value) -> Vec<String> {
    let Some(items) = value["data"]
        .as_array()
        .or_else(|| value["models"].as_array())
        .or_else(|| value["result"].as_array())
    else {
        return Vec::new();
    };
    let mut listed = Vec::new();
    for item in items {
        let Some(id) = model_id(item) else {
            continue;
        };
        if !is_chat_model(item, &id) {
            continue;
        }
        listed.push(Listed {
            id,
            created: item["created"].as_i64().unwrap_or(0),
        });
    }
    listed.sort_by(|a, b| {
        b.created
            .cmp(&a.created)
            .then_with(|| family(&b.id).cmp(&family(&a.id)))
            .then_with(|| version_parts(&b.id).cmp(&version_parts(&a.id)))
            .then_with(|| flavor(&b.id).cmp(&flavor(&a.id)))
            .then_with(|| a.id.cmp(&b.id))
    });
    listed.into_iter().map(|item| item.id).collect()
}

fn model_id(item: &Value) -> Option<String> {
    let raw = item["id"].as_str().or_else(|| item["name"].as_str())?;
    let id = raw.trim().trim_start_matches("models/").to_string();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

fn is_chat_model(item: &Value, id: &str) -> bool {
    if let Some(methods) = item["supportedGenerationMethods"].as_array() {
        let generates = methods.iter().any(|method| method.as_str() == Some("generateContent"));
        if !generates {
            return false;
        }
    }
    if let Some(endpoints) = item["endpoints"].as_array() {
        let chat = endpoints.iter().any(|endpoint| endpoint.as_str() == Some("chat"));
        if !chat {
            return false;
        }
    }
    let task = item["task"]["name"]
        .as_str()
        .or_else(|| item["task"].as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !task.is_empty() && !task.contains("text") && !task.contains("chat") {
        return false;
    }
    let id = id.to_ascii_lowercase();
    const SKIP: &[&str] = &[
        "embed", "whisper", "tts", "transcrib", "moderation", "imagen", "veo", "aqa", "rerank",
        "guard", "clip", "audio", "speech", "lyria", "native-audio", "-image", "image-", "live",
        "deep-research", "antigravity", "robotics", "computer-use", "customtools", "nano-banana",
    ];
    !SKIP.iter().any(|part| id.contains(part))
}

fn version_parts(id: &str) -> Vec<u32> {
    let mut parts = Vec::new();
    let mut current = None;
    for byte in id.bytes() {
        if byte.is_ascii_digit() {
            let digit = u32::from(byte - b'0');
            current = Some(current.unwrap_or(0u32).saturating_mul(10).saturating_add(digit));
        } else if let Some(number) = current.take() {
            if number < 1900 {
                parts.push(number);
            }
        }
    }
    if let Some(number) = current.take() {
        if number < 1900 {
            parts.push(number);
        }
    }
    parts
}

fn family(id: &str) -> u8 {
    let id = id.to_ascii_lowercase();
    if id.starts_with("gemini") || id.starts_with("gemma") || id.contains("/gemini") || id.contains("/gemma")
    {
        1
    } else {
        0
    }
}

fn flavor(id: &str) -> u8 {
    let id = id.to_ascii_lowercase();
    if id.contains("flash") || id.contains("lite") {
        2
    } else if id.contains("pro") {
        1
    } else {
        0
    }
}

fn skippable(status: u16, body: &str) -> bool {
    let about_model = body.to_ascii_lowercase().contains("model")
        || body.contains("NOT_FOUND")
        || body.contains("no longer available")
        || body.contains("does not exist");
    matches!(status, 400 | 402 | 404 | 408 | 409 | 410 | 500 | 502 | 503 | 529)
        || (status == 400 && about_model)
        || body.contains("UNAVAILABLE")
        || body.contains("model_not_found")
}

fn short(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(240).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_chat_model_comes_first() {
        let value = json!({
            "models": [
                {"name": "models/gemini-2.5-flash", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-embedding-001", "supportedGenerationMethods": ["embedContent"]},
                {"name": "models/gemini-3.8-pro", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-3.8-flash", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-2.0-flash", "supportedGenerationMethods": ["generateContent"]}
            ]
        });
        let ranked = rank_models(&value);
        assert_eq!(ranked[0], "gemini-3.8-flash");
        assert_eq!(ranked[1], "gemini-3.8-pro");
        assert!(!ranked.iter().any(|id| id.contains("embed")));
    }

    #[test]
    fn research_preview_does_not_outrank_gemini() {
        let value = json!({
            "models": [
                {"name": "models/deep-research-pro-preview-12-2025", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-3.8-flash", "supportedGenerationMethods": ["generateContent"]},
                {"name": "models/gemini-4-argon", "supportedGenerationMethods": ["generateContent"]}
            ]
        });
        let ranked = rank_models(&value);
        assert_eq!(ranked[0], "gemini-4-argon");
        assert_eq!(ranked[1], "gemini-3.8-flash");
        assert!(!ranked.iter().any(|id| id.contains("research")));
    }

    #[test]
    fn created_timestamp_beats_an_older_name() {
        let value = json!({
            "data": [
                {"id": "llama-3.1-8b", "created": 10},
                {"id": "llama-3.3-70b-versatile", "created": 20},
                {"id": "whisper-large", "created": 30}
            ]
        });
        let ranked = rank_models(&value);
        assert_eq!(ranked, vec!["llama-3.3-70b-versatile", "llama-3.1-8b"]);
    }
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

