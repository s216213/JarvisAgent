use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, Query, State};
use std::net::SocketAddr;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use cookie::Cookie;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tower_http::cors::CorsLayer;
use turso::params;
use uuid::Uuid;

use crate::agent::{self, handle_text};
use crate::host;
use crate::crypto::hash_token;
use crate::db;
use crate::secrets::{hash_password, is_secret_name, verify_password};
use crate::state::App;

pub fn router(app: App) -> Router {
    let origin = std::env::var("JARVIS_WEB_ORIGIN").unwrap_or_else(|_| "http://localhost:5173".into());
    let cors = CorsLayer::new()
        .allow_origin(
            origin
                .parse::<axum::http::HeaderValue>()
                .unwrap_or_else(|_| axum::http::HeaderValue::from_static("http://localhost:5173")),
        )
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION, header::COOKIE])
        .allow_credentials(true);
    Router::new()
        .route("/health", get(health))
        .route("/api/login", post(login))
        .route("/api/flags", get(flags_get).post(flags_post))
        .route("/api/env", get(env_get).post(env_post))
        .route("/api/status", get(status))
        .route("/api/host", get(host_get).post(host_post))
        .route("/api/desktop", post(desktop_post))
        .route("/api/pair", post(pair))
        .route("/api/vision", post(vision))
        .route("/api/ws", get(ws))
        .route("/api/term", get(term))
        .layer(cors)
        .with_state(app)
}

async fn health() -> &'static str {
    "ok"
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

async fn login(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(app): State<App>,
    Json(body): Json<LoginBody>,
) -> impl IntoResponse {
    let key = addr.ip().to_string();
    {
        let mut attempts = app.login_attempts.lock().await;
        if let Some((count, at)) = attempts.get(&key).copied() {
            if count >= 8 && at.elapsed() < Duration::from_secs(60) {
                return (StatusCode::TOO_MANY_REQUESTS, "za dużo prób").into_response();
            }
            if at.elapsed() >= Duration::from_secs(60) {
                attempts.remove(&key);
            }
        }
    }
    let hash = app.password_hash.lock().await.clone();
    if !verify_password(&body.password, &hash) {
        let mut attempts = app.login_attempts.lock().await;
        let entry = attempts.entry(key).or_insert((0, Instant::now()));
        entry.0 += 1;
        entry.1 = Instant::now();
        return (StatusCode::UNAUTHORIZED, "złe hasło").into_response();
    }
    app.login_attempts.lock().await.remove(&key);
    let session = Uuid::new_v4().to_string();
    app.web_sessions.lock().await.insert(session.clone(), ());
    let secure = std::env::var("JARVIS_COOKIE_SECURE").ok().as_deref() == Some("1");
    let mut cookie = Cookie::build(("jarvis_session", session.clone()))
        .http_only(true)
        .path("/")
        .same_site(if secure {
            cookie::SameSite::None
        } else {
            cookie::SameSite::Lax
        });
    if secure {
        cookie = cookie.secure(true);
    }
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie.build().to_string())],
        Json(json!({"session": session})),
    )
        .into_response()
}

fn session_ok(app: &App, headers: &HeaderMap) -> impl std::future::Future<Output = bool> + Send {
    session_with(app, headers, None)
}

fn session_with(
    app: &App,
    headers: &HeaderMap,
    query_session: Option<String>,
) -> impl std::future::Future<Output = bool> + Send {
    let app = app.clone();
    let raw = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim_start_matches("Bearer ")
        .to_string();
    async move {
        if let Some(token) = query_session.as_deref() {
            if !token.is_empty() && app.web_sessions.lock().await.contains_key(token) {
                return true;
            }
        }
        if let Ok(jar) = cookie::Cookie::parse(raw.split(';').next().unwrap_or("")) {
            if jar.name() == "jarvis_session" && app.web_sessions.lock().await.contains_key(jar.value())
            {
                return true;
            }
        }
        for part in raw.split(';') {
            if let Ok(jar) = cookie::Cookie::parse(part.trim()) {
                if jar.name() == "jarvis_session"
                    && app.web_sessions.lock().await.contains_key(jar.value())
                {
                    return true;
                }
            }
        }
        if bearer.is_empty() {
            return false;
        }
        let hashed = hash_token(&bearer);
        let Ok(conn) = app.db.connect() else {
            return false;
        };
        let Ok(mut rows) = conn
            .query(
                "SELECT id FROM devices WHERE token_hash = ?1",
                params![hashed.as_str()],
            )
            .await
        else {
            return false;
        };
        rows.next().await.ok().flatten().is_some()
    }
}

