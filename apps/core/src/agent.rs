use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::process::Command;
use turso::params;
use uuid::Uuid;

use crate::crypto::fingerprint;
use crate::db;
use crate::embed;
use crate::secrets::looks_like_secret;
use crate::state::App;

const SYSTEM: &str = "Jesteś Jarvisem, osobistym asystentem. Mówisz po polsku, krótko i konkretnie. \
Gdy potrzebujesz narzędzia, odpowiedz wyłącznie JSON bez markdown: \
{\"say\":\"krótko co robisz\",\"tool\":{\"name\":\"nazwa\",\"args\":{}}}. \
Gdy nie potrzebujesz narzędzia, odpowiedz zwykłym tekstem. \
Narzędzia: search (q), map (lat, lon, title), remember (text), shell (target oci|windows, command), \
script (target oci|windows, lang powershell|bash|python, code), \
schedule (name, at HH:MM, instruction, remove bool), \
move_presence (device), desktop_mode (name), \
glance (nic, gdy chcesz spojrzeć na ekran). \
Shell i script zawsze wymagają zgody użytkownika, nie omijaj jej.";

pub async fn handle_text(app: &App, device_id: &str, text: &str) -> anyhow::Result<()> {
    if let Some(answer) = pending_answer(app, text).await? {
        let _ = answer;
        return Ok(());
    }
    let lower = text.to_lowercase();
    if lower.contains("zostań tutaj") || lower.contains("zostan tutaj") {
        *app.pinned.lock().await = true;
        say(app, "Zostaję tutaj.").await;
        return Ok(());
    }
    if lower.contains("możesz przełączać") || lower.contains("mozesz przelaczac") {
        *app.pinned.lock().await = false;
        say(app, "Mogę przełączać.").await;
        return Ok(());
    }
    if lower.contains("przenieś") || lower.contains("przenies") {
        *app.pinned.lock().await = false;
        let target = if lower.contains("telefon") {
            "android-phone"
        } else if lower.contains("czat") {
            "web-chat"
        } else {
            "desktop-pc"
        };
        move_whole(app, target, true).await;
        say(app, "Jestem przy tobie.").await;
        return Ok(());
    }
    if lower.contains("z czym") {
        let links = walk_links(app).await?;
        say(app, &links).await;
        app.send_presence(&json!({"type":"done"})).await;
        return Ok(());
    }
    set_presence(app, device_id).await;
    let conn = app.db.connect()?;
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO messages (role, body, created) VALUES ('user', ?1, ?2)",
        params![seal_store(app, text).as_str(), now],
    )
    .await?;

    if db::flag(&conn, "classifier").await? && wants_note(text) {
        file_note(app, text).await?;
    }

    let memory = recall(app, text).await?;
    let prompt = format!("Pamięć i notatki:\n{memory}\n\nUżytkownik: {text}");
    let completion = match app.llm.complete(SYSTEM, &prompt).await {
        Ok(c) => c,
        Err(err) => {
            say(app, &format!("Nie mogę teraz odpowiedzieć. {err}")).await;
            return Ok(());
        }
    };
    db::audit(&conn, "llm", &completion.provider).await?;
    dispatch(app, device_id, &completion.text).await?;
    let _ = extract_graph(app, text).await;
    Ok(())
}

fn wants_note(text: &str) -> bool {
    let lower = text.to_lowercase();
    lower.contains("zapisz") || lower.contains("zapamiętaj myśl") || lower.contains("notatka")
}

async fn pending_answer(app: &App, text: &str) -> anyhow::Result<Option<()>> {
    let confirms = app.confirms.lock().await;
    if confirms.is_empty() {
        return Ok(None);
    }
    drop(confirms);
    let lower = text.trim().to_lowercase();
    let mut accept = matches!(lower.as_str(), "tak" | "potwierdź" | "potwierdz" | "wykonaj");
    let deny = matches!(lower.as_str(), "nie" | "anuluj");
    if lower.contains("to do ") {
        let folder = lower.split("to do ").last().unwrap_or("").trim().to_string();
        if !folder.is_empty() {
            *app.note_folder.lock().await = Some(folder);
            accept = true;
        }
    }
    if !accept && !deny {
        return Ok(None);
    }
    let mut confirms = app.confirms.lock().await;
    if let Some(id) = confirms.keys().next().cloned() {
        if let Some(tx) = confirms.remove(&id) {
            let _ = tx.send(accept);
            let conn = app.db.connect()?;
            db::audit(
                &conn,
                "confirm",
                &format!("{id} via=text accept={accept}"),
            )
            .await?;
        }
    }
    Ok(Some(()))
}

