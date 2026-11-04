//! A store's achievement answers kept on disk: one JSON file of lists keyed
//! by `app:language`, each with the time it was fetched. Fresh entries are
//! served without asking the store; stale ones stand in while it can't be
//! reached. Reads and writes run on the blocking pool.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::game::Achievement;
use crate::{json_store, util};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Entry {
    at: u64,
    list: Vec<Achievement>,
}

/// A cached answer and whether it is younger than the cache's age limit.
#[derive(Debug, Clone, PartialEq)]
pub struct Cached {
    pub list: Vec<Achievement>,
    pub fresh: bool,
}

/// The achievement cache of one store.
#[derive(Debug, Clone)]
pub struct AchievementCache {
    path: PathBuf,
    max_age_secs: u64,
}

fn key(app: &str, language: &str) -> String {
    format!("{app}:{language}")
}

impl AchievementCache {
    pub fn new(path: impl Into<PathBuf>, max_age_secs: u64) -> Self {
        Self {
            path: path.into(),
            max_age_secs,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn load(&self) -> HashMap<String, Entry> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || json_store::read_or_default(&path))
            .await
            .unwrap_or_default()
    }

    /// The cached list of `app` in `language`, if any.
    pub async fn get(&self, app: &str, language: &str) -> Option<Cached> {
        let entry = self.load().await.remove(&key(app, language))?;
        Some(Cached {
            fresh: util::unix_now().saturating_sub(entry.at) < self.max_age_secs,
            list: entry.list,
        })
    }

    /// Keep `list` as the answer for `app` in `language`.
    pub async fn put(&self, app: &str, language: &str, list: &[Achievement]) {
        let mut all = self.load().await;
        all.insert(
            key(app, language),
            Entry {
                at: util::unix_now(),
                list: list.to_vec(),
            },
        );
        self.save(all).await;
    }

    /// Mark every cached answer of `app` stale, keeping it as a stand-in.
    pub async fn forget(&self, app: &str) {
        let mut all = self.load().await;
        let prefix = format!("{app}:");
        let mut changed = false;
        for (k, e) in all.iter_mut() {
            if k.starts_with(&prefix) {
                e.at = 0;
                changed = true;
            }
        }
        if changed {
            self.save(all).await;
        }
    }

    async fn save(&self, all: HashMap<String, Entry>) {
        let path = self.path.clone();
        let result = tokio::task::spawn_blocking(move || json_store::write(&path, &all)).await;
        if let Ok(Err(e)) = result {
            tracing::warn!("saving achievements: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_fresh_then_stale_answers() {
        let dir = tempfile::tempdir().unwrap();
        let cache = AchievementCache::new(dir.path().join("a.json"), 600);
        assert!(cache.get("620", "en").await.is_none());
        let list = vec![Achievement {
            id: "a".into(),
            ..Default::default()
        }];
        cache.put("620", "en", &list).await;
        cache.put("6200", "en", &list).await;
        let got = cache.get("620", "en").await.unwrap();
        assert!(got.fresh);
        assert_eq!(got.list, list);
        cache.forget("620").await;
        assert!(!cache.get("620", "en").await.unwrap().fresh);
        assert!(cache.get("6200", "en").await.unwrap().fresh);
    }
}
