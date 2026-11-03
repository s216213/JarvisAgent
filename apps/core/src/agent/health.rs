use serde_json::{json, Value};
use tokio::process::Command;
use turso::params;

use crate::db;
use crate::state::App;

use super::common::{vault_path};

pub async fn health_tick(app: &App) -> anyhow::Result<()> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "health_guard").await? {
        return Ok(());
    }
    if std::env::var("JARVIS_DOCKER").ok().as_deref() != Some("1") {
        return Ok(());
    }
    let output = Command::new("docker")
        .args(["compose", "ps", "--format", "json"])
        .output()
        .await;
    let Ok(output) = output else {
        return Ok(());
    };
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let name = value["Service"].as_str().unwrap_or("").to_string();
        let state = value["State"].as_str().unwrap_or("");
        if name.is_empty() || name == "core" || state.contains("running") {
            continue;
        }
        let hour = chrono::Utc::now().format("%Y%m%d%H").to_string();
        let mut rows = conn
            .query(
                "SELECT count FROM restarts WHERE service=?1 AND hour_key=?2",
                params![name.as_str(), hour.as_str()],
            )
            .await?;
        let count: i64 = if let Some(row) = rows.next().await? {
            row.get(0)?
        } else {
            0
        };
        if count >= 3 {
            app.quiet(&format!("Nie podnoszę dalej usługi {name}.")).await;
            continue;
        }
        let _ = Command::new("docker")
            .args(["compose", "restart", &name])
            .status()
            .await;
        conn.execute(
            "INSERT INTO restarts (service, hour_key, count) VALUES (?1, ?2, ?3)
             ON CONFLICT(service, hour_key) DO UPDATE SET count = excluded.count",
            params![name.as_str(), hour.as_str(), count + 1],
        )
        .await?;
        app.quiet(&format!("Podniosłem usługę {name}.")).await;
    }
    if let Some(problem) = crate::host::tunnel_problem().await {
        app.quiet(&problem).await;
    }
    if db::flag(&conn, "log_analyzer").await? {
        scan_logs(app).await;
    }
    Ok(())
}

async fn scan_logs(app: &App) {
    let path = std::env::var("JARVIS_LOG").unwrap_or_else(|_| "data/jarvis.log".into());
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let tail: String = text.chars().rev().take(4000).collect::<String>().chars().rev().collect();
    if !(tail.contains("panic") || tail.contains(" 500 ")) {
        return;
    }
    if let Ok(completion) = app
        .llm
        .complete(
            "Zaproponuj poprawkę po polsku. Nie wdrażaj jej.",
            &format!("Log:\n{tail}"),
        )
        .await
    {
        let dir = vault_path().join("jarvis/naprawy");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("{}.md", chrono::Utc::now().timestamp()));
        let _ = std::fs::write(&file, &completion.text);
        let _ = crate::selfmap::record(
            app,
            "fix",
            "czeka",
            "Propozycja poprawki z logu",
            &file.display().to_string(),
        )
        .await;
        app.quiet("Zostawiłem propozycję poprawki w vaultcie. Nie wdrażam jej.").await;
        app.send_presence(&json!({
            "type": "widget",
            "widget": {"type": "note", "title": "Naprawa", "body": file.display().to_string()}
        }))
        .await;
    }
}
