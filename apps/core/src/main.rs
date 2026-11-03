mod agent;
mod crypto;
mod db;
mod embed;
mod envfile;
mod llm;
mod host;
mod routes;
mod secrets;
mod state;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use secrets::hash_password;
use state::App;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("jarvis_core=info".parse()?))
        .init();

    let data = PathBuf::from(std::env::var("JARVIS_DATA").unwrap_or_else(|_| "data".into()));
    std::fs::create_dir_all(&data)?;
    let identity = Arc::new(crypto::Identity::load_or_create(&data.join("server.key"))?);
    let env = envfile::EnvFile::load(data.join("jarvis.env"), &identity)?;
    let db_path = PathBuf::from(
        std::env::var("JARVIS_DB").unwrap_or_else(|_| data.join("jarvis.db").display().to_string()),
    );
    let database = db::open(&db_path).await?;
    let password = std::env::var("JARVIS_WEB_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "jarvis".into());
    let password_hash = if password.starts_with("$argon2") {
        password
    } else {
        hash_password(&password)?
    };

    let app = App {
        db: database,
        llm: llm::Router::new(),
        identity,
        env: Arc::new(tokio::sync::Mutex::new(env)),
        password_hash: Arc::new(tokio::sync::Mutex::new(password_hash)),
        web_sessions: Arc::new(tokio::sync::Mutex::new(Default::default())),
        clients: Arc::new(tokio::sync::Mutex::new(Default::default())),
        presence: Arc::new(tokio::sync::Mutex::new(None)),
        pinned: Arc::new(tokio::sync::Mutex::new(false)),
        confirms: Arc::new(tokio::sync::Mutex::new(Default::default())),
        results: Arc::new(tokio::sync::Mutex::new(Default::default())),
        login_attempts: Arc::new(tokio::sync::Mutex::new(Default::default())),
        study: Arc::new(tokio::sync::Mutex::new(false)),
        battery_said: Arc::new(tokio::sync::Mutex::new(false)),
        last_move: Arc::new(tokio::sync::Mutex::new(None)),
        note_folder: Arc::new(tokio::sync::Mutex::new(None)),
        notices: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        desk: Arc::new(tokio::sync::Mutex::new(None)),
    };

    let scheduler = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            if let Err(err) = agent::tick_schedules(&scheduler).await {
                tracing::warn!(%err, "harmonogram");
            }
            if let Err(err) = agent::health_tick(&scheduler).await {
                tracing::warn!(%err, "strażnik");
            }
        }
    });

    if std::env::var("TURSO_DATABASE_URL").is_ok() {
        let conn = app.db.connect()?;
        if db::flag(&conn, "turso_sync").await.unwrap_or(false) {
            tracing::info!("sync Turso jest włączony flagą; kopia jedzie dopiero z tokenem");
        } else {
            tracing::info!("sync Turso jest wyłączony");
        }
    }

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    tracing::info!("jarvis-core słucha na {port}");
    app.quiet("Wróciłem po starcie procesu.").await;
    axum::serve(
        listener,
        routes::router(app).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}
