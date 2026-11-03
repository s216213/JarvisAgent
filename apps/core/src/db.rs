use std::path::Path;

use anyhow::Context;
use turso::{params, Builder, Connection, Database};

pub async fn open(path: &Path) -> anyhow::Result<Database> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let db = Builder::new_local(path.to_string_lossy().as_ref())
        .build()
        .await
        .context("otwarcie pliku libSQL")?;
    let conn = db.connect()?;
    migrate(&conn).await?;
    Ok(db)
}

async fn migrate(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            pubkey TEXT,
            token_hash TEXT,
            last_seen INTEGER NOT NULL DEFAULT 0,
            idle INTEGER NOT NULL DEFAULT 1,
            away INTEGER NOT NULL DEFAULT 0,
            speech INTEGER NOT NULL DEFAULT 0,
            app TEXT,
            window_title TEXT,
            telemetry TEXT,
            capabilities TEXT
        );
        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            role TEXT NOT NULL,
            body TEXT NOT NULL,
            created INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS facts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            body TEXT NOT NULL,
            hash TEXT NOT NULL UNIQUE,
            created INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS chunks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            path TEXT NOT NULL,
            body TEXT NOT NULL,
            embedding TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS entities (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            kind TEXT NOT NULL,
            UNIQUE(name, kind)
        );
        CREATE TABLE IF NOT EXISTS edges (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            from_id INTEGER NOT NULL,
            to_id INTEGER NOT NULL,
            rel TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS feature_flags (
            name TEXT PRIMARY KEY,
            enabled INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS schedules (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            at TEXT NOT NULL,
            instruction TEXT NOT NULL,
            last_day TEXT
        );
        CREATE TABLE IF NOT EXISTS audit (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            kind TEXT NOT NULL,
            detail TEXT NOT NULL,
            created INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS restarts (
            service TEXT NOT NULL,
            hour_key TEXT NOT NULL,
            count INTEGER NOT NULL,
            PRIMARY KEY(service, hour_key)
        );
        CREATE TABLE IF NOT EXISTS jobs (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            pinned INTEGER NOT NULL,
            device_id TEXT,
            status TEXT NOT NULL,
            detail TEXT
        );
        CREATE TABLE IF NOT EXISTS web_sessions (
            token_hash TEXT PRIMARY KEY,
            created INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS evolution (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            created INTEGER NOT NULL,
            kind TEXT NOT NULL,
            status TEXT NOT NULL,
            summary TEXT NOT NULL,
            detail TEXT NOT NULL DEFAULT ''
        );
        ",
    )
    .await?;
    if conn
        .execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(path, body, content='');",
        )
        .await
        .is_err()
    {
        tracing::info!("fts5 niedostępne w tym buildzie, szukanie leksykalne zostaje w pamięci procesu");
    }
    seed_flags(conn).await?;
    Ok(())
}

const FLAG_COPY: &[(&str, &str)] = &[
    ("voice", "Fraza wybudzająca, nasłuch i mowa na komputerze i telefonie. Przełącznik jest zapisany; rdzeń go nie sprawdza."),
    ("hud", "Pływający pasek ze statusem i linijką odpowiedzi. Przełącznik jest zapisany; rdzeń go nie sprawdza."),
    ("palette", "Paleta Alt+Space: notatka, program, polecenie. Przełącznik jest zapisany; rdzeń go nie sprawdza."),
    ("clipboard", "Schowek idzie na pozostałe urządzenia. Tekst podobny do sekretu i tak zostaje."),
    ("telemetry", "Lokalizacja, bateria i stan maszyny przy obecności. Przełącznik jest zapisany; rdzeń go nie sprawdza."),
    ("proactive", "Jarvis mówi o niskiej baterii, wysokim procesorze i zmianie okna. W trybie nauki milczy."),
    ("desktop_modes", "Przełączenie trybu pulpitu, także trybu nauki, w którym powiadomienia czekają."),
    ("shell", "Polecenie w powłoce. Najpierw karta zgody."),
    ("scripts", "Jednorazowy skrypt. Najpierw karta zgody."),
    ("digest", "Skrót powiadomień: ludzie, terminy, alarmy."),
    ("schedule", "Zapis i odpalenie harmonogramu ustalonego w rozmowie."),
    ("classifier", "Surowa myśl ląduje jako notatka w folderze vaultu."),
    ("links", "Nowa notatka dostaje sekcję Powiązania."),
    ("graph", "Z rozmowy powstają fakty, osoby, projekty i krawędzie."),
    ("map", "Widget mapy i linia pogody."),
    ("search", "Szukanie w sieci i karta z wynikiem."),
    ("vision_screen", "Opis bieżącego ekranu Windows."),
    ("vision_camera", "Zdjęcie z aparatu i opis tabliczki, układu albo dokumentu."),
    ("health_guard", "Podnosi padniętą usługę Dockera, najwyżej trzy razy na godzinę."),
    ("log_analyzer", "Przy panic albo 500 zostawia propozycję poprawki w vaultcie. Liczy się tylko w takcie strażnika."),
    ("turso_sync", "Zgoda na sync pliku bazy. Sam adres Turso w env kopii nie uruchamia. Domyślnie wyłączona."),
    ("plugins", "Narzędzia z katalogu plugins, ładowane bez przebudowy rdzenia."),
    ("evolution", "Gdy brakuje narzędzia, Jarvis składa plugin albo propozycję kodu. Ryzykowne i rdzeń czekają na zgodę."),
    ("simulation", "Przebieg próbny i scenariusz co-jeśli. Nic nie wykonuje."),
    ("space", "Widok przestrzenny: hologram siebie i mapa świata."),
    ("persona", "Ton lokaja: spokojnie, krótko, zwrot panie. Wyłączona zostawia krótki ton bez zwrotu."),
    ("briefing", "Raport statusu i jedno powitanie dziennie, gdy ktoś siada przy pulpicie."),
    ("missions", "Jedna misja w tle. Odczyt idzie od razu, powłoka czeka na zgodę."),
];

pub fn flag_description(name: &str) -> &'static str {
    FLAG_COPY
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, text)| *text)
        .unwrap_or("Brak opisu.")
}