pub async fn resolve_confirm(app: &App, id: &str, accept: bool, via: &str) -> anyhow::Result<()> {
    let mut confirms = app.confirms.lock().await;
    if let Some(tx) = confirms.remove(id) {
        let _ = tx.send(accept);
    }
    drop(confirms);
    let conn = app.db.connect()?;
    db::audit(&conn, "confirm", &format!("{id} via={via} accept={accept}")).await?;
    Ok(())
}

async fn dispatch(app: &App, device_id: &str, raw: &str) -> anyhow::Result<()> {
    let trimmed = raw.trim().trim_start_matches("```json").trim_end_matches("```").trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        if let Some(say_text) = value.get("say").and_then(|v| v.as_str()) {
            if !say_text.is_empty() {
                say(app, say_text).await;
            }
        }
        if let Some(tool) = value.get("tool") {
            let name = tool.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = tool.get("args").cloned().unwrap_or_else(|| json!({}));
            run_tool(app, device_id, name, &args).await?;
            return Ok(());
        }
        if value.get("say").is_some() {
            app.send_presence(&json!({"type":"done"})).await;
            return Ok(());
        }
    }
    say(app, trimmed).await;
    app.send_presence(&json!({"type":"done"})).await;
    store_assistant(app, trimmed).await?;
    Ok(())
}

async fn run_tool(app: &App, device_id: &str, name: &str, args: &Value) -> anyhow::Result<()> {
    let conn = app.db.connect()?;
    let output = match name {
        "map" => {
            if !db::flag(&conn, "map").await? {
                "Mapa jest wyłączona.".into()
            } else {
                let lat = args.get("lat").and_then(|v| v.as_f64()).unwrap_or(52.23);
                let lon = args.get("lon").and_then(|v| v.as_f64()).unwrap_or(21.01);
                let title = args.get("title").and_then(|v| v.as_str()).unwrap_or("Mapa");
                let weather = weather_line(lat, lon).await;
                app.send_presence(&json!({
                    "type": "widget",
                    "widget": {
                        "type": "map",
                        "title": title,
                        "lat": lat,
                        "lon": lon,
                        "center": [lon, lat],
                        "zoom": 11,
                        "layers": ["weather", "location"],
                        "body": weather
                    }
                }))
                .await;
                format!("Mapa: {title}. {weather}")
            }
        }
        "search" => {
            if !db::flag(&conn, "search").await? {
                "Szukanie jest wyłączone.".into()
            } else {
                let q = args.get("q").and_then(|v| v.as_str()).unwrap_or("");
                let found = app.llm.search(q).await.unwrap_or_else(|e| e.to_string());
                app.send_presence(&json!({
                    "type": "widget",
                    "widget": {"type": "note", "title": "Szukanie", "body": found}
                }))
                .await;
                found
            }
        }
        "remember" => {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            file_note(app, text).await?;
            "Zapisałem.".into()
        }
        "shell" | "script" => exec_code(app, name, args).await?,
        "schedule" => schedule_upsert(app, args).await?,
        "move_presence" => {
            let target = args.get("device").and_then(|v| v.as_str()).unwrap_or(device_id);
            *app.pinned.lock().await = false;
            move_whole(app, target, true).await;
            format!("Jestem na {target}.")
        }
        "desktop_mode" => {
            if !db::flag(&conn, "desktop_modes").await? {
                "Tryby pulpitu są wyłączone.".into()
            } else {
                let mode = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let learning = mode.to_lowercase().contains("nauk");
                *app.study.lock().await = learning;
                ask_device(app, "desktop", &json!({"type":"desktop_mode","name": mode})).await;
                if !learning {
                    flush_notices(app).await;
                }
                format!("Ustawiam tryb {mode}.")
            }
        }
        "glance" => glance(app).await?,
        _ => format!("Nie znam narzędzia {name}."),
    };
    say(app, &output).await;
    store_assistant(app, &output).await?;
    app.send_presence(&json!({"type":"done"})).await;
    Ok(())
}

