use anyhow::Result;
use blake3::Hasher;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use log::{debug, warn};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use crate::document::Document;
use crate::error::BuildError;

pub struct BuildCache {
    cache_dir: PathBuf,
    config_fingerprint: String,
    config_changed: bool,
    documents: Arc<DashMap<PathBuf, CachedDocument>>,
    file_hashes: Arc<RwLock<HashMap<PathBuf, String>>>,
    hit_count: Arc<RwLock<usize>>,
    miss_count: Arc<RwLock<usize>>,
    max_size_mb: usize,
    expiration_duration: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedDocument {
    document: Document,
    hash: String,
    cached_at: DateTime<Utc>,
    access_count: usize,
    size_bytes: usize,
}

impl BuildCache {
    pub fn new(
        cache_dir: PathBuf,
        max_size_mb: usize,
        expiration_hours: u64,
        config_fingerprint: &str,
    ) -> Result<Self> {
        std::fs::create_dir_all(&cache_dir)?;

        // Cached documents were produced under a specific configuration; if
        // the configuration changed, everything in the cache is stale.
        let fingerprint_file = cache_dir.join(".config-fingerprint");
        let stored = std::fs::read_to_string(&fingerprint_file).unwrap_or_default();
        let config_changed = stored.trim() != config_fingerprint;
        if config_changed {
            if !stored.is_empty() {
                debug!("Configuration changed; discarding cache");
            }
            std::fs::remove_dir_all(&cache_dir)?;
            std::fs::create_dir_all(&cache_dir)?;
            std::fs::write(&fingerprint_file, config_fingerprint)?;
        }

        let cache = Self {
            cache_dir,
            config_fingerprint: config_fingerprint.to_string(),
            config_changed,
            documents: Arc::new(DashMap::new()),
            file_hashes: Arc::new(RwLock::new(HashMap::new())),
            hit_count: Arc::new(RwLock::new(0)),
            miss_count: Arc::new(RwLock::new(0)),
            max_size_mb,
            expiration_duration: Duration::from_secs(expiration_hours * 60 * 60),
        };

        // Load existing cache from disk
        cache.load_from_disk()?;

        Ok(cache)
    }

    /// The cache directory this cache was constructed with (post config-
    /// fingerprint validation/wipe). Callers that persist their own files
    /// alongside the document cache -- e.g. `BuildEnvironment::save`'s
    /// `env.bin` -- ride the same directory and are therefore covered by
    /// the same fingerprint-mismatch wipe.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// Whether this cache directory's stored fingerprint disagreed with the
    /// configuration it was opened with — the whole directory was then
    /// discarded, this build's documents, doctrees and `env.bin` included.
    ///
    /// A first build counts too (there is no stored fingerprint to agree
    /// with), which is what Sphinx's `CONFIG_NEW` is: both mean "nothing
    /// carried over from a previous build is usable", and both make every
    /// document outdated.
    pub fn config_changed(&self) -> bool {
        self.config_changed
    }

    pub fn get_document(&self, file_path: &Path) -> Result<Document> {
        self.get_document_with(file_path, |_| Some(()))
            .map(|(document, ())| document)
            .ok_or_else(|| BuildError::Cache("Document not found in cache".to_string()).into())
    }

    /// Look up a cached document, letting the caller have the final say.
    ///
    /// `accept` runs only after the entry passed the cache's own checks
    /// (content+mtime hash, expiry). Returning `None` from it means the
    /// caller cannot actually use the entry — because some companion state
    /// it needs is missing, say — and the lookup is then counted and
    /// reported as a **miss**, not a hit: a "hit" the build has to redo
    /// anyway is not a hit. Anything `accept` computes from the document
    /// (loading that companion state) comes back alongside it, so callers
    /// don't have to do the work twice.
    pub fn get_document_with<T>(
        &self,
        file_path: &Path,
        accept: impl FnOnce(&Document) -> Option<T>,
    ) -> Option<(Document, T)> {
        let hash = match self.calculate_file_hash(file_path) {
            Ok(hash) => hash,
            Err(_) => {
                *self.miss_count.write() += 1;
                return None;
            }
        };

        // Clone what we need out of the `get` guard before touching the map
        // again: holding a DashMap `Ref` while calling `alter` on the same
        // key deadlocks on the shard lock.
        let cached = self
            .documents
            .get(file_path)
            .map(|c| (c.hash.clone(), c.cached_at, c.document.clone()));

        if let Some((cached_hash, cached_at, document)) = cached {
            if cached_hash == hash && !self.is_expired(&cached_at) {
                if let Some(extra) = accept(&document) {
                    // Update access count
                    self.documents.alter(file_path, |_, mut cached| {
                        cached.access_count += 1;
                        cached
                    });

                    *self.hit_count.write() += 1;
                    debug!("Cache hit for {}", file_path.display());
                    return Some((document, extra));
                }
            } else {
                // Remove expired or outdated entry. A caller-rejected entry
                // is left alone: it is still a valid record of the file,
                // and the rebuild that follows overwrites it anyway.
                self.documents.remove(file_path);
            }
        }

        *self.miss_count.write() += 1;
        debug!("Cache miss for {}", file_path.display());
        None
    }

