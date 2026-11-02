use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::Context;
use rand::RngCore;
use sha2::{Digest, Sha256};
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

pub struct Identity {
    secret: StaticSecret,
    pub public: PublicKey,
}

impl Identity {
    pub fn load_or_create(path: &std::path::Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let secret = if path.exists() {
            let bytes = std::fs::read(path)?;
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("zły plik klucza"))?;
            StaticSecret::from(arr)
        } else {
            let secret = StaticSecret::random_from_rng(rand::rngs::OsRng);
            std::fs::write(path, secret.to_bytes())?;
            secret
        };
        let public = PublicKey::from(&secret);
        Ok(Self { secret, public })
    }

    pub fn seal(&self, recipient: &PublicKey, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        let _ = self;
        seal_to(recipient, plaintext)
    }

    pub fn open(&self, blob: &[u8]) -> anyhow::Result<Vec<u8>> {
        if blob.len() < 32 + 12 {
            anyhow::bail!("za krótki szyfrogram");
        }
        let mut eph_pub = [0u8; 32];
        eph_pub.copy_from_slice(&blob[..32]);
        let eph = PublicKey::from(eph_pub);
        let shared = self.secret.diffie_hellman(&eph);
        let key = Sha256::digest(shared.as_bytes());
        let cipher = Aes256Gcm::new_from_slice(&key).context("klucz aes")?;
        let nonce = Nonce::from_slice(&blob[32..44]);
        cipher
            .decrypt(nonce, &blob[44..])
            .map_err(|_| anyhow::anyhow!("nie udało się odszyfrować"))
    }
}

pub fn seal_to(recipient: &PublicKey, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
    let eph = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let eph_pub = PublicKey::from(&eph);
    let shared = eph.diffie_hellman(recipient);
    let key = Sha256::digest(shared.as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key)?;
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| anyhow::anyhow!("szyfrowanie"))?;
    let mut out = Vec::with_capacity(32 + 12 + ct.len());
    out.extend_from_slice(eph_pub.as_bytes());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn fingerprint(text: &str) -> String {
    hex::encode(Sha256::digest(text.trim().to_lowercase().as_bytes()))
}
