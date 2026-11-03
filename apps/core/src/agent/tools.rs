use serde_json::{json, Value};
use tokio::process::Command;
use turso::params;
use uuid::Uuid;

use crate::db;
use crate::state::App;

use super::chat::{handle_text};
use super::common::{ask_device, confirm, finish, say, store_assistant, vault_path};
use super::memory::{file_note};
use super::presence::{flush_notices, move_whole};

pub(crate) async fn execute(app: &App, device_id: &str, name: &str, args: &Value) -> anyhow::Result<String> {
    run_tool_inner(app, device_id, name, args).await
}

pub(crate) async fn run_tool(app: &App, device_id: &str, name: &str, args: &Value) -> anyhow::Result<()> {
    let job = super::jobs::begin(app, name, device_id, "").await.ok();
    let result = run_tool_inner(app, device_id, name, args).await;
    if let Some(id) = job {
        let (status, detail) = match &result {
            Ok(text) if text.starts_with("Anulowane") || text.contains("jest wyłączon") => ("failed", text.clone()),
            Ok(text) => ("done", text.clone()),
            Err(err) => ("failed", err.to_string()),
        };
        let _ = super::jobs::close(app, &id, status, &detail).await;
    }
    let output = result?;
    say(app, &output).await;
    store_assistant(app, &output).await?;
    app.send_presence(&json!({"type":"done"})).await;
    Ok(())
}

async fn run_tool_inner(app: &App, device_id: &str, name: &str, args: &Value) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if crate::plugins::list().iter().any(|plugin| plugin.id == name) {
        return crate::plugins::execute(app, name, args).await;
    }
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
        "self_map" => crate::selfmap::publish(app, None).await?,
        "simulate" => crate::simulate::run(app, args).await?,
        "evolve" => crate::evolve::run(app, args).await?,
        "status" => crate::briefing::report(app, false).await?,
        "mission" => {
            let goal = args.get("goal").and_then(|v| v.as_str()).unwrap_or("");
            crate::mission::start(app, goal).await?
        }
        _ => crate::evolve::from_unknown(app, name, args).await?,
    };
    Ok(output)
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

pub(crate) async fn glance(app: &App) -> anyhow::Result<String> {
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

pub(crate) async fn weather_line(lat: f64, lon: f64) -> String {
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
