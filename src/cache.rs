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
    pub fn new(cache_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&cache_dir)?;

        let cache = Self {
            cache_dir,
            documents: Arc::new(DashMap::new()),
            file_hashes: Arc::new(RwLock::new(HashMap::new())),
            hit_count: Arc::new(RwLock::new(0)),
            miss_count: Arc::new(RwLock::new(0)),
            max_size_mb: 500, // Default 500MB cache
            expiration_duration: Duration::from_secs(24 * 60 * 60), // 24 hours
        };

        // Load existing cache from disk
        cache.load_from_disk()?;

        Ok(cache)
    }

    pub fn get_document(&self, file_path: &Path) -> Result<Document> {
        let hash = self.calculate_file_hash(file_path)?;

        if let Some(cached) = self.documents.get(file_path) {
            if cached.hash == hash && !self.is_expired(&cached.cached_at) {
                // Update access count
                self.documents.alter(file_path, |_, mut cached| {
                    cached.access_count += 1;
                    cached
                });

                *self.hit_count.write() += 1;
                debug!("Cache hit for {}", file_path.display());
                return Ok(cached.document.clone());
            }
            // Remove expired or outdated entry
            self.documents.remove(file_path);
        }

        *self.miss_count.write() += 1;
        debug!("Cache miss for {}", file_path.display());
        Err(BuildError::Cache("Document not found in cache".to_string()).into())
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

    #[allow(dead_code)]
    pub fn clear(&self) -> Result<()> {
        self.documents.clear();
        self.file_hashes.write().clear();
        *self.hit_count.write() = 0;
        *self.miss_count.write() = 0;

        if self.cache_dir.exists() {
            std::fs::remove_dir_all(&self.cache_dir)?;
            std::fs::create_dir_all(&self.cache_dir)?;
        }

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
            self.evict_lru_entries(new_size_mb)?;
        }

        Ok(())
    }

    fn evict_lru_entries(&self, space_needed_mb: f64) -> Result<()> {
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

        // Sort by access count (LRU)
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
