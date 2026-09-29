use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub batch_id: String,
    pub created_unix: i64,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    /// Where the item originally lived.
    pub original_path: PathBuf,
    /// Path relative to the batch directory.
    pub stored_rel: String,
    pub logical_size: u64,
    pub on_disk_size: u64,
    pub is_dir: bool,
    pub rule_id: String,
    pub quarantined_unix: i64,
}

/// Summary shown in the quarantine tab.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchMeta {
    pub batch_id: String,
    pub created_unix: i64,
    pub item_count: usize,
    pub logical_bytes: u64,
    pub on_disk_bytes: u64,
    pub dir: String,
}

impl Manifest {
    pub fn load(dir: &Path) -> Option<Manifest> {
        let path = dir.join("manifest.json");
        let s = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&s).ok()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let path = dir.join("manifest.json");
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    /// Total on-disk footprint, which is what the user actually gains.
    pub fn on_disk_total(&self) -> u64 {
        self.entries.iter().map(|e| e.on_disk_size).sum()
    }

    pub fn logical_total(&self) -> u64 {
        self.entries.iter().map(|e| e.logical_size).sum()
    }
}

/// Enumerate all batches currently in the quarantine, newest first.
pub fn list_batches(root: &Path) -> Vec<BatchMeta> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        if let Some(m) = Manifest::load(&p) {
            out.push(BatchMeta {
                batch_id: m.batch_id.clone(),
                created_unix: m.created_unix,
                item_count: m.entries.len(),
                logical_bytes: m.logical_total(),
                on_disk_bytes: m.on_disk_total(),
                dir: p.to_string_lossy().to_string(),
            });
        }
    }
    out.sort_by(|a, b| b.created_unix.cmp(&a.created_unix));
    out
}
