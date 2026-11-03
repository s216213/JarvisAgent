use serde_json::{json, Value};
use turso::params;
use uuid::Uuid;

use crate::state::App;

pub(crate) fn pins(kind: &str) -> bool {
    matches!(kind, "shell" | "script" | "glance" | "desktop_mode" | "apply_patch")
}

pub(crate) async fn begin(app: &App, kind: &str, device_id: &str, detail: &str) -> anyhow::Result<String> {
    let id = Uuid::new_v4().to_string();
    let conn = app.db.connect()?;
    conn.execute(
        "INSERT INTO jobs (id, kind, pinned, device_id, status, detail) VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
        params![id.as_str(), kind, i64::from(pins(kind)), device_id, detail],
    )
    .await?;
    crate::board::publish(app).await;
    Ok(id)
}

pub(crate) async fn close(app: &App, id: &str, status: &str, detail: &str) -> anyhow::Result<()> {
    let detail: String = detail.chars().take(500).collect();
    let conn = app.db.connect()?;
    conn.execute(
        "UPDATE jobs SET status = ?1, detail = ?2 WHERE id = ?3",
        params![status, detail.as_str(), id],
    )
    .await?;
    crate::board::publish(app).await;
    Ok(())
}

pub async fn recent(conn: &turso::Connection) -> Vec<Value> {
    let Ok(mut rows) = conn
        .query(
            "SELECT id, kind, pinned, device_id, status, detail FROM jobs ORDER BY rowid DESC LIMIT 24",
            (),
        )
        .await
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next().await {
        let id: String = row.get(0).unwrap_or_default();
        let kind: String = row.get(1).unwrap_or_default();
        let pinned: i64 = row.get(2).unwrap_or(0);
        let device: String = row.get(3).unwrap_or_default();
        let status: String = row.get(4).unwrap_or_default();
        let detail: String = row.get(5).unwrap_or_default();
        out.push(json!({
            "id": id,
            "kind": kind,
            "pinned": pinned == 1,
            "device_id": device,
            "status": status,
            "detail": detail,
        }));
    }
    out
}
