use crate::supervisor::SupervisorState;
use serde::Serialize;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct ClipStorageStats {
    pub clip_count: usize,
    pub total_bytes: u64,
    pub quota_gb: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClipClearResult {
    pub deleted_files: usize,
    pub freed_bytes: u64,
    pub stats: ClipStorageStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct IncidentClearResult {
    pub removed_incidents: usize,
    pub deleted_files: usize,
    pub freed_bytes: u64,
    pub stats: ClipStorageStats,
}

impl SupervisorState {
    /// Inspect the on-disk incident clip folder without modifying files.
    pub async fn clip_storage_stats(&self) -> ClipStorageStats {
        let dir = self.clips_dir.clone();
        let quota_gb = self.cfg.read().await.media.clip_quota_gb;
        let (clip_count, total_bytes) = tokio::task::spawn_blocking(move || scan_clips_dir(&dir))
            .await
            .unwrap_or((0, 0));
        ClipStorageStats {
            clip_count,
            total_bytes,
            quota_gb,
        }
    }

    /// Delete every MP4 clip in storage and clear references from active/stored incidents.
    pub async fn clear_all_clips(&self) -> ClipClearResult {
        let dir = self.clips_dir.clone();
        let (deleted_files, freed_bytes) =
            tokio::task::spawn_blocking(move || delete_all_mp4_in_dir(&dir))
                .await
                .unwrap_or((0, 0));

        {
            let mut list = self.incidents.write().await;
            for inc in list.iter_mut() {
                inc.clip_filename = None;
            }
        }
        if let Err(err) = self.db.clear_clips_now().await {
            tracing::error!("Failed clearing incident clip filenames: {err}");
        }
        let stats = self.clip_storage_stats().await;
        self.broadcast_snapshot().await;
        ClipClearResult {
            deleted_files,
            freed_bytes,
            stats,
        }
    }

    /// Delete a single MP4 clip by filename and clear matching incident links.
    pub async fn delete_single_clip(&self, filename: &str) -> ClipClearResult {
        let dir = self.clips_dir.clone();
        let target = filename.to_string();
        let (deleted_files, freed_bytes) =
            tokio::task::spawn_blocking(move || delete_named_mp4_in_dir(&dir, &target))
                .await
                .unwrap_or((0, 0));

        {
            let mut list = self.incidents.write().await;
            for inc in list.iter_mut() {
                if inc.clip_filename.as_deref() == Some(filename) {
                    inc.clip_filename = None;
                }
            }
        }
        if let Err(err) = self.db.clear_clip_filename_now(filename).await {
            tracing::error!(%filename, "Failed clearing clip filename in DB: {err}");
        }
        let stats = self.clip_storage_stats().await;
        self.broadcast_snapshot().await;
        ClipClearResult {
            deleted_files,
            freed_bytes,
            stats,
        }
    }

    /// Remove resolved incidents and delete their orphaned MP4 replay clips.
    pub async fn clear_resolved_incidents(&self) -> IncidentClearResult {
        let (removed_incidents, orphan_clips) = {
            let mut list = self.incidents.write().await;
            let orig_len = list.len();
            let mut removed_files = Vec::new();
            let mut kept_files = HashSet::new();
            for inc in list.iter() {
                if inc.resolved_at.is_none() {
                    if let Some(f) = &inc.clip_filename {
                        kept_files.insert(f.clone());
                    }
                } else if let Some(f) = &inc.clip_filename {
                    removed_files.push(f.clone());
                }
            }
            list.retain(|inc| inc.resolved_at.is_none());
            let removed_count = orig_len.saturating_sub(list.len());
            removed_files.retain(|f| !kept_files.contains(f));
            (removed_count, removed_files)
        };

        let dir = self.clips_dir.clone();
        let (deleted_files, freed_bytes) = tokio::task::spawn_blocking(move || {
            let mut del = 0usize;
            let mut freed = 0u64;
            for name in orphan_clips {
                let (d, b) = delete_named_mp4_in_dir(&dir, &name);
                del += d;
                freed += b;
            }
            (del, freed)
        })
        .await
        .unwrap_or((0, 0));

        if let Err(err) = self.db.clear_resolved_incidents_now().await {
            tracing::error!("Failed clearing resolved incidents: {err}");
        }
        let stats = self.clip_storage_stats().await;
        self.broadcast_snapshot().await;
        IncidentClearResult {
            removed_incidents,
            deleted_files,
            freed_bytes,
            stats,
        }
    }

    /// Flush all incidents from memory and SQLite, and delete every MP4 replay clip.
    pub async fn clear_all_incidents(&self) -> IncidentClearResult {
        {
            let mut ch_map = self.channels.write().await;
            for m in ch_map.values_mut() {
                m.active_incident_id = None;
            }
        }
        let removed_incidents = {
            let mut list = self.incidents.write().await;
            let count = list.len();
            list.clear();
            count
        };

        let dir = self.clips_dir.clone();
        let (deleted_files, freed_bytes) =
            tokio::task::spawn_blocking(move || delete_all_mp4_in_dir(&dir))
                .await
                .unwrap_or((0, 0));

        if let Err(err) = self.db.clear_all_incidents_now().await {
            tracing::error!("Failed flushing all incidents: {err}");
        }
        let stats = self.clip_storage_stats().await;
        self.broadcast_snapshot().await;
        IncidentClearResult {
            removed_incidents,
            deleted_files,
            freed_bytes,
            stats,
        }
    }
}

pub(crate) fn scan_clips_dir(dir: &Path) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut count = 0usize;
    let mut total = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("mp4") {
            count += 1;
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (count, total)
}

pub(crate) fn delete_all_mp4_in_dir(dir: &Path) -> (usize, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut deleted = 0usize;
    let mut freed = 0u64;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("mp4") {
            continue;
        }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        if std::fs::remove_file(&path).is_ok() {
            deleted += 1;
            freed += size;
        }
    }
    (deleted, freed)
}

pub(crate) fn delete_named_mp4_in_dir(dir: &Path, filename: &str) -> (usize, u64) {
    let path = dir.join(filename);
    if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("mp4") {
        return (0, 0);
    }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if std::fs::remove_file(&path).is_ok() {
        (1, size)
    } else {
        (0, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_and_delete_mp4_clips_only() {
        let tmp = std::env::temp_dir().join(format!("pgmwatch-clips-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("ch1_a.mp4"), vec![1u8; 1024]).unwrap();
        std::fs::write(tmp.join("ch1_b.mp4"), vec![2u8; 2048]).unwrap();
        std::fs::write(tmp.join("keep_me.txt"), b"note").unwrap();

        assert_eq!(scan_clips_dir(&tmp), (2, 3072));
        assert_eq!(delete_named_mp4_in_dir(&tmp, "ch1_a.mp4"), (1, 1024));
        assert_eq!(scan_clips_dir(&tmp), (1, 2048));

        assert_eq!(delete_all_mp4_in_dir(&tmp), (1, 2048));
        assert_eq!(scan_clips_dir(&tmp), (0, 0));
        assert!(tmp.join("keep_me.txt").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
