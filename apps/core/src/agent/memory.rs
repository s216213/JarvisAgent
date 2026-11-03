use std::path::PathBuf;

use serde_json::{json, Value};
use turso::params;

use crate::crypto::fingerprint;
use crate::db;
use crate::embed;
use crate::secrets::looks_like_secret;
use crate::state::App;

use super::common::{confirm, open_store, seal_store, vault_path};

pub(crate) async fn file_note(app: &App, text: &str) -> anyhow::Result<()> {
    let vault = vault_path();
    let folders = list_dirs(&vault);
    let prompt = format!(
        "Folderów w vaultcie: {}. Zwróć JSON {{\"folder\":\"\",\"title\":\"\",\"tags\":[],\"body\":\"\"}} dla myśli: {text}",
        folders.join(", ")
    );
    let raw = app
        .llm
        .complete("Klasyfikujesz notatki. Tylko JSON.", &prompt)
        .await
        .map(|c| c.text)
        .unwrap_or_else(|_| {
            json!({"folder":"jarvis/pamiec","title":"Myśl","tags":["jarvis"],"body": text}).to_string()
        });
    let parsed: Value = serde_json::from_str(raw.trim().trim_matches('`')).unwrap_or_else(|_| {
        json!({"folder":"jarvis/pamiec","title":"Myśl","tags":["jarvis"],"body": text})
    });
    let folder = parsed["folder"].as_str().unwrap_or("jarvis/pamiec");
    let title = parsed["title"].as_str().unwrap_or("Myśl");
    let body = parsed["body"].as_str().unwrap_or(text);
    let tags = parsed["tags"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    app.send_presence(&json!({
        "type": "widget",
        "widget": {"type": "note", "title": title, "body": format!("{folder} [{tags}]")}
    }))
    .await;
    if !confirm(app, &format!("Zapiszę notatkę w {folder}."), &tags).await? {
        return Ok(());
    }
    let folder = app
        .note_folder
        .lock()
        .await
        .take()
        .unwrap_or_else(|| folder.to_string());
    let dir = vault.join(&folder);
    std::fs::create_dir_all(&dir)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("{stamp}.md"));
    let links = related_links(app, body).await.unwrap_or_default();
    let head = note_template()
        .replace("{title}", title)
        .replace("{tags}", &tags)
        .replace("{stamp}", &stamp.to_string());
    let note = format!("{head}\n\n{body}\n\n## Powiązania\n{links}\n");
    std::fs::write(&path, note)?;
    index_file(app, &path).await?;
    app.send_presence(&json!({
        "type": "widget",
        "widget": {"type":"note","title": title, "body": path.display().to_string()}
    }))
    .await;
    Ok(())
}

fn list_dirs(root: &PathBuf) -> Vec<String> {
    let mut names = vec!["jarvis/pamiec".into(), "jarvis/wzrok".into(), "dziennik".into()];
    if let Ok(read) = std::fs::read_dir(root) {
        for entry in read.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

pub async fn index_file(app: &App, path: &PathBuf) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let display = path.display().to_string();
    let vector = embed::to_json(&embed::embed(&text));
    let stored = seal_store(app, &text);
    let conn = app.db.connect()?;
    conn.execute(
        "INSERT INTO chunks (path, body, embedding) VALUES (?1, ?2, ?3)",
        params![display.as_str(), stored.as_str(), vector.as_str()],
    )
    .await?;
    let _ = conn
        .execute(
            "INSERT INTO chunks_fts (path, body) VALUES (?1, ?2)",
            params![display.as_str(), text.as_str()],
        )
        .await;
    Ok(())
}

async fn related_links(app: &App, text: &str) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "links").await? {
        return Ok(String::new());
    }
    let hits = recall(app, text).await?;
    Ok(hits
        .lines()
        .filter(|line| line.starts_with("- "))
        .take(5)
        .map(|line| format!("- [[{}]]", line.trim_start_matches("- ").split(':').next().unwrap_or(line)))
        .collect::<Vec<_>>()
        .join("\n"))
}

pub(crate) async fn recall(app: &App, query: &str) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    let mut rows = conn
        .query(
            "SELECT path, body, embedding FROM chunks ORDER BY id DESC LIMIT 80",
            (),
        )
        .await?;
    let needle = embed::embed(query);
    let mut scored = Vec::new();
    while let Some(row) = rows.next().await? {
        let path: String = row.get(0)?;
        let stored: String = row.get(1)?;
        let body = open_store(app, &stored);
        let embedding: String = row.get(2)?;
        let score = embed::cosine(&needle, &embed::from_json(&embedding));
        let lexical = if body.to_lowercase().contains(&query.to_lowercase()) {
            0.25
        } else {
            0.0
        };
        scored.push((score + lexical, path, body.chars().take(240).collect::<String>()));
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut lines = Vec::new();
    let mut facts = conn
        .query("SELECT body FROM facts ORDER BY id DESC LIMIT 8", ())
        .await?;
    while let Some(row) = facts.next().await? {
        let stored: String = row.get(0)?;
        let body = open_store(app, &stored);
        lines.push(format!("- fakt: {body}"));
    }
    for (_, path, snippet) in scored.into_iter().take(4) {
        lines.push(format!("- {path}: {snippet}"));
    }
    let mut recent = conn
        .query(
            "SELECT role, body FROM messages ORDER BY id DESC LIMIT 8",
            (),
        )
        .await?;
    let mut short = Vec::new();
    while let Some(row) = recent.next().await? {
        let role: String = row.get(0)?;
        let stored: String = row.get(1)?;
        let body = open_store(app, &stored);
        short.push(format!("{role}: {body}"));
    }
    short.reverse();
    lines.extend(short);
    Ok(lines.join("\n"))
}

