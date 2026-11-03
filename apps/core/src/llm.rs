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
        let mut errors = Vec::new();
        for name in chain.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(until) = self.open_until.lock().await.get(name).copied() {
                if Instant::now() < until {
                    errors.push(format!("{name}: przerwa po 429"));
                    continue;
                }
            }
            match self.one(name, system, user).await {
                Ok(text) => {
                    tracing::info!(provider = name, "model odpowiedział");
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
                    errors.push(format!("{name}: {}", short(&msg)));
                }
            }
        }
        let report = if errors.is_empty() {
            "brak providera".to_string()
        } else {
            errors.join(" | ")
        };
        anyhow::bail!(report)
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
        for model in models {
            match self.complete_with(provider, &model, system, user).await {
                Ok(text) => {
                    self.promote(provider, &model).await;
                    return Ok(text);
                }
                Err(err) => {
                    let msg = err.to_string();
                    tracing::warn!(provider, %model, error = %short(&msg), "model odrzucony");
                    if msg.starts_with("401") {
                        anyhow::bail!(msg);
                    }
                    if msg.contains("429") {
                        self.demote(provider, &model).await;
                        limited = true;
                        last = format!("{model}: 429");
                        continue;
                    }
                    self.demote(provider, &model).await;
                    last = if let Some(rest) = msg.strip_prefix("pomiń ") {
                        rest.to_string()
                    } else {
                        format!("{model}: {}", short(&msg))
                    };
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
        let messages = json!([
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ]);
        let (status, value) = match provider {
            "cloudflare" => {
                let (token, account) = credentials(provider)?;
                let url = format!(
                    "https://api.cloudflare.com/client/v4/accounts/{account}/ai/run/{model}"
                );
                self.post_bearer(&url, &token, &json!({"messages": messages}))
                    .await
                    .map_err(|err| timeout_as_skip(model, err))?
            }
            "cohere" => {
                let (key, _) = credentials(provider)?;
                self.post_bearer(
                    "https://api.cohere.com/v2/chat",
                    &key,
                    &json!({"model": model, "messages": messages}),
                )
                .await
                .map_err(|err| timeout_as_skip(model, err))?
            }
            _ => {
                let (key, _) = credentials(provider)?;
                let url = chat_url(provider)?;
                let mut body = json!({"model": model, "messages": messages});
                if wants_low_reasoning(provider, model) {
                    body["reasoning_effort"] = json!("low");
                }
                if provider == "groq" && model.contains("gpt-oss") {
                    body["max_tokens"] = json!(2048);
                }
                let first = self
                    .post_bearer(url, &key, &body)
                    .await
                    .map_err(|err| timeout_as_skip(model, err))?;
                if first.0.as_u16() == 400 && body.get("reasoning_effort").is_some() {
                    if let Some(object) = body.as_object_mut() {
                        object.remove("reasoning_effort");
                    }
                    self.post_bearer(url, &key, &body)
                        .await
                        .map_err(|err| timeout_as_skip(model, err))?
                } else {
                    first
                }
            }
        };
        if status.as_u16() == 429 {
            anyhow::bail!("429");
        }
        if !status.is_success() {
            let body = value.to_string();
            if auth_failure(status.as_u16(), &body) {
                anyhow::bail!("401 {}", short(&body));
            }
            if skippable(status.as_u16(), &body) {
                anyhow::bail!("pomiń {model}: {}", short(&body));
            }
            anyhow::bail!(short(&body));
        }
        let text = message_text(provider, &value);
        if text.trim().is_empty() {
            anyhow::bail!("pomiń {model}: pusta odpowiedź");
        }
        Ok(text)
    }

    async fn post_bearer(&self, url: &str, key: &str, body: &Value) -> anyhow::Result<(reqwest::StatusCode, Value)> {
        let response = self
            .http
            .post(url)
            .bearer_auth(key)
            .timeout(Duration::from_secs(25))
            .json(body)
            .send()
            .await?;
        let status = response.status();
        let value = response.json().await.unwrap_or(json!({}));
        Ok((status, value))
    }

    async fn candidates(&self, provider: &str) -> Vec<String> {
        let mut models = Vec::new();
        if let Some(pinned) = optional_env(&format!("{}_MODEL", model_prefix(provider))) {
            push_model(&mut models, pinned);
        }
        for model in preferred(provider) {
            push_model(&mut models, (*model).to_string());
        }
        for model in self.discovered(provider).await {
            push_model(&mut models, model);
            if models.len() == 6 {
                break;
            }
        }
        for model in fallback(provider) {
            if models.len() == 6 {
                break;
            }
            push_model(&mut models, model);
        }
        models
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
                let models = order_models(provider, models);
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
        let response = request.timeout(Duration::from_secs(8)).send().await?;
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
        "gemini" => vec![
            "gemini-3.5-flash-lite".into(),
            "gemini-3.8-flash".into(),
            "gemini-flash-latest".into(),
        ],
        "groq" => vec![
            "openai/gpt-oss-20b".into(),
            "openai/gpt-oss-120b".into(),
            "qwen/qwen3.8-27b".into(),
        ],
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
        if !is_chat_model(item, &id) || !is_free_tier(item, &id) || retired(&id) {
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
        "orpheus", "preview", "safeguard",
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

fn is_free_tier(item: &Value, id: &str) -> bool {
    if let Some(pricing) = item.get("pricing") {
        return is_zero(&pricing["prompt"]) && is_zero(&pricing["completion"]);
    }
    let id = id.to_ascii_lowercase();
    if id.ends_with(":free") || id.contains("/free") || id.contains("-free") {
        return true;
    }
    const PAID: &[&str] = &["-pro", "pro-", "/pro", "ultra", "opus", "argon", "-plus"];
    !PAID.iter().any(|part| id.contains(part)) && !id.ends_with("pro")
}

fn is_zero(value: &Value) -> bool {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .unwrap_or(1.0)
        == 0.0
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
    matches!(status, 400 | 402 | 403 | 404 | 408 | 409 | 410 | 422 | 500 | 502 | 503 | 529)
        || (status == 400 && about_model)
        || body.contains("UNAVAILABLE")
        || body.contains("model_not_found")
}

fn push_model(models: &mut Vec<String>, model: String) {
    if !model.is_empty() && !models.iter().any(|have| have == &model) {
        models.push(model);
    }
}

fn timeout_as_skip(model: &str, err: anyhow::Error) -> anyhow::Error {
    let timed = err
        .downcast_ref::<reqwest::Error>()
        .is_some_and(|error| error.is_timeout())
        || err.to_string().to_ascii_lowercase().contains("timed out");
    if timed {
        anyhow::anyhow!("pomiń {model}: timeout")
    } else {
        err
    }
}

fn wants_low_reasoning(provider: &str, model: &str) -> bool {
    provider == "gemini" || (provider == "groq" && model.contains("gpt-oss"))
}

fn auth_failure(status: u16, body: &str) -> bool {
    if status == 401 {
        return true;
    }
    let body = body.to_ascii_lowercase();
    body.contains("api key not valid")
        || body.contains("invalid api key")
        || body.contains("api_key_invalid")
        || body.contains("invalid_api_key")
}

fn message_text(provider: &str, value: &Value) -> String {
    match provider {
        "cloudflare" => value["result"]["response"].as_str().unwrap_or("").to_string(),
        "cohere" => value["message"]["content"][0]["text"].as_str().unwrap_or("").to_string(),
        _ => openai_content(&value["choices"][0]["message"]),
    }
}

fn openai_content(message: &Value) -> String {
    let content = &message["content"];
    if let Some(text) = content.as_str() {
        if !text.trim().is_empty() {
            return text.to_string();
        }
    }
    if let Some(parts) = content.as_array() {
        let mut out = String::new();
        for part in parts {
            if part["type"].as_str() == Some("reasoning") || part["thought"].as_bool() == Some(true) {
                continue;
            }
            let Some(text) = part["text"].as_str().or_else(|| part["content"].as_str()) else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
        if !out.trim().is_empty() {
            return out;
        }
    }
    message["reasoning"]
        .as_str()
        .or_else(|| message["reasoning_content"].as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn preferred(provider: &str) -> &'static [&'static str] {
    match provider {
        "groq" => &["openai/gpt-oss-20b", "openai/gpt-oss-120b", "qwen/qwen3.8-27b"],
        "gemini" => &["gemini-3.5-flash-lite", "gemini-3.8-flash", "gemini-flash-latest"],
        _ => &[],
    }
}

fn retired(id: &str) -> bool {
    matches!(
        id,
        "llama-3.3-70b-versatile"
            | "llama-3.1-8b-instant"
            | "llama3-70b-8192"
            | "llama3-8b-8192"
            | "llama-3.3-70b-specdec"
    )
}

fn order_models(provider: &str, models: Vec<String>) -> Vec<String> {
    let pref = preferred(provider);
    let mut models: Vec<String> = models.into_iter().filter(|id| !retired(id)).collect();
    models.sort_by_key(|id| pref.iter().position(|item| item == id).unwrap_or(10_000));
    models
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
        assert_eq!(ranked[1], "gemini-2.5-flash");
        assert!(!ranked.iter().any(|id| id.contains("pro") || id.contains("embed")));
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
        assert_eq!(ranked, vec!["gemini-3.8-flash"]);
    }

    #[test]
    fn paid_catalog_entries_are_left_out() {
        let value = json!({
            "data": [
                {"id": "vendor/big-pro", "pricing": {"prompt": "0.0002", "completion": "0.0008"}},
                {"id": "vendor/flash:free", "pricing": {"prompt": "0", "completion": "0"}}
            ]
        });
        assert_eq!(rank_models(&value), vec!["vendor/flash:free"]);
    }

    #[test]
    fn created_timestamp_beats_an_older_name() {
        let value = json!({
            "data": [
                {"id": "openai/gpt-oss-20b", "created": 10},
                {"id": "openai/gpt-oss-120b", "created": 20},
                {"id": "whisper-large", "created": 30}
            ]
        });
        let ranked = rank_models(&value);
        assert_eq!(ranked, vec!["openai/gpt-oss-120b", "openai/gpt-oss-20b"]);
    }

    #[test]
    fn groq_catalog_leads_with_a_live_chat_model() {
        let value = json!({
            "data": [
                {"id": "llama-3.3-70b-versatile", "created": 100},
                {"id": "canopylabs/orpheus-v1-english", "created": 400},
                {"id": "openai/gpt-oss-120b", "created": 200},
                {"id": "openai/gpt-oss-20b", "created": 150},
                {"id": "whisper-large-v3", "created": 500}
            ]
        });
        let ranked = order_models("groq", rank_models(&value));
        assert_eq!(ranked, vec!["openai/gpt-oss-20b", "openai/gpt-oss-120b"]);
        let spare = fallback("groq");
        assert!(spare.contains(&"openai/gpt-oss-20b".into()));
        assert!(!spare.iter().any(|id| id.contains("llama-3.3")));
    }

    #[test]
    fn gemini_lite_is_tried_before_flash() {
        let models = order_models(
            "gemini",
            vec!["gemini-3.8-flash".into(), "gemini-3.5-flash-lite".into()],
        );
        assert_eq!(models[0], "gemini-3.5-flash-lite");
    }

    #[test]
    fn thought_parts_do_not_hide_the_answer() {
        let value = json!({
            "choices": [{
                "message": {
                    "content": [
                        {"type": "reasoning", "text": "najpierw myślę"},
                        {"type": "text", "text": "odpowiedź"}
                    ]
                }
            }]
        });
        assert_eq!(message_text("groq", &value), "odpowiedź");
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

