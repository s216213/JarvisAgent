use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tokio::process::Command;

use crate::agent;
use crate::db;
use crate::plugins;
use crate::selfmap;
use crate::state::App;

const DRAFT: &str = "Układasz propozycję dla Jarvisa. Zwróć wyłącznie JSON bez markdown. \
Nowy tool: {\"kind\":\"plugin\",\"id\":\"nazwa\",\"description\":\"...\",\"capabilities\":[\"read\"],\"lang\":\"python\",\"code\":\"...\"}. \
capabilities: read, compute, net, files, shell. \
Zmiana źródeł: {\"kind\":\"core|web|desktop|mobile\",\"id\":\"krotka-nazwa\",\"summary\":\"...\",\"files\":[{\"path\":\"apps/core/src/przyklad.rs\",\"content\":\"...\"}]}. \
Możesz podać diff zamiast files. Nie dotykaj .env, kluczy ani flag zgody. \
Języki: Rust w apps/core, Vue i TypeScript w apps/web, C# w apps/desktop, Kotlin w apps/mobile.";

pub async fn command(app: &App, text: &str) -> anyhow::Result<Option<String>> {
    let lower = text
        .trim()
        .trim_matches(|ch: char| matches!(ch, '.' | '!' | '?' | ','))
        .to_lowercase();
    let action = match lower.as_str() {
        "zastosuj na oci" | "zastosuj na serwerze" => ("apply", "oci"),
        "zastosuj na komputerze" | "zastosuj lokalnie" | "zastosuj na pc" => ("apply", "pc"),
        "zastosuj na obu" | "zastosuj wszedzie" | "zastosuj wszędzie" => ("apply", "both"),
        "zbuduj" | "zbuduj na oci" => ("build", ""),
        "wypchnij" | "wypchnij galaz" | "wypchnij gałąź" => ("push", ""),
        _ => return Ok(None),
    };
    let Some(proposal) = latest()? else {
        return Ok(Some("Nie mam propozycji do zastosowania.".into()));
    };
    let id = proposal["id"].as_str().unwrap_or("");
    let result = match action.0 {
        "apply" => apply(app, id, action.1).await?,
        "build" => build(app, id).await?,
        "push" => push(app, id).await?,
        _ => "Nie rozumiem.".into(),
    };
    Ok(Some(result))
}

pub async fn run(app: &App, args: &Value) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "evolution").await? {
        return Ok("Ewolucja jest wyłączona.".into());
    }
    let spec = if args.get("code").is_some() || args.get("diff").is_some() || args.get("files").is_some() {
        args.clone()
    } else {
        let goal = args.get("goal").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if goal.trim().is_empty() {
            return Ok("Powiedz, czego brakuje.".into());
        }
        let completion = app.llm.complete(DRAFT, &goal).await?;
        parse_loose(&completion.text).unwrap_or(json!({"summary": completion.text}))
    };
    accept(app, &spec).await
}

pub async fn from_unknown(app: &App, name: &str, args: &Value) -> anyhow::Result<String> {
    let conn = app.db.connect()?;
    if !db::flag(&conn, "evolution").await? {
        return Ok(format!("Nie znam narzędzia {name}."));
    }
    let goal = format!("Brakuje narzędzia {name}. Argumenty: {args}. Złóż plugin albo, gdy trzeba zmienić rdzeń, pliki źródłowe.");
    let completion = app.llm.complete(DRAFT, &goal).await?;
    let mut spec = parse_loose(&completion.text).unwrap_or(json!({}));
    if spec.get("id").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
        spec["id"] = json!(name);
    }
    if spec.get("kind").is_none() {
        spec["kind"] = json!("plugin");
    }
    accept(app, &spec).await
}