async fn exec_code(app: &App, name: &str, args: &Value) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    let flag_name = if name == "script" { "scripts" } else { "shell" };
    if !db::flag(&conn, flag_name).await? {
        return Ok("Ta funkcja jest wyłączona.".into());
    }
    let target = args.get("target").and_then(|v| v.as_str()).unwrap_or("oci");
    let summary = if name == "script" {
        format!(
            "Skrypt {} na {target}",
            args.get("lang").and_then(|v| v.as_str()).unwrap_or("bash")
        )
    } else {
        format!(
            "Polecenie na {target}: {}",
            args.get("command").and_then(|v| v.as_str()).unwrap_or("")
        )
    };
    let body = if name == "script" {
        args.get("code").and_then(|v| v.as_str()).unwrap_or("").to_string()
    } else {
        args.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string()
    };
    if !confirm(app, &summary, &body).await? {
        return Ok("Anulowane.".into());
    }
    let result = if target == "windows" {
        ask_device(
            app,
            "desktop",
            &json!({"type":"run","lang": args.get("lang"), "command": body}),
        )
        .await
    } else if name == "script" {
        run_temp_script(
            args.get("lang").and_then(|v| v.as_str()).unwrap_or("bash"),
            &body,
        )
        .await
    } else {
        run_shell(&body).await
    };
    db::audit(&conn, name, &summary).await?;
    Ok(result)
}

async fn confirm(app: &App, summary: &str, body: &str) -> anyhow::Result<bool> {
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

async fn run_shell(command: &str) -> String {
    let mut child = Command::new("sh");
    child.arg("-c").arg(command);
    finish(child).await
}

async fn run_temp_script(lang: &str, code: &str) -> String {
    let id = Uuid::new_v4();
    let (file, program, args): (String, &str, Vec<String>) = match lang {
        "python" => (format!("/tmp/jarvis-{id}.py"), "python3", vec![]),
        "powershell" => (format!("/tmp/jarvis-{id}.ps1"), "pwsh", vec!["-File".into()]),
        _ => (format!("/tmp/jarvis-{id}.sh"), "bash", vec![]),
    };
    if std::fs::write(&file, code).is_err() {
        return "Nie mogę zapisać skryptu.".into();
    }
    let mut cmd = Command::new(program);
    cmd.args(args).arg(&file);
    let result = finish(cmd).await;
    let _ = std::fs::remove_file(&file);
    result
}

async fn finish(mut command: Command) -> String {
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

async fn ask_device(app: &App, kind: &str, message: &Value) -> String {
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

pub async fn tool_result(app: &App, id: &str, output: &str) {
    if let Some(tx) = app.results.lock().await.remove(id) {
        let _ = tx.send(output.to_string());
    }
}

async fn schedule_upsert(app: &App, args: &Value) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "schedule").await? {
        return Ok("Harmonogram jest wyłączony.".into());
    }
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("zadanie");
    if args.get("remove").and_then(|v| v.as_bool()).unwrap_or(false) {
        conn.execute("DELETE FROM schedules WHERE name = ?1", params![name])
            .await?;
        return Ok(format!("Usunąłem {name}."));
    }
    let at = args.get("at").and_then(|v| v.as_str()).unwrap_or("08:00");
    let instruction = args
        .get("instruction")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    conn.execute(
        "INSERT INTO schedules (name, at, instruction) VALUES (?1, ?2, ?3)",
        params![name, at, instruction],
    )
    .await?;
    Ok(format!("Zapisałem {name} na {at}."))
}

pub async fn tick_schedules(app: &App) -> anyhow::Result<()> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "schedule").await? {
        return Ok(());
    }
    let now = chrono::Local::now();
    let hm = now.format("%H:%M").to_string();
    let day = now.format("%Y-%m-%d").to_string();
    let mut rows = conn
        .query("SELECT id, name, at, instruction, last_day FROM schedules", ())
        .await?;
    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        let at: String = row.get(2)?;
        let instruction: String = row.get(3)?;
        let last: Option<String> = row.get(4).ok();
        if at == hm && last.as_deref() != Some(day.as_str()) {
            conn.execute(
                "UPDATE schedules SET last_day = ?1 WHERE id = ?2",
                params![day.as_str(), id],
            )
            .await?;
            say(app, &format!("{name}.")).await;
            let _ = handle_text(app, "schedule", &instruction).await;
        }
    }
    Ok(())
}

