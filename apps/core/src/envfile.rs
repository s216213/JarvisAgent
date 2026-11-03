use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::crypto::Identity;
use crate::secrets::is_secret_name;

pub fn load_dev_dotenv() -> BTreeMap<String, String> {
    if std::env::var("JARVIS_DOCKER").ok().as_deref() == Some("1") {
        return BTreeMap::new();
    }
    let Some(path) = find_dotenv() else {
        return BTreeMap::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let values = parse(&text);
    apply_gaps(&values);
    tracing::info!(path = %path.display(), "dev .env jest źródłem zmiennych");
    values
}

pub fn apply_dev_dotenv(values: &BTreeMap<String, String>) {
    for (key, value) in values {
        if value.is_empty() {
            continue;
        }
        unsafe { std::env::set_var(key, value) };
    }
}

fn find_dotenv() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(".env");
        if candidate.is_file() {
            return Some(candidate);
        }
        if dir.join(".git").is_dir() {
            return None;
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn parse(text: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            values.insert(key.trim().to_string(), value.to_string());
        }
    }
    values
}

fn apply_gaps(values: &BTreeMap<String, String>) {
    for (key, value) in values {
        if value.is_empty() {
            continue;
        }
        let already = std::env::var(key).ok().filter(|set| !set.is_empty());
        if already.is_none() {
            unsafe { std::env::set_var(key, value) };
        }
    }
}

pub struct EnvFile {
    path: PathBuf,
    values: BTreeMap<String, String>,
}

impl EnvFile {
    pub fn load(path: PathBuf, identity: &Identity) -> anyhow::Result<Self> {
        let mut values = BTreeMap::new();
        if path.exists() {
            let bytes = std::fs::read(&path)?;
            let text = if bytes.starts_with(b"JENV1") {
                let plain = identity.open(&bytes[5..])?;
                String::from_utf8(plain)?
            } else {
                String::from_utf8(bytes)?
            };
            values = parse(&text);
        }
        let file = Self { path, values };
        file.apply();
        Ok(file)
    }

    pub fn apply(&self) {
        for (key, value) in &self.values {
            if value.is_empty() && std::env::var(key).ok().is_some_and(|set| !set.is_empty()) {
                continue;
            }
            unsafe { std::env::set_var(key, value) };
        }
    }

    pub fn list(&self) -> Vec<serde_json::Value> {
        self.values
            .iter()
            .map(|(key, value)| {
                let secret = is_secret_name(key);
                serde_json::json!({
                    "name": key,
                    "secret": secret,
                    "set": !value.is_empty(),
                    "preview": if secret { serde_json::Value::Null } else { serde_json::json!(value) }
                })
            })
            .collect()
    }

    pub fn set(&mut self, identity: &Identity, key: &str, value: &str) -> anyhow::Result<()> {
        self.values.insert(key.to_string(), value.to_string());
        unsafe { std::env::set_var(key, value) };
        self.save(identity)
    }

    pub fn remove(&mut self, identity: &Identity, key: &str) -> anyhow::Result<()> {
        self.values.remove(key);
        unsafe { std::env::remove_var(key) };
        self.save(identity)
    }

    fn save(&self, identity: &Identity) -> anyhow::Result<()> {
        let mut body = String::new();
        for (key, value) in &self.values {
            body.push_str(key);
            body.push('=');
            body.push_str(value);
            body.push('\n');
        }
        let sealed = identity.seal(&identity.public, body.as_bytes())?;
        let mut bytes = b"JENV1".to_vec();
        bytes.extend(sealed);
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, bytes)?;
        Ok(())
    }
}
