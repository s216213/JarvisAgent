use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tokio::process::Command;

use crate::agent;
use crate::db;
use crate::selfmap;
use crate::state::App;

#[derive(Clone, Debug)]
pub struct Plugin {
    pub id: String,
    pub description: String,
    pub args: Value,
    pub capabilities: Vec<String>,
    pub trust: String,
    pub enabled: bool,
    pub lang: String,
    pub file: String,
}

const BUILTIN: &[&str] = &[
    "search",
    "map",
    "remember",
    "shell",
    "script",
    "schedule",
    "move_presence",
    "desktop_mode",
    "glance",
    "self_map",
    "simulate",
    "evolve",
    "status",
    "mission",
];

pub fn root() -> PathBuf {
    let data = std::env::var("JARVIS_DATA").unwrap_or_else(|_| "data".into());
    PathBuf::from(data).join("plugins")
}

pub fn list() -> Vec<Plugin> {
    let dir = root();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join("plugin.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(plugin) = parse(&value) {
            if !BUILTIN.contains(&plugin.id.as_str()) {
                out.push(plugin);
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn find(name: &str) -> Option<Plugin> {
    list().into_iter().find(|plugin| plugin.id == name && plugin.enabled)
}

pub fn prompt_lines() -> String {
    let plugins = list()
        .into_iter()
        .filter(|plugin| plugin.enabled)
        .map(|plugin| {
            format!(
                "- {} ({} {}) : {}",
                plugin.id,
                plugin.capabilities.join(", "),
                plugin.args,
                plugin.description
            )
        })
        .collect::<Vec<_>>();
    if plugins.is_empty() {
        "(brak)".into()
    } else {
        plugins.join("\n")
    }
}

pub fn needs_confirm(capabilities: &[String], trust: &str, code: &str) -> bool {
    let declared = capabilities.iter().any(|item| matches!(item.as_str(), "net" | "files" | "shell"));
    let trust_auto = trust == "auto";
    declared || !trust_auto || code_looks_privileged(code)
}

pub fn code_looks_privileged(code: &str) -> bool {
    let lower = code.to_lowercase();
    const NEEDLES: &[&str] = &[
        "subprocess",
        "os.system",
        "os.popen",
        "socket",
        "urllib",
        "requests",
        "http.client",
        "open(",
        "pathlib",
        "shutil",
        "curl ",
        "wget ",
        "remove-item",
        "invoke-webrequest",
        "set-content",
        "out-file",
    ];
    NEEDLES.iter().any(|needle| lower.contains(needle))
}

pub async fn execute(app: &App, name: &str, args: &Value) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "plugins").await? {
        return Ok("Pluginy są wyłączone.".into());
    }
    let Some(plugin) = list().into_iter().find(|plugin| plugin.id == name) else {
        return Ok(format!("Nie znam narzędzia {name}."));
    };
    if find(name).is_none() {
        return Ok(format!("Plugin {name} jest wyłączony."));
    }
    let code = std::fs::read_to_string(root().join(&plugin.id).join(&plugin.file)).unwrap_or_default();
    if needs_confirm(&plugin.capabilities, &plugin.trust, &code) {
        let summary = format!("Plugin {}: {}", plugin.id, plugin.description);
        let body = clip(&code, 3500);
        if !agent::confirm(app, &summary, &body).await? {
            return Ok("Anulowane.".into());
        }
    }
    let (program, prefix): (&str, Vec<String>) = match plugin.lang.as_str() {
        "python" => ("python3", Vec::new()),
        "powershell" => ("pwsh", vec!["-File".into()]),
        _ => ("bash", Vec::new()),
    };
    let script = root().join(&plugin.id).join(&plugin.file);
    let mut cmd = Command::new(program);
    cmd.args(prefix)
        .arg(&script)
        .current_dir(root().join(&plugin.id))
        .env("JARVIS_ARGS", args.to_string());
    let result = agent::finish(cmd).await;
    let _ = db::audit(&conn, "plugin", &plugin.id).await;
    Ok(result)
}

pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    let path = manifest_path(id)?;
    let mut value: Value = read_manifest(&path)?;
    value["enabled"] = json!(enabled);
    std::fs::write(&path, serde_json::to_string_pretty(&value).map_err(|err| err.to_string())?)
        .map_err(|err| err.to_string())?;
    Ok(())
}

pub fn remove(id: &str) -> Result<(), String> {
    let id = safe_id(id)?;
    let dir = root().join(&id);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|err| err.to_string())?;
    }
    Ok(())
}

