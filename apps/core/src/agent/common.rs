use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::process::Command;
use turso::params;
use uuid::Uuid;

use crate::state::App;

pub(crate) async fn confirm(app: &App, summary: &str, body: &str) -> anyhow::Result<bool> {
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.confirms.lock().await.insert(id.clone(), tx);
    app.send_presence(&json!({
        "type": "confirm_request",
        "id": id,
        "summary": summary,
        "body": body
    }))
    .await;
    say(app, summary).await;
    match tokio::time::timeout(Duration::from_secs(90), rx).await {
        Ok(Ok(true)) => Ok(true),
        _ => Ok(false),
    }
}

pub(crate) async fn finish(mut command: Command) -> String {
    let finished = match tokio::time::timeout(Duration::from_secs(30), command.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => return err.to_string(),
        Err(_) => return "Przekroczony czas.".into(),
    };
    let mut text = String::from_utf8_lossy(&finished.stdout).to_string();
    let err = String::from_utf8_lossy(&finished.stderr);
    if !err.trim().is_empty() {
        text.push('\n');
        text.push_str(&err);
    }
    text.push_str(&format!("\nkod {}", finished.status.code().unwrap_or(-1)));
    if text.len() > 4000 {
        text.truncate(4000);
    }
    text
}

pub(crate) async fn ask_device(app: &App, kind: &str, message: &Value) -> String {
    let id = Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.results.lock().await.insert(id.clone(), tx);
    let mut payload = message.clone();
    payload["id"] = json!(id);
    let clients = app.clients.lock().await;
    let target = clients.keys().find(|key| key.contains(kind)).cloned();
    drop(clients);
    if let Some(device) = target {
        app.send(&device, &payload).await;
    } else {
        return "Urządzenie nie jest podłączone.".into();
    }
    match tokio::time::timeout(Duration::from_secs(30), rx).await {
        Ok(Ok(text)) => text,
        _ => "Brak wyniku z urządzenia.".into(),
    }
}

pub(crate) async fn say(app: &App, text: &str) {
    for part in split_clauses(text) {
        app.send_presence(&json!({"type":"token","text": part, "final": false}))
            .await;
    }
}

fn split_clauses(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in text.chars() {
        buf.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') || buf.chars().count() > 80 {
            let piece = buf.trim().to_string();
            if !piece.is_empty() {
                out.push(piece);
            }
            buf.clear();
        }
    }
    if !buf.trim().is_empty() {
        out.push(buf.trim().to_string());
    }
    out
}

pub(crate) async fn store_assistant(app: &App, text: &str) -> anyhow::Result<()> {
    let conn = app.db.connect()?;
    conn.execute(
        "INSERT INTO messages (role, body, created) VALUES ('assistant', ?1, ?2)",
        params![seal_store(app, text).as_str(), chrono::Utc::now().timestamp()],
    )
    .await?;
    Ok(())
}

pub fn vault_path() -> PathBuf {
    PathBuf::from(std::env::var("JARVIS_VAULT").unwrap_or_else(|_| "data/vault".into()))
}

pub(crate) fn seal_store(app: &App, text: &str) -> String {
    match app.identity.seal(&app.identity.public, text.as_bytes()) {
        Ok(bytes) => format!(
            "seal:{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
        ),
        Err(_) => text.to_string(),
    }
}

pub(crate) fn open_store(app: &App, stored: &str) -> String {
    let Some(rest) = stored.strip_prefix("seal:") else {
        return stored.to_string();
    };
    let Ok(bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, rest) else {
        return String::new();
    };
    app.identity
        .open(&bytes)
        .ok()
        .and_then(|plain| String::from_utf8(plain).ok())
        .unwrap_or_default()
}
