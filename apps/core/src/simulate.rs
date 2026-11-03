use serde_json::{json, Value};

use crate::agent;
use crate::db;
use crate::selfmap;
use crate::state::App;

const PLANNER: &str = "Jesteś planistą Jarvisa. Nic nie wykonujesz. \
Odpowiedz wyłącznie JSON bez markdown: \
{\"summary\":\"po polsku, zaznacz że to przebieg próbny\",\"steps\":[{\"action\":\"...\",\"side_effect\":false}],\"nodes\":[{\"id\":\"a\",\"label\":\"...\"}]}.";

pub async fn run(app: &App, args: &Value) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "simulation").await? {
        return Ok("Symulacja jest wyłączona.".into());
    }
    let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("dry");
    let goal = args.get("goal").and_then(|v| v.as_str()).unwrap_or("").trim();
    if goal.is_empty() {
        return Ok("Podaj cel symulacji.".into());
    }
    let memory = agent::recall(app, goal).await.unwrap_or_default();
    let prompt = if mode == "whatif" {
        format!(
            "To jest scenariusz co-jeśli. Nie zapisuj pamięci i nie wykonuj narzędzi.\nPamięć tylko do odczytu:\n{memory}\n\nCel: {goal}"
        )
    } else {
        format!("Suchy przebieg, bez wykonania.\nCel: {goal}\nKontekst:\n{memory}")
    };
    let completion = app.llm.complete(PLANNER, &prompt).await?;
    let parsed = parse_loose(&completion.text);
    let summary = parsed
        .as_ref()
        .and_then(|value| value.get("summary").and_then(|v| v.as_str()))
        .unwrap_or(completion.text.trim())
        .to_string();
    let steps = parsed
        .as_ref()
        .and_then(|value| value.get("steps").cloned())
        .unwrap_or_else(|| json!([]));
    let ghost = parsed
        .as_ref()
        .and_then(|value| value.get("nodes").and_then(|v| v.as_array()).cloned())
        .unwrap_or_default();
    let body = if steps.as_array().map(|items| items.is_empty()).unwrap_or(true) {
        format!("Przebieg próbny. {summary}")
    } else {
        format!("Przebieg próbny.\n{summary}\n{steps}")
    };
    let ghost_nodes: Vec<Value> = ghost
        .into_iter()
        .map(|node| {
            json!({
                "id": node.get("id").and_then(|v| v.as_str()).unwrap_or("ghost"),
                "label": node.get("label").and_then(|v| v.as_str()).unwrap_or("symulacja"),
                "group": "ghost"
            })
        })
        .collect();
    let _ = selfmap::publish(app, Some(ghost_nodes)).await;
    app.send_presence(&json!({
        "type": "widget",
        "widget": {"type": "simulation", "title": if mode == "whatif" { "Co jeśli" } else { "Suchy przebieg" }, "body": body}
    }))
    .await;
    Ok(body.chars().take(1200).collect())
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
