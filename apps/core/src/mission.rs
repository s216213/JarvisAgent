use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{execute, say};
use crate::db;
use crate::state::App;

const PLANNER: &str = "Jesteś planistą Jarvisa. Nic nie wykonujesz. \
Odpowiedz wyłącznie JSON bez markdown: \
{\"steps\":[{\"tool\":\"search\",\"args\":{\"q\":\"\"},\"label\":\"krótko po polsku\"}]}. \
Dozwolone tool: search, map, glance, status, self_map, shell, script, evolve. \
search ma args.q. map ma lat, lon, title. shell ma target i command. script ma target, lang, code. \
status, glance i self_map mają puste args. evolve ma goal. Najwyżej 4 kroki.";

const ALLOWED: &[&str] = &[
    "search", "map", "glance", "status", "self_map", "shell", "script", "evolve",
];

struct Step {
    tool: String,
    args: Value,
    label: String,
}

pub async fn start(app: &App, goal: &str) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "missions").await? {
        return Ok("Misje są wyłączone.".into());
    }
    let goal = goal.trim();
    if goal.is_empty() {
        return Ok("Podaj cel misji.".into());
    }
    cancel(app).await;
    let flag = Arc::new(AtomicBool::new(false));
    *app.mission_cancel.lock().await = Some(flag.clone());
    let app = app.clone();
    let goal = goal.to_string();
    tokio::spawn(async move {
        drive(app, goal, flag).await;
    });
    Ok("Zaczynam.".into())
}

pub async fn cancel(app: &App) -> bool {
    let flag = app.mission_cancel.lock().await.clone();
    let Some(flag) = flag else {
        return false;
    };
    flag.store(true, Ordering::Relaxed);
    let pending: Vec<_> = app.confirms.lock().await.drain().map(|(_, tx)| tx).collect();
    for tx in pending {
        let _ = tx.send(false);
    }
    true
}

async fn drive(app: App, goal: String, flag: Arc<AtomicBool>) {
    let steps = match plan(&app, &goal).await {
        Ok(steps) => steps,
        Err(err) => {
            say(&app, &format!("Nie ułożyłem kroków. {err}")).await;
            finish(&app, &flag).await;
            return;
        }
    };
    if flag.load(Ordering::Relaxed) {
        finish(&app, &flag).await;
        return;
    }
    if steps.is_empty() {
        say(&app, "Nie układam pustej misji.").await;
        finish(&app, &flag).await;
        return;
    }
    let total = steps.len();
    for (index, step) in steps.iter().enumerate() {
        if flag.load(Ordering::Relaxed) {
            say(&app, "Przerywam misję.").await;
            break;
        }
        let label = format!("krok {} z {total}. {}", index + 1, step.label);
        app.set_mission(&label).await;
        say(&app, &format!("Krok {} z {total}. {}.", index + 1, step.label)).await;
        if !ALLOWED.contains(&step.tool.as_str()) {
            say(&app, "Pomijam ten krok.").await;
            continue;
        }
        let result = execute(&app, "mission", &step.tool, &step.args).await;
        if flag.load(Ordering::Relaxed) {
            say(&app, "Przerywam misję.").await;
            break;
        }
        match result {
            Ok(text) => {
                let short: String = text.chars().take(180).collect();
                if !short.trim().is_empty() {
                    say(&app, &short).await;
                }
            }
            Err(err) => say(&app, &format!("Krok stanął. {err}")).await,
        }
    }
    if !flag.load(Ordering::Relaxed) {
        say(&app, "Misja skończona.").await;
    }
    finish(&app, &flag).await;
}

async fn finish(app: &App, flag: &Arc<AtomicBool>) {
    let current = app.mission_cancel.lock().await.clone();
    if current.as_ref().is_some_and(|item| Arc::ptr_eq(item, flag)) {
        *app.mission_cancel.lock().await = None;
        app.set_mission("").await;
    }
    app.send_presence(&json!({"type":"done"})).await;
}

async fn plan(app: &App, goal: &str) -> anyhow::Result<Vec<Step>> {
    let completion = app.llm.complete(PLANNER, goal).await?;
    let parsed = parse_loose(&completion.text).unwrap_or(json!({}));
    let raw = parsed.get("steps").and_then(|value| value.as_array()).cloned().unwrap_or_default();
    let mut steps = Vec::new();
    for item in raw.into_iter().take(4) {
        let tool = item
            .get("tool")
            .and_then(|value| value.as_str())
            .or_else(|| item.get("action").and_then(|value| value.as_str()))
            .unwrap_or("")
            .to_string();
        if tool.is_empty() {
            continue;
        }
        let args = item.get("args").cloned().unwrap_or_else(|| json!({}));
        let label = item
            .get("label")
            .and_then(|value| value.as_str())
            .unwrap_or(tool.as_str())
            .chars()
            .take(80)
            .collect();
        steps.push(Step { tool, args, label });
    }
    Ok(steps)
}

fn parse_loose(raw: &str) -> Option<Value> {
    let trimmed = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    serde_json::from_str(trimmed).ok().or_else(|| {
        let start = trimmed.find('{')?;
        let end = trimmed.rfind('}')?;
        serde_json::from_str(&trimmed[start..=end]).ok()
    })
}
