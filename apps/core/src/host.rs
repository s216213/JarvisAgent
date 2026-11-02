use std::io::{Read, Write};

use axum::extract::ws::{Message, WebSocket};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde_json::{json, Value};

pub fn resources() -> Value {
    let free_mb = mem_available_mb();
    json!({
        "free_mb": free_mb,
        "desktop": free_mb >= 1500,
        "listen_restart": "Zmiana adresu nasłuchu wymaga restartu kontenera."
    })
}

pub fn mem_available_mb() -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return 0;
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb: u64 = rest.split_whitespace().next().unwrap_or("0").parse().unwrap_or(0);
            return kb / 1024;
        }
    }
    0
}

pub async fn compose_ps() -> String {
    let output = tokio::process::Command::new("docker")
        .args(["compose", "ps", "--format", "json"])
        .output()
        .await;
    match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout).to_string(),
        Err(err) => err.to_string(),
    }
}

pub async fn restart_service(service: &str) -> String {
    if !service.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
        return "Zła nazwa usługi.".into();
    }
    if service == "core" {
        return "Własnego procesu nie restartuję od środka.".into();
    }
    let output = tokio::process::Command::new("docker")
        .args(["compose", "restart", service])
        .output()
        .await;
    match output {
        Ok(output) => {
            let err = String::from_utf8_lossy(&output.stderr);
            if output.status.success() {
                format!("Zrestartowałem {service}.")
            } else {
                err.to_string()
            }
        }
        Err(err) => err.to_string(),
    }
}

pub async fn start_desktop() -> String {
    if mem_available_mb() < 1500 {
        return "Za mało wolnego RAM na pulpit. Zostaje terminal.".into();
    }
    let output = tokio::process::Command::new("docker")
        .args(["compose", "--profile", "desktop", "up", "-d", "webtop"])
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => "Pulpit nasłuchuje na localhost:6080.".into(),
        Ok(output) => String::from_utf8_lossy(&output.stderr).to_string(),
        Err(err) => err.to_string(),
    }
}

pub async fn tunnel_problem() -> Option<String> {
    let host = std::env::var("JARVIS_PUBLIC_HOST").ok()?;
    let url = if host.starts_with("http") {
        host
    } else {
        format!("https://{host}/health")
    };
    match reqwest::get(url).await {
        Ok(response) if response.status().is_success() => None,
        Ok(response) => Some(format!("Publiczny adres odpowiada {}.", response.status())),
        Err(_) => Some("Publiczny adres nie odpowiada.".into()),
    }
}

pub async fn serve_pty(mut socket: WebSocket) {
    let system = native_pty_system();
    let Ok(pair) = system.openpty(PtySize {
        rows: 28,
        cols: 100,
        pixel_width: 0,
        pixel_height: 0,
    }) else {
        let _ = socket.send(Message::Text("Nie mogę otworzyć terminala.".into())).await;
        return;
    };
    let shell = if cfg!(windows) { "powershell.exe" } else { "bash" };
    let mut cmd = CommandBuilder::new(shell);
    if let Ok(dir) = std::env::var("JARVIS_DEPLOY") {
        cmd.cwd(dir);
    }
    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(child) => child,
        Err(err) => {
            let _ = socket.send(Message::Text(err.to_string().into())).await;
            return;
        }
    };
    let reader = pair.master.try_clone_reader().ok();
    let mut writer = pair.master.take_writer().ok();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    if let Some(mut reader) = reader {
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(w) = writer.as_mut() {
                            let _ = w.write_all(text.as_bytes());
                            let _ = w.flush();
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            chunk = rx.recv() => {
                let Some(bytes) = chunk else { break };
                let text = String::from_utf8_lossy(&bytes).to_string();
                if socket.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = child.kill();
}
