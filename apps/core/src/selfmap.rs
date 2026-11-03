use std::path::PathBuf;

use serde_json::{json, Value};

use crate::agent;
use crate::db;
use crate::plugins;
use crate::state::App;

const MODULES: &[(&str, &str)] = &[
    ("agent", "Pętla: jedna tura, jedno narzędzie"),
    ("memory", "Vault, fakty, krawędzie i krótkie wiadomości"),
    ("llm", "Router dostawców"),
    ("routes", "HTTP i WebSocket"),
    ("plugins", "Narzędzia z katalogu, bez przebudowy"),
    ("evolve", "Plugin albo diff, zgoda na ryzyko i rdzeń"),
    ("simulate", "Przebieg próbny bez skutków"),
    ("briefing", "Raport i powitanie"),
    ("mission", "Jedna sprawa w tle"),
    ("voice", "Głos na urządzeniu"),
];

pub async fn record(app: &App, kind: &str, status: &str, summary: &str, detail: &str) -> anyhow::Result<()> {
    let conn = app.db.connect()?;
    db::record_evolution(&conn, kind, status, summary, detail).await?;
    let dir = agent::vault_path().join("jarvis");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("changelog.md");
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M");
    let line = format!("\n## {stamp}\n- [{status}] {kind}: {summary}\n");
    let previous = std::fs::read_to_string(&path).unwrap_or_else(|_| "# Historia Jarvisa\n".into());
    std::fs::write(&path, format!("{previous}{line}"))?;
    Ok(())
}

pub async fn brief(app: &App) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    let chunks = db::count_rows(&conn, "chunks").await.unwrap_or(0);
    let facts = db::count_rows(&conn, "facts").await.unwrap_or(0);
    let edges = db::count_rows(&conn, "edges").await.unwrap_or(0);
    let messages = db::count_rows(&conn, "messages").await.unwrap_or(0);
    let history = db::list_evolution(&conn, 3).await.unwrap_or_default();
    let last = history
        .first()
        .map(|(_, kind, status, summary)| format!("{kind}/{status}: {summary}"))
        .unwrap_or_else(|| "brak wpisów".into());
    let tools = plugins::prompt_lines();
    Ok(format!(
        "Jestem Jarvisem. Rdzeń jest w Rust, klienci tylko rozmawiają.\n\
Moduły: pętla, pamięć, dostawcy, trasa, pluginy, ewolucja, symulacja, raport, misja, głos.\n\
Pamięć: {chunks} fragmentów, {facts} faktów, {edges} krawędzi, {messages} wiadomości.\n\
Pluginy:\n{tools}\n\
Ostatnia zmiana: {last}."
    ))
}

pub async fn scene(app: &App) -> anyhow::Result<Value> {
    let conn = app.db.connect()?;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for (id, label) in MODULES {
        nodes.push(json!({"id": id, "label": label, "group": "core"}));
        if *id != "agent" {
            edges.push(json!({"from": "agent", "to": id}));
        }
    }
    nodes.push(json!({
        "id": "memory-store",
        "label": format!(
            "pamięć {} / {}",
            db::count_rows(&conn, "chunks").await.unwrap_or(0),
            db::count_rows(&conn, "facts").await.unwrap_or(0)
        ),
        "group": "memory"
    }));
    edges.push(json!({"from": "memory", "to": "memory-store"}));
    for name in ["search", "map", "remember", "shell", "script", "schedule", "glance", "self_map", "simulate", "evolve", "status", "mission"] {
        let id = format!("tool:{name}");
        nodes.push(json!({"id": id, "label": name, "group": "tool"}));
        edges.push(json!({"from": "agent", "to": id}));
    }
    for plugin in plugins::list() {
        let id = format!("plugin:{}", plugin.id);
        nodes.push(json!({"id": id, "label": plugin.id, "group": "plugin"}));
        edges.push(json!({"from": "plugins", "to": id}));
    }
    for (created, kind, status, summary) in db::list_evolution(&conn, 8).await.unwrap_or_default() {
        let id = format!("evo:{created}");
        let label: String = summary.chars().take(48).collect();
        nodes.push(json!({"id": id, "label": format!("{kind} {status}: {label}"), "group": "history"}));
        edges.push(json!({"from": "evolve", "to": id}));
    }
    let desk = app.desk.lock().await.clone();
    let (lat, lon) = desk.unwrap_or((52.23, 21.01));
    Ok(json!({
        "text": brief(app).await?,
        "nodes": nodes,
        "edges": edges,
        "map": {"lat": lat, "lon": lon, "zoom": 11}
    }))
}

pub async fn publish(app: &App, ghost: Option<Vec<Value>>) -> anyhow::Result<String> {
    let scene = scene(app).await?;
    let text = scene["text"].as_str().unwrap_or("").to_string();
    app.send_presence(&json!({
        "type": "widget",
        "widget": {
            "type": "self_map",
            "title": "Mapa siebie",
            "body": text
        }
    }))
    .await;
    let conn = app.db.connect()?;
    if db::flag(&conn, "space").await.unwrap_or(true) {
        let mut payload = scene.clone();
        payload["type"] = json!("space_scene");
        if let Some(ghost) = ghost {
            payload["ghost"] = json!(ghost);
        }
        app.send_presence(&payload).await;
        let ids: Vec<String> = app
            .clients
            .lock()
            .await
            .keys()
            .filter(|id| id.contains("space"))
            .cloned()
            .collect();
        for id in ids {
            app.send(&id, &payload).await;
        }
    }
    Ok(text)
}

pub fn proposals_dir() -> PathBuf {
    agent::vault_path().join("jarvis/propozycje")
}