async fn file_note(app: &App, text: &str) -> anyhow::Result<()> {
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

async fn recall(app: &App, query: &str) -> anyhow::Result<String> {
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

async fn extract_graph(app: &App, text: &str) -> anyhow::Result<()> {
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
    if speech == 1 {
        set_presence(app, device_id).await;
    } else if away == 1 && device_id.contains("desktop") {
        if let Some(phone) = find_kind(app, "android").await {
            move_whole(app, &phone, false).await;
        }
    }
    if db::flag(&conn, "proactive").await? {
        maybe_battery(app, payload).await;
    }
    Ok(())
}

async fn maybe_battery(app: &App, payload: &Value) {
    let Some(level) = payload
        .pointer("/telemetry/battery")
        .and_then(|v| v.as_f64())
    else {
        return;
    };
    if level > 20.0 {
        *app.battery_said.lock().await = false;
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
    say(app, &format!("Bateria ma {level:.0} procent, a niedługo masz sesję.")).await;
}

async fn find_kind(app: &App, kind: &str) -> Option<String> {
    let clients = app.clients.lock().await;
    clients.keys().find(|id| id.contains(kind)).cloned()
}

async fn move_whole(app: &App, device_id: &str, force: bool) {
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

async fn set_presence(app: &App, device_id: &str) {
    if *app.pinned.lock().await {
        return;
    }
    *app.presence.lock().await = Some(device_id.to_string());
}

async fn glance(app: &App) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "vision_screen").await? {
        return Ok("Wzrok jest wyłączony.".into());
    }
    let shot = ask_device(app, "desktop", &json!({"type":"capture"})).await;
    if shot.starts_with("data:") || shot.len() > 100 {
        let clean = shot.trim_start_matches("data:image/png;base64,");
        let text = app
            .llm
            .complete_vision("Opisz po polsku, co jest na ekranie.", clean, "image/png")
            .await
            .unwrap_or_else(|e| e.to_string());
        return Ok(text);
    }
    Ok(shot)
}

pub async fn glance_saved(app: &App, image_b64: &str, mime: &str) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "vision_screen").await? {
        return Ok("Wzrok jest wyłączony.".into());
    }
    let text = app
        .llm
        .complete_vision("Opisz po polsku, co jest na ekranie.", image_b64, mime)
        .await?;
    say(app, &text).await;
    Ok(text)
}

pub async fn save_photo(app: &App, image_b64: &str, mime: &str) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "vision_camera").await? {
        return Ok("Aparat jest wyłączony.".into());
    }
    let text = app
        .llm
        .complete_vision(
            "Opisz obiekt po polsku: tabliczka, układ albo dokument.",
            image_b64,
            mime,
        )
        .await?;
    let dir = vault_path().join("jarvis/wzrok");
    std::fs::create_dir_all(&dir)?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let note = dir.join(format!("{stamp}.md"));
    let image = dir.join(format!("{stamp}.img"));
    if let Ok(bytes) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, image_b64) {
        let _ = std::fs::write(&image, bytes);
    }
    std::fs::write(&note, format!("# Zdjęcie\n\n{text}\n"))?;
    say(app, &text).await;
    Ok(note.display().to_string())
}

async fn say(app: &App, text: &str) {
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

async fn store_assistant(app: &App, text: &str) -> anyhow::Result<()> {
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
        let _ = std::fs::write(&file, completion.text);
        app.quiet("Zostawiłem propozycję poprawki w vaultcie. Nie wdrażam jej.").await;
        app.send_presence(&json!({
            "type": "widget",
            "widget": {"type": "note", "title": "Naprawa", "body": file.display().to_string()}
        }))
        .await;
    }
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

fn seal_store(app: &App, text: &str) -> String {
    match app.identity.seal(&app.identity.public, text.as_bytes()) {
        Ok(bytes) => format!(
            "seal:{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
        ),
        Err(_) => text.to_string(),
    }
}

fn open_store(app: &App, stored: &str) -> String {
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

async fn weather_line(lat: f64, lon: f64) -> String {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&current=temperature_2m"
    );
    let Ok(response) = reqwest::get(url).await else {
        return "Pogoda niedostępna.".into();
    };
    let Ok(value) = response.json::<Value>().await else {
        return "Pogoda niedostępna.".into();
    };
    value["current"]["temperature_2m"]
        .as_f64()
        .map(|c| format!("{c:.0}°C"))
        .unwrap_or_else(|| "Pogoda niedostępna.".into())
}

async fn walk_links(app: &App) -> anyhow::Result<String> {
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

async fn flush_notices(app: &App) {
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
    app.send_presence(&json!({
        "type": "widget",
        "widget": {"type": "note", "title": "Skrót", "body": summary}
    }))
    .await;
    app.quiet(&summary).await;
}
