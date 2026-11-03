use serde_json::{json, Value};
use turso::params;

use crate::db;
use crate::state::App;

use super::common::{say, seal_store, store_assistant};
use super::memory::{extract_graph, file_note, recall, walk_links};
use super::presence::{move_whole, set_presence};
use super::tools::{run_tool};

const TOOLS: &str = "Narzędzia: search (q), map (lat, lon, title), remember (text), shell (target oci|windows, command), \
script (target oci|windows, lang powershell|bash|python, code), \
schedule (name, at HH:MM, instruction, remove bool), \
move_presence (device), desktop_mode (name), \
glance (nic, gdy chcesz spojrzeć na ekran), \
self_map (nic, mapa ciebie i historia), \
simulate (mode dry|whatif, goal), \
evolve (goal, kind plugin|core|web|desktop|mobile, id, description, capabilities, lang, code, diff, files), \
status (nic, raport sytuacyjny), mission (goal). \
Pluginy z listy poniżej wołasz po ich id. \
Shell, script oraz pluginy z siecią, plikami albo powłoką zawsze wymagają zgody, nie omijaj jej. \
simulate niczego nie wykonuje. evolve nie wyłącza zgód i nie wypycha main. \
status niczego nie zmienia. mission idzie w tle, a powłoka w jej kroku i tak czeka na zgodę.";

const PLAIN: &str = "Jesteś Jarvisem, osobistym asystentem. Mówisz po polsku, krótko i konkretnie. \
Gdy potrzebujesz narzędzia, odpowiedz wyłącznie JSON bez markdown: \
{\"say\":\"krótko co robisz\",\"tool\":{\"name\":\"nazwa\",\"args\":{}}}. \
Gdy nie potrzebujesz narzędzia, odpowiedz zwykłym tekstem.";

const BUTLER: &str = "Jesteś Jarvisem, osobistym asystentem. Mówisz po polsku, spokojnie i krótko, zwracasz się „panie”. \
Suchy humor jest dopuszczalny, bez cytatów z filmu. \
Gdy potrzebujesz narzędzia, odpowiedz wyłącznie JSON bez markdown: \
{\"say\":\"jedno zdanie potwierdzenia, na przykład Sprawdzam pogodę.\",\"tool\":{\"name\":\"nazwa\",\"args\":{}}}. \
Gdy nie potrzebujesz narzędzia, odpowiedz zwykłym tekstem.";

async fn system_prompt(app: &App) -> String {
    let persona = match app.db.connect() {
        Ok(conn) => db::flag(&conn, "persona").await.unwrap_or(true),
        Err(_) => true,
    };
    let tone = if persona { BUTLER } else { PLAIN };
    format!("{tone}\n{TOOLS}\n\nPluginy:\n{}", crate::plugins::prompt_lines())
}

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
    if let Some(done) = crate::evolve::command(app, text).await? {
        say(app, &done).await;
        app.send_presence(&json!({"type":"done"})).await;
        return Ok(());
    }
    let phrase = bare(text);
    if matches!(phrase.as_str(), "anuluj misję" | "anuluj misje") {
        set_presence(app, device_id).await;
        let line = if crate::mission::cancel(app).await {
            "Przerywam misję."
        } else {
            "Nie mam misji."
        };
        say(app, line).await;
        app.send_presence(&json!({"type":"done"})).await;
        return Ok(());
    }
    if phrase == "misja" {
        set_presence(app, device_id).await;
        let line = crate::mission::start(app, "").await?;
        say(app, &line).await;
        app.send_presence(&json!({"type":"done"})).await;
        return Ok(());
    }
    if let Some(greet) = status_cue(&phrase) {
        set_presence(app, device_id).await;
        let line = crate::briefing::report(app, greet).await?;
        say(app, &line).await;
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
    let brief = crate::selfmap::brief(app).await.unwrap_or_else(|err| err.to_string());
    let prompt = format!("Kim jestem:\n{brief}\n\nPamięć i notatki:\n{memory}\n\nUżytkownik: {text}");
    let completion = match app.llm.complete(&system_prompt(app).await, &prompt).await {
        Ok(c) => c,
        Err(err) => {
            say(app, &format!("Nie mogę teraz odpowiedzieć. {err}")).await;
            return Ok(());
        }
    };
    db::audit(&conn, "llm", &completion.provider).await?;
    let simulated = requested_tool(&completion.text).as_deref() == Some("simulate");
    dispatch(app, device_id, &completion.text).await?;
    if !simulated {
        let _ = extract_graph(app, text).await;
    }
    Ok(())
}

fn bare(text: &str) -> String {
    text.trim()
        .trim_matches(|ch: char| matches!(ch, '.' | '!' | '?' | ','))
        .to_lowercase()
}

fn status_cue(phrase: &str) -> Option<bool> {
    match phrase {
        "status" | "raport" | "co słychać" | "co slychac" => Some(false),
        "dzień dobry" | "dzien dobry" => Some(true),
        _ => None,
    }
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

fn requested_tool(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_start_matches("```json").trim_end_matches("```").trim();
    let value: Value = serde_json::from_str(trimmed).ok()?;
    Some(value.get("tool")?.get("name")?.as_str()?.to_string())
}
