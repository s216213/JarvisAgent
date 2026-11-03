use chrono::Timelike;
use serde_json::json;

use crate::agent::{open_store, weather_line};
use crate::db;
use crate::state::App;

pub async fn report(app: &App, greet: bool) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "briefing").await? {
        return Ok("Raport jest wyłączony.".into());
    }
    let persona = db::flag(&conn, "persona").await.unwrap_or(true);
    let text = compose(app, &conn, greet, persona).await?;
    app.send_presence(&json!({
        "type": "widget",
        "widget": {"type": "brief", "title": "Raport", "body": text}
    }))
    .await;
    Ok(text)
}

async fn compose(
    app: &App,
    conn: &turso::Connection,
    greet: bool,
    persona: bool,
) -> anyhow::Result<String> {
    let pulse = app.pulse.lock().await.clone();
    let digest = app.last_digest.lock().await.clone();
    let (lat, lon) = app.desk.lock().await.unwrap_or((52.23, 21.01));
    let weather = weather_line(lat, lon).await;
    let schedules = schedules(conn).await?;
    let memory = latest_fact(app, conn).await?;
    let opening = opening(greet, persona);
    let weather_line = if weather.contains('°') {
        format!("Przy biurku jest {weather}")
    } else {
        weather
    };
    let agenda = if digest.trim().is_empty() {
        schedules
    } else {
        format!("{schedules} Z powiadomień: {}", one_line(&digest))
    };
    let mut parts = vec![
        sentence(&opening),
        sentence(&weather_line),
        sentence(&machine_line(&pulse)),
        sentence(&agenda),
    ];
    if let Some(fact) = memory {
        parts.push(sentence(&format!("Pamiętam: {}", one_line(&fact))));
    }
    Ok(parts.join(" "))
}

fn opening(greet: bool, persona: bool) -> String {
    let hour = chrono::Local::now().hour();
    let hello = match hour {
        0..=11 => "Dzień dobry",
        12..=17 => "Witam",
        _ => "Dobry wieczór",
    };
    if greet {
        if persona {
            format!("{hello}, panie")
        } else {
            hello.into()
        }
    } else if persona {
        "Panie, stan na teraz".into()
    } else {
        "Stan na teraz".into()
    }
}

fn machine_line(pulse: &crate::state::Pulse) -> String {
    let cpu = pulse
        .cpu
        .map(|value| format!("{value:.0} procent"))
        .unwrap_or_else(|| "nieznany".into());
    let ram = pulse
        .ram
        .map(|value| format!("{value:.0} procent"))
        .unwrap_or_else(|| "nieznana".into());
    let battery = pulse
        .battery
        .map(|value| format!("{value:.0} procent"))
        .unwrap_or_else(|| "nieznana".into());
    let network = if pulse.network.trim().is_empty() {
        "nieznana"
    } else {
        pulse.network.as_str()
    };
    format!("Procesor {cpu}, pamięć {ram}, bateria {battery}, sieć {network}")
}

async fn schedules(conn: &turso::Connection) -> anyhow::Result<String> {
    let mut rows = conn
        .query("SELECT name, at FROM schedules ORDER BY at LIMIT 3", ())
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        let name: String = row.get(0)?;
        let at: String = row.get(1)?;
        items.push(format!("{name} o {at}"));
    }
    if items.is_empty() {
        Ok("Harmonogram jest pusty".into())
    } else {
        Ok(format!("W harmonogramie: {}", items.join(", ")))
    }
}

async fn latest_fact(app: &App, conn: &turso::Connection) -> anyhow::Result<Option<String>> {
    let mut rows = conn
        .query("SELECT body FROM facts ORDER BY id DESC LIMIT 1", ())
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let stored: String = row.get(0)?;
    let plain = open_store(app, &stored);
    let plain = plain.trim();
    if plain.is_empty() {
        Ok(None)
    } else {
        Ok(Some(plain.to_string()))
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sentence(text: &str) -> String {
    let trimmed = text.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        return String::new();
    }
    format!("{trimmed}.")
}