pub async fn install(app: &App, spec: &Value, code: &str) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "plugins").await? {
        return Ok("Pluginy są wyłączone.".into());
    }
    let id = safe_id(spec.get("id").and_then(|v| v.as_str()).unwrap_or(""))
        .map_err(|err| anyhow::anyhow!(err))?;
    if BUILTIN.contains(&id.as_str()) {
        return Ok("Ta nazwa jest zajęta przez wbudowane narzędzie.".into());
    }
    let description = spec
        .get("description")
        .and_then(|v| v.as_str())
        .unwrap_or("Plugin")
        .to_string();
    let mut capabilities = spec
        .get("capabilities")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec!["read".into()]);
    if capabilities.is_empty() {
        capabilities.push("read".into());
    }
    let lang = spec.get("lang").and_then(|v| v.as_str()).unwrap_or("python");
    let lang = if matches!(lang, "python" | "bash" | "powershell") {
        lang
    } else {
        "python"
    };
    if needs_confirm(&capabilities, "auto", code) {
        let summary = format!("Włączyć plugin {id}? {description}");
        if !agent::confirm(app, &summary, &clip(code, 3500)).await? {
            selfmap::record(app, "plugin", "odrzucono", &id, &description).await?;
            return Ok("Anulowane.".into());
        }
    }
    let dir = root().join(&id);
    std::fs::create_dir_all(&dir)?;
    let file = match lang {
        "bash" => "main.sh",
        "powershell" => "main.ps1",
        _ => "main.py",
    };
    std::fs::write(dir.join(file), code)?;
    let trust = if needs_confirm(&capabilities, "auto", code) {
        "ask"
    } else {
        "auto"
    };
    let manifest = json!({
        "id": id,
        "description": description,
        "args": spec.get("args").cloned().unwrap_or_else(|| json!({"input": "string"})),
        "capabilities": capabilities,
        "trust": trust,
        "enabled": true,
        "entry": {"kind": "script", "lang": lang, "file": file}
    });
    std::fs::write(dir.join("plugin.json"), serde_json::to_string_pretty(&manifest)?)?;
    selfmap::record(app, "plugin", if trust == "auto" { "auto" } else { "zastosowano" }, &id, &description).await?;
    let _ = db::audit(&conn, "plugin-install", &id).await;
    crate::board::publish(app).await;
    Ok(format!("Plugin {id} jest włączony."))
}

pub fn safe_id(raw: &str) -> Result<String, String> {
    let id = raw.trim().to_lowercase().replace(' ', "-");
    if !(2..=40).contains(&id.len()) || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
        return Err("Nazwa pluginu może mieć 2–40 znaków: litery, cyfry, - i _.".into());
    }
    Ok(id)
}

fn parse(value: &Value) -> Option<Plugin> {
    let id = value.get("id")?.as_str()?.to_string();
    if safe_id(&id).is_err() {
        return None;
    }
    let entry = value.get("entry")?;
    Some(Plugin {
        id,
        description: value.get("description").and_then(|v| v.as_str()).unwrap_or("").into(),
        args: value.get("args").cloned().unwrap_or_else(|| json!({})),
        capabilities: value
            .get("capabilities")
            .and_then(|v| v.as_array())
            .map(|items| items.iter().filter_map(|item| item.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default(),
        trust: value.get("trust").and_then(|v| v.as_str()).unwrap_or("ask").into(),
        enabled: value.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        lang: entry.get("lang").and_then(|v| v.as_str()).unwrap_or("python").into(),
        file: entry.get("file").and_then(|v| v.as_str()).unwrap_or("main.py").into(),
    })
}

fn manifest_path(id: &str) -> Result<PathBuf, String> {
    let id = safe_id(id)?;
    let path = root().join(&id).join("plugin.json");
    if !path.is_file() {
        return Err("Nie ma takiego pluginu.".into());
    }
    Ok(path)
}

fn read_manifest(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

fn clip(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_trust_rejects_network_and_open() {
        assert!(!needs_confirm(&["read".into()], "auto", "print(1+1)"));
        assert!(needs_confirm(&["net".into()], "auto", "print(1)"));
        assert!(needs_confirm(&["read".into()], "ask", "print(1)"));
        assert!(needs_confirm(&["read".into()], "auto", "open('x')"));
    }

    #[test]
    fn plugin_id_is_narrow() {
        assert!(safe_id("Pogoda Dnia").is_ok());
        assert!(safe_id("../x").is_err());
        assert!(safe_id("a").is_err());
    }
}
