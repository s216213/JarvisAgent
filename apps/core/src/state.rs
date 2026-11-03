use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{mpsc, Mutex, oneshot};

use crate::crypto::Identity;
use crate::envfile::EnvFile;
use crate::llm::Router;
use turso::Database;

pub type Outbound = mpsc::UnboundedSender<String>;

#[derive(Clone, Debug, Default)]
pub struct Pulse {
    pub cpu: Option<f64>,
    pub ram: Option<f64>,
    pub battery: Option<f64>,
    pub network: String,
}

#[derive(Clone, Debug, Default)]
pub struct Hud {
    pub alert: String,
    pub mission: String,
}

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
    pub pulse: Arc<Mutex<Pulse>>,
    pub hud: Arc<Mutex<Hud>>,
    pub last_window: Arc<Mutex<String>>,
    pub window_said: Arc<Mutex<Option<Instant>>>,
    pub cpu_high: Arc<Mutex<u8>>,
    pub cpu_said: Arc<Mutex<bool>>,
    pub greeted_day: Arc<Mutex<Option<String>>>,
    pub last_digest: Arc<Mutex<String>>,
    pub mission_cancel: Arc<Mutex<Option<Arc<AtomicBool>>>>,
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

    pub async fn publish_hud(&self) {
        let pulse = self.pulse.lock().await.clone();
        let hud = self.hud.lock().await.clone();
        let message = serde_json::json!({
            "type": "hud_state",
            "cpu": pulse.cpu,
            "ram": pulse.ram,
            "battery": pulse.battery,
            "network": pulse.network,
            "alert": hud.alert,
            "mission": hud.mission,
        });
        let clients = self.clients.lock().await;
        let text = message.to_string();
        for tx in clients.values() {
            let _ = tx.send(text.clone());
        }
    }

    pub async fn set_alert(&self, text: &str) {
        self.hud.lock().await.alert = text.to_string();
        self.publish_hud().await;
    }

    pub async fn set_mission(&self, text: &str) {
        self.hud.lock().await.mission = text.to_string();
        self.publish_hud().await;
    }
}
