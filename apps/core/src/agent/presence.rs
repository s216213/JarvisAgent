use std::time::Duration;

use serde_json::{json, Value};
use turso::params;

use crate::db;
use crate::secrets::looks_like_secret;
use crate::state::App;

use super::common::{open_store, say};

pub async fn ingest_clipboard(app: &App, device_id: &str, text: &str) -> anyhow::Result<()> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "clipboard").await? || looks_like_secret(text) {
        return Ok(());
    }
    let clients = app.clients.lock().await;
    let others: Vec<String> = clients
        .keys()
        .filter(|id| id.as_str() != device_id)
        .cloned()
        .collect();
    drop(clients);
    for id in others {
        app.send(&id, &json!({"type":"clipboard_set","text": text, "from": device_id}))
            .await;
    }
    Ok(())
}

pub async fn update_presence(app: &App, device_id: &str, payload: &Value) -> anyhow::Result<()> {
    let conn = app.db.connect()?;
    let now = chrono::Utc::now().timestamp();
    let idle = if payload.get("idle").and_then(|v| v.as_bool()).unwrap_or(true) { 1 } else { 0 };
    let away = if payload.get("away").and_then(|v| v.as_bool()).unwrap_or(false) { 1 } else { 0 };
    let speech = if payload.get("speech").and_then(|v| v.as_bool()).unwrap_or(false) { 1 } else { 0 };
    conn.execute(
        "UPDATE devices SET last_seen=?1, idle=?2, away=?3, speech=?4, app=?5, window_title=?6, telemetry=?7 WHERE id=?8",
        params![
            now,
            idle,
            away,
            speech,
            payload.get("app").and_then(|v| v.as_str()).unwrap_or(""),
            payload.get("window").and_then(|v| v.as_str()).unwrap_or(""),
            payload.get("telemetry").map(|v| v.to_string()).unwrap_or_default(),
            device_id
        ],
    )
    .await?;
    if let (Some(lat), Some(lon)) = (
        payload.pointer("/telemetry/lat").and_then(|v| v.as_f64()),
        payload.pointer("/telemetry/lon").and_then(|v| v.as_f64()),
    ) {
        if device_id.contains("desktop") {
            *app.desk.lock().await = Some((lat, lon));
        } else if let Some((desk_lat, desk_lon)) = *app.desk.lock().await {
            if haversine_km(lat, lon, desk_lat, desk_lon) > 0.2 {
                if let Some(phone) = find_kind(app, "android").await {
                    move_whole(app, &phone, false).await;
                }
            }
        }
    }
    if device_id.contains("desktop") {
        if let Some(tel) = payload.get("telemetry").filter(|value| value.is_object()) {
            let mut pulse = app.pulse.lock().await;
            if let Some(cpu) = number(tel, &["cpu", "Cpu"]) {
                pulse.cpu = Some(cpu);
            }
            if let Some(ram) = number(tel, &["ram", "Ram"]) {
                pulse.ram = Some(ram);
            }
            if let Some(battery) = number(tel, &["battery", "Battery"]) {
                pulse.battery = Some(battery);
            }
            if let Some(network) = text_field(tel, &["network", "Network"]) {
                pulse.network = network;
            }
        }
    }
    if speech == 1 {
        set_presence(app, device_id).await;
    } else if away == 1 && device_id.contains("desktop") {
        if let Some(phone) = find_kind(app, "android").await {
            move_whole(app, &phone, false).await;
        }
    }
    let learning = *app.study.lock().await;
    if db::flag(&conn, "proactive").await? && !learning {
        maybe_battery(app, payload).await;
        if device_id.contains("desktop") {
            maybe_cpu(app).await;
            maybe_window(app, payload).await;
        }
    }
    if device_id.contains("desktop") && !learning {
        maybe_greet(app, device_id, idle, away).await;
    }
    app.publish_hud().await;
    crate::board::publish(app).await;
    Ok(())
}

async fn maybe_battery(app: &App, payload: &Value) {
    let Some(level) = payload
        .get("telemetry")
        .and_then(|tel| number(tel, &["battery", "Battery"]))
    else {
        return;
    };
    if level > 20.0 {
        *app.battery_said.lock().await = false;
        let alert = app.hud.lock().await.alert.clone();
        if alert.starts_with("Bateria") {
            app.set_alert("").await;
        }
        return;
    }
    if level > 12.0 || *app.battery_said.lock().await {
        return;
    }
    let Ok(conn) = app.db.connect() else { return };
    let Ok(mut rows) = conn
        .query("SELECT body FROM facts ORDER BY id DESC LIMIT 20", ())
        .await
    else {
        return;
    };
    let mut soon = false;
    while let Ok(Some(row)) = rows.next().await {
        let body: String = row.get(0).unwrap_or_default();
        let plain = open_store(app, &body).to_lowercase();
        if plain.contains("sesj") || plain.contains("20 minut") {
            soon = true;
            break;
        }
    }
    if !soon {
        return;
    }
    *app.battery_said.lock().await = true;
    let line = format!("Bateria ma {level:.0} procent, a niedługo masz sesję.");
    app.set_alert(&line).await;
    say(app, &line).await;
}