    pub fn store_document(&self, file_path: &Path, document: &Document) -> Result<()> {
        let hash = self.calculate_file_hash(file_path)?;
        let size_bytes = self.estimate_document_size(document);

        let cached_doc = CachedDocument {
            document: document.clone(),
            hash: hash.clone(),
            cached_at: Utc::now(),
            access_count: 1,
            size_bytes,
        };

        // Check if we need to evict some entries
        self.evict_if_needed(size_bytes)?;

        self.documents.insert(file_path.to_path_buf(), cached_doc);
        self.file_hashes
            .write()
            .insert(file_path.to_path_buf(), hash.clone());

        debug!(
            "Cached document: {} ({} bytes)",
            file_path.display(),
            size_bytes
        );

        // Persist to disk asynchronously
        self.persist_to_disk(file_path, document)?;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn invalidate(&self, file_path: &Path) {
        self.documents.remove(file_path);
        self.file_hashes.write().remove(file_path);

        // Remove from disk cache
        let cache_file = self.get_cache_file_path(file_path);
        if cache_file.exists() {
            if let Err(e) = std::fs::remove_file(&cache_file) {
                warn!(
                    "Failed to remove cache file {}: {}",
                    cache_file.display(),
                    e
                );
            }
        }

        debug!("Invalidated cache for {}", file_path.display());
    }

    /// Empty the cache, in memory and on disk (`-E`, `--clean`).
    ///
    /// The fingerprint file is written back immediately: it records which
    /// configuration the *directory* belongs to, and leaving it missing
    /// would make the next build mistake this deliberate emptying for a
    /// configuration change and throw away everything the build that
    /// follows this one is about to cache.
    #[allow(dead_code)]
    pub fn clear(&self) -> Result<()> {
        self.documents.clear();
        self.file_hashes.write().clear();
        *self.hit_count.write() = 0;
        *self.miss_count.write() = 0;

        if self.cache_dir.exists() {
            std::fs::remove_dir_all(&self.cache_dir)?;
        }
        std::fs::create_dir_all(&self.cache_dir)?;
        std::fs::write(
            self.cache_dir.join(".config-fingerprint"),
            &self.config_fingerprint,
        )?;

        debug!("Cleared all cache");
        Ok(())
    }

    pub fn hit_count(&self) -> usize {
        *self.hit_count.read()
    }

    #[allow(dead_code)]
    pub fn miss_count(&self) -> usize {
        *self.miss_count.read()
    }

    #[allow(dead_code)]
    pub fn hit_ratio(&self) -> f64 {
        let hits = *self.hit_count.read() as f64;
        let misses = *self.miss_count.read() as f64;
        if hits + misses > 0.0 {
            hits / (hits + misses)
        } else {
            0.0
        }
    }

    pub fn size_mb(&self) -> f64 {
        let total_bytes: usize = self
            .documents
            .iter()
            .map(|entry| entry.value().size_bytes)
            .sum();
        total_bytes as f64 / 1024.0 / 1024.0
    }

    fn calculate_file_hash(&self, file_path: &Path) -> Result<String> {
        let content = std::fs::read(file_path)?;
        let metadata = std::fs::metadata(file_path)?;

        let mut hasher = Hasher::new();
        hasher.update(&content);

        // Include file metadata in hash
        if let Ok(modified) = metadata.modified() {
            if let Ok(duration) = modified.duration_since(UNIX_EPOCH) {
                hasher.update(&duration.as_secs().to_le_bytes());
            }
        }

        Ok(hasher.finalize().to_hex().to_string())
    }

    fn is_expired(&self, cached_at: &DateTime<Utc>) -> bool {
        let now = Utc::now();
        let elapsed = now.signed_duration_since(*cached_at);
        elapsed.num_seconds() > self.expiration_duration.as_secs() as i64
    }

    fn estimate_document_size(&self, document: &Document) -> usize {
        // Rough estimate of document size in memory
        document.html.len()
            + document.title.len()
            + document.source_path.to_string_lossy().len()
            + document.output_path.to_string_lossy().len()
            + 1024 // Overhead for other fields
    }

    fn evict_if_needed(&self, new_size: usize) -> Result<()> {
        let current_size_mb = self.size_mb();
        let new_size_mb = (new_size as f64) / 1024.0 / 1024.0;

        if current_size_mb + new_size_mb > self.max_size_mb as f64 {
            self.evict_least_accessed_entries(new_size_mb)?;
        }

        Ok(())
    }

    /// Evict entries with the lowest access counts (LFU-style). This is not
    /// LRU — recency is not tracked — and is named accordingly.
    fn evict_least_accessed_entries(&self, space_needed_mb: f64) -> Result<()> {
        let mut entries: Vec<_> = self
            .documents
            .iter()
            .map(|entry| {
                (
                    entry.key().clone(),
                    entry.value().access_count,
                    entry.value().size_bytes,
                )
            })
            .collect();

        // Sort by access count (least-accessed first)
        entries.sort_by_key(|(_, access_count, _)| *access_count);

        let mut space_freed_mb = 0.0;
        for (path, _, size_bytes) in entries {
            if space_freed_mb >= space_needed_mb {
                break;
            }

            self.documents.remove(&path);
            self.file_hashes.write().remove(&path);
            space_freed_mb += (size_bytes as f64) / 1024.0 / 1024.0;

            debug!(
                "Evicted {} from cache ({} MB)",
                path.display(),
                size_bytes as f64 / 1024.0 / 1024.0
            );
        }

        Ok(())
    }

    fn load_from_disk(&self) -> Result<()> {
        if !self.cache_dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.path().extension().is_some_and(|ext| ext == "json")
            {
                if let Err(e) = self.load_cache_file(&entry.path()) {
                    warn!(
                        "Failed to load cache file {}: {}",
                        entry.path().display(),
                        e
                    );
                }
            }
        }

        debug!("Loaded {} documents from disk cache", self.documents.len());
        Ok(())
    }

