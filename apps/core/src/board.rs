use serde_json::{json, Value};
use crate::agent::jobs;
use crate::db;
use crate::state::App;

pub async fn snapshot(app: &App) -> Value {
    let Ok(conn) = app.db.connect() else {
        return json!({"error": "baza"});
    };
    let scene = crate::selfmap::scene(app)
        .await
        .unwrap_or_else(|err| json!({"text": err.to_string(), "nodes": [], "edges": []}));
    let plugins = crate::plugins::list()
        .into_iter()
        .map(|plugin| {
            json!({
                "id": plugin.id,
                "description": plugin.description,
                "enabled": plugin.enabled,
                "capabilities": plugin.capabilities,
                "trust": plugin.trust,
            })
        })
        .collect::<Vec<_>>();
    let proposals = crate::evolve::list_proposals();
    let job_rows = jobs::recent(&conn).await;
    let schedules = schedules(&conn).await;
    let audit = db::recent_audit(&conn, 12).await.unwrap_or_default();
    let evolution = db::list_evolution(&conn, 8).await.unwrap_or_default();
    let evolution_json = evolution
        .iter()
        .map(|(created, kind, status, summary)| {
            json!({"created": created, "kind": kind, "status": status, "summary": summary})
        })
        .collect::<Vec<_>>();
    let agents = cards(&job_rows, &plugins, &evolution);
    let feed = feed(&conn, &audit, app).await;
    json!({
        "scene": scene,
        "plugins": plugins,
        "proposals": proposals,
        "jobs": job_rows,
        "schedules": schedules,
        "audit": audit,
        "evolution": evolution_json,
        "agents": agents,
        "providers": crate::llm::Router::providers(),
        "recent_429": app.llm.recent_429().await,
        "host": crate::host::resources(),
        "devices": devices(&conn).await,
        "feed": feed,
        "memory": {
            "chunks": db::count_rows(&conn, "chunks").await.unwrap_or(0),
            "facts": db::count_rows(&conn, "facts").await.unwrap_or(0),
            "edges": db::count_rows(&conn, "edges").await.unwrap_or(0),
            "messages": db::count_rows(&conn, "messages").await.unwrap_or(0),
            "lexical": true
        },
        "presence": app.presence.lock().await.clone(),
    })
}

pub async fn publish(app: &App) {
    let mut body = snapshot(app).await;
    body["type"] = json!("board");
    let text = body.to_string();
    let clients = app.clients.lock().await;
    for tx in clients.values() {
        let _ = tx.send(text.clone());
    }
}

fn cards(jobs: &[Value], plugins: &[Value], evolution: &[(i64, String, String, String)]) -> Vec<Value> {
    let running = |kinds: &[&str]| {
        jobs.iter().any(|job| {
            job["status"].as_str() == Some("running")
                && kinds.iter().any(|kind| job["kind"].as_str() == Some(kind))
        })
    };
    let one = |id: &str, label: &str, detail: &str, live: bool| {
        json!({
            "id": id,
            "label": label,
            "detail": detail,
            "status": if live { "running" } else { "standby" }
        })
    };
    let mut out = vec![
        one("research", "Research", "search", running(&["search"])),
        one("memory", "Memory", "remember", running(&["remember"])),
        one("coding", "Coding", "shell / script", running(&["shell", "script"])),
        one("system", "System", "pulpit", running(&["desktop_mode"])),
        one("glance", "Glance", "wzrok", running(&["glance"])),
        one("schedule", "Schedule", "harmonogram", running(&["schedule"])),
    ];
    for plugin in plugins {
        let id = plugin["id"].as_str().unwrap_or("");
        let enabled = plugin["enabled"].as_bool().unwrap_or(false);
        out.push(json!({
            "id": format!("plugin:{id}"),
            "label": id,
            "detail": if enabled { "plugin" } else { "wyłączony" },
            "status": if running(&[id]) { "running" } else { "standby" }
        }));
    }
    let detail = evolution
        .first()
        .map(|(_, kind, status, summary)| format!("{kind}/{status}: {summary}"))
        .unwrap_or_else(|| "brak propozycji".into());
    out.push(one("evolve", "Evolve", &detail, running(&["evolve", "apply_patch"])));
    out
}

async fn schedules(conn: &turso::Connection) -> Vec<Value> {
    let Ok(mut rows) = conn
        .query("SELECT name, at, instruction, last_day FROM schedules ORDER BY at", ())
        .await
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        out.push(json!({
            "name": row.get::<String>(0).unwrap_or_default(),
            "at": row.get::<String>(1).unwrap_or_default(),
            "instruction": row.get::<String>(2).unwrap_or_default(),
            "last_day": row.get::<String>(3).unwrap_or_default(),
        }));
    }
    out
}

async fn devices(conn: &turso::Connection) -> Vec<Value> {
    let Ok(mut rows) = conn
        .query(
            "SELECT id, app, window_title, telemetry, idle, away FROM devices ORDER BY last_seen DESC LIMIT 6",
            (),
        )
        .await
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        let telemetry = row.get::<String>(3).unwrap_or_default();
        let parsed = serde_json::from_str::<Value>(&telemetry).unwrap_or(json!({}));
        out.push(json!({
            "id": row.get::<String>(0).unwrap_or_default(),
            "app": row.get::<String>(1).unwrap_or_default(),
            "window": row.get::<String>(2).unwrap_or_default(),
            "telemetry": parsed,
            "idle": row.get::<i64>(4).unwrap_or(1) == 1,
            "away": row.get::<i64>(5).unwrap_or(0) == 1,
        }));
    }
    out
}

async fn feed(conn: &turso::Connection, audit: &[String], app: &App) -> Vec<Value> {
    let mut out = Vec::new();
    for line in audit {
        out.push(json!({"kind": "audit", "text": line}));
    }
    let Ok(mut rows) = conn
        .query(
            "SELECT role, body FROM messages ORDER BY id DESC LIMIT 6",
            (),
        )
        .await
    else {
        return out;
    };
    let mut lines = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        let role: String = row.get(0).unwrap_or_default();
        let stored: String = row.get(1).unwrap_or_default();
        let body = crate::agent::open_store(app, &stored);
        let text: String = body.chars().take(180).collect();
        if !text.is_empty() {
            lines.push(json!({"kind": role, "text": text}));
        }
    }
    lines.reverse();
    out.extend(lines);
    out
}
