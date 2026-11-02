const DIM: usize = 64;

pub fn embed(text: &str) -> Vec<f32> {
    let mut vec = vec![0f32; DIM];
    let lower = text.to_lowercase();
    for token in lower.split(|c: char| !c.is_alphanumeric()) {
        if token.len() < 2 {
            continue;
        }
        let mut hash: u32 = 2166136261;
        for byte in token.bytes() {
            hash ^= byte as u32;
            hash = hash.wrapping_mul(16777619);
        }
        let idx = (hash as usize) % DIM;
        let sign = if hash & 1 == 0 { 1.0 } else { -1.0 };
        vec[idx] += sign;
    }
    let norm = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vec {
            *value /= norm;
        }
    }
    vec
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

pub fn to_json(vec: &[f32]) -> String {
    serde_json::to_string(vec).unwrap_or_else(|_| "[]".into())
}

pub fn from_json(text: &str) -> Vec<f32> {
    serde_json::from_str(text).unwrap_or_default()
}