async fn accept(app: &App, spec: &Value) -> anyhow::Result<String> {
    if let Some(name) = spec.get("flag").and_then(|value| value.as_str()) {
        if db::protected_flag(name) {
            return Ok("Nie wyłączam flag zgody ani ewolucji.".into());
        }
    }
    let kind = spec.get("kind").and_then(|v| v.as_str()).unwrap_or("plugin");
    if kind == "plugin" {
        let code = spec.get("code").and_then(|v| v.as_str()).unwrap_or("");
        if code.trim().is_empty() {
            return Ok("Propozycja pluginu nie ma kodu.".into());
        }
        return plugins::install(app, spec, code).await;
    }
    let id = plugins::safe_id(spec.get("id").and_then(|v| v.as_str()).unwrap_or(kind)).unwrap_or_else(|_| "propozycja".into());
    let summary = spec
        .get("summary")
        .and_then(|v| v.as_str())
        .or_else(|| spec.get("description").and_then(|v| v.as_str()))
        .unwrap_or("Zmiana źródeł")
        .to_string();
    let diff = spec.get("diff").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let files = spec.get("files").cloned().unwrap_or_else(|| json!([]));
    if diff.trim().is_empty() && files.as_array().map(|items| items.is_empty()).unwrap_or(true) {
        return Ok("Propozycja rdzenia nie ma diffa ani plików.".into());
    }
    if let Err(err) = check_paths(&diff, &files) {
        return Ok(err);
    }
    let preview = if diff.trim().is_empty() {
        files.to_string()
    } else {
        diff.clone()
    };
    let card = format!("{summary}\n\n{}", clip(&preview, 3500));
    if !agent::confirm(app, &format!("Zapisać propozycję {id}?"), &card).await? {
        selfmap::record(app, kind, "odrzucono", &summary, &id).await?;
        return Ok("Anulowane.".into());
    }
    let stored = json!({
        "id": id,
        "kind": kind,
        "summary": summary,
        "diff": diff,
        "files": files,
        "status": "czeka",
        "branch": format!("jarvis/{id}")
    });
    write_proposal(&id, &stored)?;
    selfmap::record(app, kind, "czeka", &summary, &id).await?;
    app.send_presence(&json!({
        "type": "widget",
        "widget": {
            "type": "proposal",
            "title": format!("Propozycja {id}"),
            "body": format!("{summary}\nPowiedz: zastosuj na oci, zastosuj na komputerze albo zastosuj na obu. Potem: zbuduj. Wypchnięcie: wypchnij."),
            "id": id
        }
    }))
    .await;
    Ok(format!("Propozycja {id} czeka. Mogę ją położyć na OCI albo na komputerze."))
}

pub async fn apply(app: &App, id: &str, target: &str) -> anyhow::Result<String> {
    let mut proposal = read_proposal(id)?;
    if proposal["status"].as_str() == Some("odrzucono") {
        return Ok("Ta propozycja jest odrzucona.".into());
    }
    let mut notes = Vec::new();
    if target == "oci" || target == "both" {
        match apply_repo(&proposal).await {
            Ok(note) => {
                notes.push(note);
                proposal["status"] = json!("zastosowano");
            }
            Err(err) => return Ok(err.to_string()),
        }
    }
    if target == "pc" || target == "both" {
        let payload = json!({
            "type": "apply_patch",
            "summary": proposal["summary"],
            "body": proposal.to_string()
        });
        let job = agent::begin_job(app, "apply_patch", "desktop-pc", id).await.ok();
        let note = agent::ask_device(app, "desktop", &payload).await;
        if let Some(job) = job {
            let status = if note.starts_with("Nałożyłem") { "done" } else { "failed" };
            let _ = agent::close_job(app, &job, status, &note).await;
        }
        notes.push(note);
    }
    write_proposal(id, &proposal)?;
    let summary = proposal["summary"].as_str().unwrap_or(id).to_string();
    selfmap::record(app, "core", "zastosowano", &summary, target).await?;
    Ok(notes.join("\n"))
}

