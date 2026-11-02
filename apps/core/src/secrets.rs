use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::password_hash::SaltString;
use rand::rngs::OsRng;

pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD", "PASS"]
        .iter()
        .any(|part| upper.contains(part))
}

pub fn looks_like_secret(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 16 {
        return false;
    }
    let lower = trimmed.to_lowercase();
    if lower.contains("sk-")
        || lower.contains("api_key")
        || lower.contains("bearer ")
        || lower.starts_with("ghp_")
        || lower.contains("BEGIN PRIVATE")
    {
        return true;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() < 24 {
        return false;
    }
    let unique = chars.iter().collect::<std::collections::HashSet<_>>().len();
    let letters = chars.iter().filter(|c| c.is_ascii_alphanumeric()).count();
    letters as f32 / chars.len() as f32 > 0.9 && unique > 16 && !trimmed.contains(' ')
}