async fn flags_get(State(app): State<App>, headers: HeaderMap) -> impl IntoResponse {
    if !session_ok(&app, &headers).await {
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    let Ok(conn) = app.db.connect() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "baza").into_response();
    };
    match db::list_flags(&conn).await {
        Ok(flags) => {
            let body = flags
                .into_iter()
                .map(|(name, enabled)| {
                    json!({
                        "name": name,
                        "enabled": enabled,
                        "description": db::flag_description(&name),
                    })
                })
                .collect::<Vec<_>>();
            Json(json!({"flags": body})).into_response()
        }
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct FlagBody {
    name: String,
    enabled: bool,
}

async fn flags_post(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<FlagBody>,
) -> impl IntoResponse {
    if !session_ok(&app, &headers).await {
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    let Ok(conn) = app.db.connect() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "baza").into_response();
    };
    if db::set_flag(&conn, &body.name, body.enabled).await.is_err() {
        return (StatusCode::BAD_REQUEST, "flaga").into_response();
    }
    let _ = db::audit(&conn, "flag", &body.name).await;
    StatusCode::OK.into_response()
}

async fn env_get(State(app): State<App>, headers: HeaderMap) -> impl IntoResponse {
    if !session_ok(&app, &headers).await {
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    let env = app.env.lock().await;
    Json(json!({"vars": env.list()})).into_response()
}

#[derive(Deserialize)]
struct EnvBody {
    name: String,
    value: Option<String>,
    remove: Option<bool>,
}

async fn env_post(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<EnvBody>,
) -> impl IntoResponse {
    if !session_ok(&app, &headers).await {
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    let mut env = app.env.lock().await;
    let result = if body.remove.unwrap_or(false) {
        env.remove(&app.identity, &body.name)
    } else {
        env.set(&app.identity, &body.name, body.value.as_deref().unwrap_or(""))
    };
    if let Err(err) = result {
        return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
    }
    if body.name == "JARVIS_WEB_PASSWORD" {
        if let Some(value) = body.value.as_deref() {
            if let Ok(hash) = hash_password(value) {
                *app.password_hash.lock().await = hash;
                app.web_sessions.lock().await.clear();
            }
        }
    }
    drop(env);
    if let Ok(conn) = app.db.connect() {
        let label = if is_secret_name(&body.name) {
            body.name.clone()
        } else {
            body.name.clone()
        };
        let _ = db::audit(&conn, "env", &label).await;
    }
    StatusCode::OK.into_response()
}

async fn status(State(app): State<App>, headers: HeaderMap) -> impl IntoResponse {
    if !session_ok(&app, &headers).await {
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    let Ok(conn) = app.db.connect() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "baza").into_response();
    };
    let audit = db::recent_audit(&conn, 12).await.unwrap_or_default();
    let presence = app.presence.lock().await.clone();
    let clients: Vec<String> = app.clients.lock().await.keys().cloned().collect();
    Json(json!({
        "presence": presence,
        "devices": clients,
        "audit": audit,
        "providers": crate::llm::Router::providers(),
        "recent_429": app.llm.recent_429().await,
        "host": host::resources(),
        "turso": std::env::var("TURSO_DATABASE_URL").is_ok() && db::flag(&conn, "turso_sync").await.unwrap_or(false)
    }))
    .into_response()
}

async fn host_get(State(app): State<App>, headers: HeaderMap) -> impl IntoResponse {
    if !session_ok(&app, &headers).await {
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    let ps = host::compose_ps().await;
    Json(json!({"ps": ps, "resources": host::resources()})).into_response()
}

#[derive(Deserialize)]
struct HostBody {
    service: String,
}

async fn host_post(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<HostBody>,
) -> impl IntoResponse {
    if !session_ok(&app, &headers).await {
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    let text = host::restart_service(&body.service).await;
    if let Ok(conn) = app.db.connect() {
        let _ = db::audit(&conn, "restart", &body.service).await;
    }
    Json(json!({"text": text})).into_response()
}

async fn desktop_post(State(app): State<App>, headers: HeaderMap) -> impl IntoResponse {
    if !session_ok(&app, &headers).await {
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    Json(json!({"text": host::start_desktop().await})).into_response()
}

#[derive(Deserialize)]
struct TermQuery {
    session: Option<String>,
}

async fn term(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<TermQuery>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    if !session_with(&app, &headers, query.session).await {
        tracing::warn!("terminal odrzucony, brak sesji");
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    if let Ok(conn) = app.db.connect() {
        let _ = db::audit(&conn, "pty", "start").await;
    }
    upgrade.on_upgrade(host::serve_pty).into_response()
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
    id: String,
    kind: String,
    name: String,
}

async fn pair(State(app): State<App>, Json(body): Json<PairBody>) -> impl IntoResponse {
    let expected = std::env::var("JARVIS_PAIR_CODE").unwrap_or_else(|_| "jarvis".into());
    if body.code != expected {
        return (StatusCode::UNAUTHORIZED, "zły kod").into_response();
    }
    let token = Uuid::new_v4().to_string();
    let Ok(conn) = app.db.connect() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "baza").into_response();
    };
    if conn
        .execute(
            "INSERT INTO devices (id, kind, name, token_hash) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET token_hash = excluded.token_hash, kind = excluded.kind",
            params![
                body.id.as_str(),
                body.kind.as_str(),
                body.name.as_str(),
                hash_token(&token).as_str()
            ],
        )
        .await
        .is_err()
    {
        return (StatusCode::BAD_REQUEST, "para").into_response();
    }
    Json(json!({"token": token})).into_response()
}

#[derive(Deserialize)]
struct VisionBody {
    image: String,
    mime: Option<String>,
    kind: Option<String>,
}

async fn vision(
    State(app): State<App>,
    headers: HeaderMap,
    Json(body): Json<VisionBody>,
) -> impl IntoResponse {
    if !session_ok(&app, &headers).await {
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    let mime = body.mime.unwrap_or_else(|| "image/jpeg".into());
    let result = if body.kind.as_deref() == Some("screen") {
        agent::glance_saved(&app, &body.image, &mime).await
    } else {
        agent::save_photo(&app, &body.image, &mime).await
    };
    match result {
        Ok(text) => Json(json!({"text": text})).into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
struct WsQuery {
    device_id: Option<String>,
    session: Option<String>,
}

async fn ws(
    State(app): State<App>,
    headers: HeaderMap,
    Query(query): Query<WsQuery>,
    upgrade: WebSocketUpgrade,
) -> impl IntoResponse {
    if !session_with(&app, &headers, query.session).await {
        return (StatusCode::UNAUTHORIZED, "brak sesji").into_response();
    }
    let device_id = query
        .device_id
        .unwrap_or_else(|| format!("web-{}", Uuid::new_v4()));
    upgrade
        .on_upgrade(move |socket| client_loop(app, device_id, socket))
        .into_response()
}

async fn client_loop(app: App, device_id: String, mut socket: WebSocket) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    app.clients.lock().await.insert(device_id.clone(), tx);
    if app.presence.lock().await.is_none() {
        *app.presence.lock().await = Some(device_id.clone());
    }
    let conn = app.db.connect();
    if let Ok(conn) = conn {
        let _ = conn
            .execute(
                "INSERT INTO devices (id, kind, name) VALUES (?1, 'web', ?1)
                 ON CONFLICT(id) DO NOTHING",
                params![device_id.as_str()],
            )
            .await;
    }
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            route_event(&app, &device_id, &value).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            outgoing = rx.recv() => {
                match outgoing {
                    Some(text) => {
                        if socket.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
        }
    }
    app.clients.lock().await.remove(&device_id);
}

async fn route_event(app: &App, device_id: &str, value: &Value) {
    let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "user_text" => {
            let text = value.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if let Err(err) = handle_text(app, device_id, text).await {
                tracing::error!(%err, "tura");
            }
        }
        "presence" => {
            let _ = agent::update_presence(app, device_id, value).await;
        }
        "confirm" => {
            let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let accept = value.get("accept").and_then(|v| v.as_bool()).unwrap_or(false);
            let via = value.get("via").and_then(|v| v.as_str()).unwrap_or("button");
            let _ = agent::resolve_confirm(app, id, accept, via).await;
        }
        "clipboard" => {
            let text = value.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let _ = agent::ingest_clipboard(app, device_id, text).await;
        }
        "tool_result" => {
            let id = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let output = value.get("output").and_then(|v| v.as_str()).unwrap_or("");
            agent::tool_result(app, id, output).await;
        }
        "pin" => {
            *app.pinned.lock().await = value.get("on").and_then(|v| v.as_bool()).unwrap_or(true);
        }
        "digest" => {
            let _ = agent::ingest_notice(
                app,
                value.get("app").and_then(|v| v.as_str()).unwrap_or(""),
                value.get("title").and_then(|v| v.as_str()).unwrap_or(""),
                value.get("body").and_then(|v| v.as_str()).unwrap_or(""),
            )
            .await;
        }
        _ => {}
    }
}