    fn load_cache_file(&self, cache_file: &Path) -> Result<()> {
        let content = std::fs::read_to_string(cache_file)?;
        let cached_doc: CachedDocument = serde_json::from_str(&content)?;

        // Check if the cached document is still valid
        if !self.is_expired(&cached_doc.cached_at) {
            let source_path = &cached_doc.document.source_path;
            if source_path.exists() {
                let current_hash = self.calculate_file_hash(source_path)?;
                if current_hash == cached_doc.hash {
                    self.documents.insert(source_path.clone(), cached_doc);
                }
            }
        }

        Ok(())
    }

    fn persist_to_disk(&self, file_path: &Path, _document: &Document) -> Result<()> {
        let cache_file = self.get_cache_file_path(file_path);
        if let Some(parent) = cache_file.parent() {
            std::fs::create_dir_all(parent)?;
        }

        if let Some(cached_doc) = self.documents.get(file_path) {
            let content = serde_json::to_string_pretty(&*cached_doc)?;
            std::fs::write(&cache_file, content)?;
        }

        Ok(())
    }

    fn get_cache_file_path(&self, file_path: &Path) -> PathBuf {
        let hash = blake3::hash(file_path.to_string_lossy().as_bytes());
        let filename = format!("{}.json", hash.to_hex());
        self.cache_dir.join(filename)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_document(source: &Path) -> Document {
        let mut doc = Document::new(source.to_path_buf(), source.with_extension("html"));
        doc.html = "<html><body>cached</body></html>".to_string();
        doc.source_mtime = Utc::now();
        doc
    }

    #[test]
    fn roundtrip_preserves_rendered_html() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("page.rst");
        std::fs::write(&source, "Page\n----\n").unwrap();

        let cache = BuildCache::new(tmp.path().join("cache"), 500, 24, "fp-1").unwrap();
        cache
            .store_document(&source, &make_document(&source))
            .unwrap();

        let restored = cache.get_document(&source).unwrap();
        assert_eq!(restored.html, "<html><body>cached</body></html>");
        assert_eq!(cache.hit_count(), 1);
    }