pub async fn build(app: &App, id: &str) -> anyhow::Result<String> {
    let proposal = read_proposal(id)?;
    if proposal["status"].as_str() != Some("zastosowano") {
        return Ok("Najpierw zastosuj propozycję na OCI.".into());
    }
    let repo = repo_path();
    if !repo.join(".git").exists() {
        return Ok(format!("Nie widzę repozytorium w {}.", repo.display()));
    }
    let image = own_image().await;
    let script = "docker-compose build core caddy && docker-compose up -d --no-build core && docker-compose up -d --no-build --force-recreate caddy";
    let started = if let Some(image) = image {
        let _ = Command::new("docker").args(["rm", "-f", "jarvis-rebuild"]).status().await;
        let repo_text = repo.display().to_string();
        let mount = format!("{repo_text}:{repo_text}");
        Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                "jarvis-rebuild",
                "--rm",
                "-v",
                "/var/run/docker.sock:/var/run/docker.sock",
                "-v",
                &mount,
                "-w",
                &repo_text,
                "--entrypoint",
                "bash",
                &image,
                "-lc",
                script,
            ])
            .status()
            .await
            .map(|status| status.success())
            .unwrap_or(false)
    } else {
        false
    };
    if !started {
        let status = Command::new("docker-compose")
            .current_dir(&repo)
            .args(["build", "core", "caddy"])
            .status()
            .await;
        return match status {
            Ok(code) if code.success() => Ok("Zbudowałem rdzeń i stronę. Podniesienie stosu zrób przez deploy/oci-up.sh, jeśli proces ma zostać.".into()),
            Ok(code) => Ok(format!("Budowa skończyła się kodem {}.", code.code().unwrap_or(-1))),
            Err(err) => Ok(format!("Nie mogę zbudować na OCI: {err}")),
        };
    }
    let _ = app;
    selfmap::record(app, "core", "zastosowano", "Budowa na OCI", id).await?;
    Ok("Buduję rdzeń i stronę na OCI. Nowy proces wstanie po zakończeniu budowy.".into())
}

pub async fn push(app: &App, id: &str) -> anyhow::Result<String> {
    let proposal = read_proposal(id)?;
    let branch = proposal["branch"].as_str().unwrap_or("").to_string();
    if !branch.starts_with("jarvis/") {
        return Ok("Nie wypycham tej gałęzi.".into());
    }
    if !agent::confirm(app, &format!("Wypchnąć {branch}?"), "Bez force. main zostaje.").await? {
        return Ok("Anulowane.".into());
    }
    let repo = repo_path();
    let output = Command::new("git")
        .args(["-C", &repo.display().to_string(), "push", "-u", "origin", &branch])
        .output()
        .await;
    match output {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).to_string();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            if text.to_lowercase().contains("forced") {
                return Ok("Odrzucam wypchnięcie.".into());
            }
            selfmap::record(app, "core", "zastosowano", &format!("Wypchnięto {branch}"), &text).await?;
            Ok(clip(&text, 1200))
        }
        Err(err) => Ok(format!("Nie mogę wypchnąć: {err}")),
    }
}

pub fn list_proposals() -> Vec<Value> {
    let dir = selfmap::proposals_dir();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                out.push(json!({
                    "id": value["id"],
                    "kind": value["kind"],
                    "summary": value["summary"],
                    "status": value["status"],
                    "branch": value["branch"]
                }));
            }
        }
    }
    out
}

async fn apply_repo(proposal: &Value) -> anyhow::Result<String> {
    let repo = repo_path();
    if !repo.join(".git").exists() {
        return Ok(format!(
            "Na OCI nie widzę gita w {}. Ustaw JARVIS_REPO i zamontuj klon.",
            repo.display()
        ));
    }
    let dirty = git(&repo, &["status", "--porcelain"]).await?;
    if !dirty.trim().is_empty() {
        return Ok("Klon na OCI ma lokalne zmiany. Nie dokładam propozycji na brudne drzewo.".into());
    }
    let branch = proposal["branch"].as_str().unwrap_or("jarvis/propozycja");
    if !branch.starts_with("jarvis/") || branch.contains("..") {
        return Ok("Zła nazwa gałęzi.".into());
    }
    git(&repo, &["checkout", "-B", branch]).await?;
    let files = proposal.get("files").cloned().unwrap_or_else(|| json!([]));
    let diff = proposal.get("diff").and_then(|v| v.as_str()).unwrap_or("");
    let mut paths = Vec::new();
    if let Some(items) = files.as_array() {
        for item in items {
            let rel = item.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = item.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if !allowed_path(rel) {
                return Ok(format!("Odrzucam ścieżkę {rel}."));
            }
            let full = repo.join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full, content)?;
            paths.push(rel.to_string());
        }
    }
    if !diff.trim().is_empty() {
        for rel in diff_paths(diff) {
            if !allowed_path(&rel) {
                return Ok(format!("Odrzucam ścieżkę {rel}."));
            }
            paths.push(rel);
        }
        let file = repo.join(".jarvis-proposal.diff");
        std::fs::write(&file, diff)?;
        let path = file.display().to_string();
        let applied = git(&repo, &["apply", "--whitespace=nowarn", &path]).await;
        let _ = std::fs::remove_file(&file);
        applied?;
    }
    if paths.is_empty() {
        return Ok("Propozycja nie ma plików do zapisu.".into());
    }
    for path in &paths {
        git(&repo, &["add", "--", path]).await?;
    }
    let summary = proposal["summary"].as_str().unwrap_or("propozycja");
    let message = format!("Jarvis: {summary}");
    git(
        &repo,
        &[
            "-c",
            "user.email=jarvis@localhost",
            "-c",
            "user.name=Jarvis",
            "commit",
            "-m",
            &message,
        ],
    )
    .await?;
    Ok(format!("Zapisałem na OCI, gałąź {branch}."))
}

