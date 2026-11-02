use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, Mutex, oneshot};

use crate::crypto::Identity;
use crate::envfile::EnvFile;
use crate::llm::Router;
use turso::Database;

pub type Outbound = mpsc::UnboundedSender<String>;

#[derive(Clone)]
pub struct App {
    pub db: Database,
    pub llm: Router,
    pub identity: Arc<Identity>,
    pub env: Arc<Mutex<EnvFile>>,
    pub password_hash: Arc<Mutex<String>>,
    pub web_sessions: Arc<Mutex<HashMap<String, ()>>>,
    pub clients: Arc<Mutex<HashMap<String, Outbound>>>,
    pub presence: Arc<Mutex<Option<String>>>,
    pub pinned: Arc<Mutex<bool>>,
    pub confirms: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    pub results: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
    pub login_attempts: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
    pub study: Arc<Mutex<bool>>,
    pub battery_said: Arc<Mutex<bool>>,
    pub last_move: Arc<Mutex<Option<Instant>>>,
    pub note_folder: Arc<Mutex<Option<String>>>,
    pub notices: Arc<Mutex<Vec<String>>>,
    pub desk: Arc<Mutex<Option<(f64, f64)>>>,
}

impl App {
    pub async fn send(&self, device_id: &str, message: &serde_json::Value) {
        let text = message.to_string();
        let clients = self.clients.lock().await;
        if let Some(tx) = clients.get(device_id) {
            let _ = tx.send(text);
        }
    }

    pub async fn send_presence(&self, message: &serde_json::Value) {
        let id = self.presence.lock().await.clone();
        if let Some(id) = id {
            self.send(&id, message).await;
        } else {
            let clients = self.clients.lock().await;
            for tx in clients.values() {
                let _ = tx.send(message.to_string());
            }
        }
    }

    pub async fn quiet(&self, text: &str) {
        let clients = self.clients.lock().await;
        let phone = clients.keys().find(|id| id.contains("android")).cloned();
        drop(clients);
        let message = serde_json::json!({"type": "quiet", "text": text});
        if let Some(id) = phone {
            self.send(&id, &message).await;
        }
        if let Ok(conn) = self.db.connect() {
            let _ = crate::db::audit(&conn, "push", text).await;
        }
    }
}