    #[test]
    fn warm_hit_does_not_deadlock() {
        // Regression: `get` guard held across `alter` on the same DashMap key
        // deadlocked every warm incremental rebuild.
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("page.rst");
        std::fs::write(&source, "Page\n----\n").unwrap();

        let cache = BuildCache::new(tmp.path().join("cache"), 500, 24, "fp-1").unwrap();
        cache
            .store_document(&source, &make_document(&source))
            .unwrap();
        for _ in 0..3 {
            cache.get_document(&source).unwrap();
        }
        assert_eq!(cache.hit_count(), 3);
    }

    #[test]
    fn caller_rejected_entry_counts_as_a_miss() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("page.rst");
        std::fs::write(&source, "Page\n----\n").unwrap();

        let cache = BuildCache::new(tmp.path().join("cache"), 500, 24, "fp-1").unwrap();
        cache
            .store_document(&source, &make_document(&source))
            .unwrap();

        let rejected = cache.get_document_with(&source, |_| None::<()>);
        assert!(rejected.is_none());
        assert_eq!(cache.hit_count(), 0, "a rejected entry is not a hit");
        assert_eq!(cache.miss_count(), 1);

        // The entry survives rejection, so a later accepting lookup hits.
        let accepted = cache.get_document_with(&source, |doc| Some(doc.html.clone()));
        assert_eq!(
            accepted.map(|(_, html)| html).as_deref(),
            Some("<html><body>cached</body></html>")
        );
        assert_eq!(cache.hit_count(), 1);
    }

    #[test]
    fn changed_fingerprint_discards_persisted_cache() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("page.rst");
        std::fs::write(&source, "Page\n----\n").unwrap();
        let cache_dir = tmp.path().join("cache");

        {
            let cache = BuildCache::new(cache_dir.clone(), 500, 24, "fp-1").unwrap();
            cache
                .store_document(&source, &make_document(&source))
                .unwrap();
        }

        // Same fingerprint: persisted entry survives.
        {
            let cache = BuildCache::new(cache_dir.clone(), 500, 24, "fp-1").unwrap();
            assert!(cache.get_document(&source).is_ok());
        }

        // Different fingerprint: cache is wiped.
        {
            let cache = BuildCache::new(cache_dir, 500, 24, "fp-2").unwrap();
            assert!(cache.get_document(&source).is_err());
        }
    }

    #[test]
    fn clearing_keeps_the_directory_claimed_by_this_configuration() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("page.rst");
        std::fs::write(&source, "Page\n----\n").unwrap();
        let cache_dir = tmp.path().join("cache");

        {
            let cache = BuildCache::new(cache_dir.clone(), 500, 24, "fp-1").unwrap();
            cache.clear().unwrap();
            // What a build after `-E`/`--clean` caches:
            cache
                .store_document(&source, &make_document(&source))
                .unwrap();
        }

        let cache = BuildCache::new(cache_dir, 500, 24, "fp-1").unwrap();
        assert!(
            !cache.config_changed(),
            "an emptied cache still belongs to the configuration that emptied it"
        );
        assert!(
            cache.get_document(&source).is_ok(),
            "a cleared cache that was refilled must survive to the next build"
        );
    }

    #[test]
    fn config_changed_reports_the_wipe() {
        let tmp = TempDir::new().unwrap();
        let cache_dir = tmp.path().join("cache");

        // A first build has nothing to agree with: everything is new.
        assert!(BuildCache::new(cache_dir.clone(), 500, 24, "fp-1")
            .unwrap()
            .config_changed());
        assert!(!BuildCache::new(cache_dir.clone(), 500, 24, "fp-1")
            .unwrap()
            .config_changed());
        assert!(BuildCache::new(cache_dir, 500, 24, "fp-2")
            .unwrap()
            .config_changed());
    }

    #[test]
    fn expiration_hours_are_plumbed() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join("page.rst");
        std::fs::write(&source, "Page\n----\n").unwrap();

        // 0-hour expiry: everything is expired immediately.
        let cache = BuildCache::new(tmp.path().join("cache"), 500, 0, "fp-1").unwrap();
        cache
            .store_document(&source, &make_document(&source))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(
            cache.get_document(&source).is_err(),
            "entries must expire per the configured horizon"
        );
    }
}