async fn seed_flags(conn: &Connection) -> anyhow::Result<()> {
    let flags = [
        ("voice", 1),
        ("hud", 1),
        ("palette", 1),
        ("clipboard", 1),
        ("telemetry", 1),
        ("proactive", 1),
        ("desktop_modes", 1),
        ("shell", 1),
        ("scripts", 1),
        ("digest", 1),
        ("schedule", 1),
        ("classifier", 1),
        ("links", 1),
        ("graph", 1),
        ("map", 1),
        ("search", 1),
        ("vision_screen", 1),
        ("vision_camera", 1),
        ("health_guard", 1),
        ("log_analyzer", 1),
        ("turso_sync", 0),
        ("plugins", 1),
        ("evolution", 1),
        ("simulation", 1),
        ("space", 1),
        ("persona", 1),
        ("briefing", 1),
        ("missions", 1),
    ];
    for (name, enabled) in flags {
        conn.execute(
            "INSERT INTO feature_flags (name, enabled) VALUES (?1, ?2) ON CONFLICT(name) DO NOTHING",
            params![name, enabled],
        )
        .await?;
    }
    Ok(())
}

pub async fn flag(conn: &Connection, name: &str) -> anyhow::Result<bool> {
    let mut rows = conn
        .query(
            "SELECT enabled FROM feature_flags WHERE name = ?1",
            params![name],
        )
        .await?;
    if let Some(row) = rows.next().await? {
        let value: i64 = row.get(0)?;
        return Ok(value != 0);
    }
    Ok(true)
}

pub async fn set_flag(conn: &Connection, name: &str, enabled: bool) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO feature_flags (name, enabled) VALUES (?1, ?2)
         ON CONFLICT(name) DO UPDATE SET enabled = excluded.enabled",
        params![name, if enabled { 1 } else { 0 }],
    )
    .await?;
    Ok(())
}

pub async fn list_flags(conn: &Connection) -> anyhow::Result<Vec<(String, bool)>> {
    let mut rows = conn
        .query("SELECT name, enabled FROM feature_flags ORDER BY name", ())
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let name: String = row.get(0)?;
        let enabled: i64 = row.get(1)?;
        out.push((name, enabled != 0));
    }
    Ok(out)
}

pub async fn save_web_session(conn: &Connection, token_hash: &str) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO web_sessions (token_hash, created) VALUES (?1, ?2)
         ON CONFLICT(token_hash) DO UPDATE SET created = excluded.created",
        params![token_hash, now],
    )
    .await?;
    Ok(())
}

pub async fn web_session_exists(conn: &Connection, token_hash: &str) -> anyhow::Result<bool> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM web_sessions WHERE token_hash = ?1",
            params![token_hash],
        )
        .await?;
    Ok(rows.next().await?.is_some())
}

pub async fn clear_web_sessions(conn: &Connection) -> anyhow::Result<()> {
    conn.execute("DELETE FROM web_sessions", ()).await?;
    Ok(())
}

pub async fn audit(conn: &Connection, kind: &str, detail: &str) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO audit (kind, detail, created) VALUES (?1, ?2, ?3)",
        params![kind, detail, now],
    )
    .await?;
    Ok(())
}

pub fn protected_flag(name: &str) -> bool {
    matches!(name, "shell" | "scripts" | "evolution")
}

pub async fn record_evolution(
    conn: &Connection,
    kind: &str,
    status: &str,
    summary: &str,
    detail: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    let detail: String = detail.chars().take(8000).collect();
    conn.execute(
        "INSERT INTO evolution (created, kind, status, summary, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![now, kind, status, summary, detail.as_str()],
    )
    .await?;
    Ok(())
}

pub async fn list_evolution(conn: &Connection, limit: i64) -> anyhow::Result<Vec<(i64, String, String, String)>> {
    let mut rows = conn
        .query(
            "SELECT created, kind, status, summary FROM evolution ORDER BY id DESC LIMIT ?1",
            params![limit],
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let created: i64 = row.get(0)?;
        let kind: String = row.get(1)?;
        let status: String = row.get(2)?;
        let summary: String = row.get(3)?;
        out.push((created, kind, status, summary));
    }
    Ok(out)
}

pub async fn count_rows(conn: &Connection, table: &str) -> anyhow::Result<i64> {
    let sql = match table {
        "chunks" => "SELECT COUNT(*) FROM chunks",
        "facts" => "SELECT COUNT(*) FROM facts",
        "edges" => "SELECT COUNT(*) FROM edges",
        "messages" => "SELECT COUNT(*) FROM messages",
        _ => return Ok(0),
    };
    let mut rows = conn.query(sql, ()).await?;
    if let Some(row) = rows.next().await? {
        let count: i64 = row.get(0)?;
        return Ok(count);
    }
    Ok(0)
}

pub async fn recent_audit(conn: &Connection, limit: i64) -> anyhow::Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT kind, detail FROM audit ORDER BY id DESC LIMIT ?1",
            params![limit],
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let kind: String = row.get(0)?;
        let detail: String = row.get(1)?;
        out.push(format!("{kind}: {detail}"));
    }
    Ok(out)
}