async fn maybe_cpu(app: &App) {
    let Some(cpu) = app.pulse.lock().await.cpu else {
        return;
    };
    if cpu > 90.0 {
        let count = {
            let mut high = app.cpu_high.lock().await;
            *high = high.saturating_add(1);
            *high
        };
        let said = *app.cpu_said.lock().await;
        if count >= 2 && !said {
            *app.cpu_said.lock().await = true;
            let line = if persona_on(app).await {
                format!("Panie, procesor jest przy {cpu:.0} procentach.")
            } else {
                format!("Procesor jest przy {cpu:.0} procentach.")
            };
            app.set_alert(&line).await;
            say(app, &line).await;
        }
        return;
    }
    *app.cpu_high.lock().await = 0;
    *app.cpu_said.lock().await = false;
    let alert = app.hud.lock().await.alert.to_lowercase();
    if alert.contains("procesor") {
        app.set_alert("").await;
    }
}

async fn maybe_window(app: &App, payload: &Value) {
    let app_name = payload.get("app").and_then(|value| value.as_str()).unwrap_or("").trim();
    let title = payload.get("window").and_then(|value| value.as_str()).unwrap_or("").trim();
    if app_name.is_empty() && title.is_empty() {
        return;
    }
    let key = format!("{app_name}|{title}");
    let mut last = app.last_window.lock().await;
    if last.is_empty() {
        *last = key;
        return;
    }
    if *last == key {
        return;
    }
    *last = key;
    drop(last);
    if app_name.to_lowercase().contains("jarvis") {
        return;
    }
    let mut said = app.window_said.lock().await;
    if said.is_some_and(|instant| instant.elapsed() < Duration::from_secs(45)) {
        return;
    }
    *said = Some(std::time::Instant::now());
    drop(said);
    let shown = if title.is_empty() { app_name } else { title };
    let line = if persona_on(app).await {
        format!("Panie, na wierzchu jest {shown}.")
    } else {
        format!("Na wierzchu jest {shown}.")
    };
    app.set_alert(&line).await;
    say(app, &line).await;
}

async fn maybe_greet(app: &App, device_id: &str, idle: i64, away: i64) {
    if away != 0 || idle != 0 {
        return;
    }
    let Ok(conn) = app.db.connect() else {
        return;
    };
    if !db::flag(&conn, "briefing").await.unwrap_or(false) {
        return;
    }
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut greeted = app.greeted_day.lock().await;
    if greeted.as_deref() == Some(day.as_str()) {
        return;
    }
    *greeted = Some(day);
    drop(greeted);
    set_presence(app, device_id).await;
    match crate::briefing::report(app, true).await {
        Ok(text) => say(app, &text).await,
        Err(err) => say(app, &format!("Nie złożyłem powitania. {err}")).await,
    }
    app.send_presence(&json!({"type":"done"})).await;
}

async fn persona_on(app: &App) -> bool {
    let Ok(conn) = app.db.connect() else {
        return true;
    };
    db::flag(&conn, "persona").await.unwrap_or(true)
}

fn number(value: &Value, keys: &[&str]) -> Option<f64> {
    for key in keys {
        let Some(item) = value.get(*key) else {
            continue;
        };
        if let Some(number) = item.as_f64() {
            return Some(number);
        }
        if let Some(number) = item.as_i64() {
            return Some(number as f64);
        }
    }
    None
}

fn text_field(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(|item| item.as_str()) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

async fn find_kind(app: &App, kind: &str) -> Option<String> {
    let clients = app.clients.lock().await;
    clients.keys().find(|id| id.contains(kind)).cloned()
}

pub(crate) async fn move_whole(app: &App, device_id: &str, force: bool) {
    if !force {
        if let Some(prev) = *app.last_move.lock().await {
            if prev.elapsed() < Duration::from_secs(60) {
                return;
            }
        }
    }
    *app.last_move.lock().await = Some(std::time::Instant::now());
    set_presence(app, device_id).await;
    app.send(
        device_id,
        &json!({"type":"move_presence","device_id": device_id, "whole_task": true}),
    )
    .await;
}

pub(crate) async fn set_presence(app: &App, device_id: &str) {
    if *app.pinned.lock().await {
        return;
    }
    *app.presence.lock().await = Some(device_id.to_string());
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6371.0;
    let p1 = lat1.to_radians();
    let p2 = lat2.to_radians();
    let dp = (lat2 - lat1).to_radians();
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    r * 2.0 * a.sqrt().asin()
}

pub async fn ingest_notice(app: &App, source: &str, title: &str, body: &str) -> anyhow::Result<()> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "digest").await? {
        return Ok(());
    }
    let line = format!("{source}: {title}. {body}");
    if *app.study.lock().await {
        app.notices.lock().await.push(line);
        return Ok(());
    }
    summarize_notices(app, vec![line]).await;
    Ok(())
}

pub(crate) async fn flush_notices(app: &App) {
    let batch = std::mem::take(&mut *app.notices.lock().await);
    if batch.is_empty() {
        return;
    }
    summarize_notices(app, batch).await;
}

async fn summarize_notices(app: &App, batch: Vec<String>) {
    let joined = batch.join("\n");
    let summary = app
        .llm
        .complete(
            "Zostaw po polsku tylko ludzi, terminy i alarmy. Jedno zdanie.",
            &joined,
        )
        .await
        .map(|item| item.text)
        .unwrap_or(joined);
    *app.last_digest.lock().await = summary.chars().take(200).collect();
    app.send_presence(&json!({
        "type": "widget",
        "widget": {"type": "note", "title": "Skrót", "body": summary}
    }))
    .await;
    app.quiet(&summary).await;
}