pub(crate) async fn extract_graph(app: &App, text: &str) -> anyhow::Result<()> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "graph").await? {
        return Ok(());
    }
    let hash = fingerprint(text);
    let fact_prompt = format!("Wypisz jeden fakt po polsku albo pusty string. Tekst: {text}");
    if let Ok(completion) = app.llm.complete("Zwracasz sam fakt, bez komentarza.", &fact_prompt).await {
        let fact = completion.text.trim();
        if !fact.is_empty() && fact.len() < 400 {
            let _ = conn
                .execute(
                    "INSERT INTO facts (body, hash, created) VALUES (?1, ?2, ?3)",
                    params![seal_store(app, fact).as_str(), hash.as_str(), chrono::Utc::now().timestamp()],
                )
                .await;
            let path = vault_path().join("jarvis/pamiec");
            std::fs::create_dir_all(&path)?;
            let file = path.join(format!("{hash}.md"));
            if !file.exists() {
                std::fs::write(&file, format!("# Fakt\n\n{fact}\n"))?;
            }
        }
    }
    let graph_prompt = format!(
        "Zwróć JSON {{\"nodes\":[{{\"name\":\"\",\"kind\":\"osoba|projekt|technologia|serwer|zmienna|notatka\"}}],\"edges\":[{{\"from\":\"\",\"to\":\"\",\"rel\":\"używa|wdrożony na|powiązany z|należy do\"}}]}}. \
Nie zapisuj wartości sekretów. Tekst: {text}"
    );
    if let Ok(completion) = app
        .llm
        .complete("Tylko JSON grafu.", &graph_prompt)
        .await
    {
        if let Ok(value) = serde_json::from_str::<Value>(completion.text.trim().trim_matches('`')) {
            if let Some(nodes) = value["nodes"].as_array() {
                for node in nodes {
                    let name = node["name"].as_str().unwrap_or("");
                    let kind = node["kind"].as_str().unwrap_or("notatka");
                    if name.is_empty() || looks_like_secret(name) {
                        continue;
                    }
                    let _ = conn
                        .execute(
                            "INSERT INTO entities (name, kind) VALUES (?1, ?2) ON CONFLICT(name, kind) DO NOTHING",
                            params![name, kind],
                        )
                        .await;
                }
            }
            if let Some(edges) = value["edges"].as_array() {
                for edge in edges {
                    let from = edge["from"].as_str().unwrap_or("");
                    let to = edge["to"].as_str().unwrap_or("");
                    let rel = edge["rel"].as_str().unwrap_or("powiązany z");
                    let from_id = entity_id(&conn, from).await?;
                    let to_id = entity_id(&conn, to).await?;
                    if let (Some(from_id), Some(to_id)) = (from_id, to_id) {
                        let _ = conn
                            .execute(
                                "INSERT INTO edges (from_id, to_id, rel) VALUES (?1, ?2, ?3)",
                                params![from_id, to_id, rel],
                            )
                            .await;
                    }
                }
            }
            write_graph_overlay(&conn).await?;
        }
    }
    Ok(())
}

async fn entity_id(conn: &turso::Connection, name: &str) -> anyhow::Result<Option<i64>> {
    let mut rows = conn
        .query("SELECT id FROM entities WHERE name = ?1", params![name])
        .await?;
    if let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        return Ok(Some(id));
    }
    Ok(None)
}

async fn write_graph_overlay(conn: &turso::Connection) -> anyhow::Result<()> {
    let mut rows = conn
        .query(
            "SELECT a.name, e.rel, b.name FROM edges e
             JOIN entities a ON a.id = e.from_id
             JOIN entities b ON b.id = e.to_id
             ORDER BY e.id DESC LIMIT 80",
            (),
        )
        .await?;
    let mut body = String::from("# Graf\n\n");
    while let Some(row) = rows.next().await? {
        let from: String = row.get(0)?;
        let rel: String = row.get(1)?;
        let to: String = row.get(2)?;
        body.push_str(&format!("- [[{from}]] {rel} [[{to}]]\n"));
    }
    let path = vault_path().join("jarvis/graf.md");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, body)?;
    Ok(())
}

fn note_template() -> String {
    let dir = vault_path().join("szablony");
    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            if let Ok(text) = std::fs::read_to_string(entry.path()) {
                if text.contains("title") {
                    return text.lines().take(8).collect::<Vec<_>>().join("\n");
                }
            }
        }
    }
    "---\ntitle: {title}\ntags: [{tags}]\ndate: {stamp}\n---".into()
}

pub(crate) async fn walk_links(app: &App) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    let mut rows = conn
        .query(
            "SELECT a.name, e.rel, b.name FROM edges e
             JOIN entities a ON a.id = e.from_id
             JOIN entities b ON b.id = e.to_id
             ORDER BY e.id DESC LIMIT 12",
            (),
        )
        .await?;
    let mut lines = Vec::new();
    while let Some(row) = rows.next().await? {
        let from: String = row.get(0)?;
        let rel: String = row.get(1)?;
        let to: String = row.get(2)?;
        lines.push(format!("{from} {rel} {to}"));
    }
    if lines.is_empty() {
        Ok("Nie mam jeszcze powiązań.".into())
    } else {
        Ok(lines.join(". "))
    }
}