fn repo_path() -> PathBuf {
    PathBuf::from(std::env::var("JARVIS_REPO").unwrap_or_else(|_| "/home/ubuntu/JarvisAgent".into()))
}

fn latest() -> anyhow::Result<Option<Value>> {
    let dir = selfmap::proposals_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(None);
    };
    let mut best: Option<(std::time::SystemTime, Value)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if value["status"].as_str() == Some("odrzucono") {
            continue;
        }
        let modified = entry.metadata().and_then(|meta| meta.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if best.as_ref().map(|(time, _)| modified >= *time).unwrap_or(true) {
            best = Some((modified, value));
        }
    }
    Ok(best.map(|(_, value)| value))
}

fn write_proposal(id: &str, value: &Value) -> anyhow::Result<()> {
    let dir = selfmap::proposals_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{id}.json")), serde_json::to_string_pretty(value)?)?;
    Ok(())
}

fn read_proposal(id: &str) -> anyhow::Result<Value> {
    let id = plugins::safe_id(id).map_err(|err| anyhow::anyhow!(err))?;
    let path = selfmap::proposals_dir().join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&text)?)
}

async fn git(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo).args(args);
    let text = agent::finish(cmd).await;
    if text.lines().last().is_some_and(|line| line.trim() == "kod 0") {
        Ok(text)
    } else {
        Err(anyhow::anyhow!(text))
    }
}

async fn own_image() -> Option<String> {
    let host = std::fs::read_to_string("/etc/hostname").ok()?;
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let output = Command::new("docker")
        .args(["inspect", "-f", "{{.Config.Image}}", host])
        .output()
        .await
        .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !name.is_empty() {
        Some(name)
    } else {
        None
    }
}

fn check_paths(diff: &str, files: &Value) -> Result<(), String> {
    for path in diff_paths(diff) {
        if !allowed_path(&path) {
            return Err(format!("Odrzucam ścieżkę {path}."));
        }
    }
    if let Some(items) = files.as_array() {
        for item in items {
            let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if !allowed_path(path) {
                return Err(format!("Odrzucam ścieżkę {path}."));
            }
        }
    }
    Ok(())
}

fn allowed_path(path: &str) -> bool {
    let path = path.replace('\\', "/");
    if path.starts_with('/') || path.contains("..") || path.contains(".env") || path.contains("server.key") {
        return false;
    }
    const PREFIXES: &[&str] = &[
        "apps/core/",
        "apps/web/",
        "apps/desktop/",
        "apps/mobile/",
        "deploy/",
        "protocol/",
        "design/",
    ];
    PREFIXES.iter().any(|prefix| path.starts_with(prefix))
}

fn diff_paths(diff: &str) -> Vec<String> {
    diff.lines()
        .filter_map(|line| line.strip_prefix("+++ b/"))
        .map(|path| path.trim().to_string())
        .filter(|path| path != "/dev/null")
        .collect()
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

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_sources() {
        assert!(allowed_path("apps/core/src/agent.rs"));
        assert!(allowed_path("apps/web/src/pages/Chat.vue"));
        assert!(allowed_path("apps/desktop/VoiceLoop.cs"));
        assert!(allowed_path("apps/mobile/app/src/main/java/app/jarvis/MainActivity.kt"));
        assert!(!allowed_path(".env"));
        assert!(!allowed_path("apps/core/../../.env"));
        assert!(!allowed_path("/etc/passwd"));
    }
}
