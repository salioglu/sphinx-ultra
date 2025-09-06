use anyhow::Result;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use tokio::sync::broadcast;

pub struct FileWatcher {
    source_dir: PathBuf,
    change_sender: broadcast::Sender<PathBuf>,
    _watcher: notify::RecommendedWatcher,
}

impl FileWatcher {
    pub fn new(source_dir: PathBuf) -> Result<Self> {
        let (tx, _rx) = broadcast::channel(100);
        let (file_tx, file_rx): (
            Sender<notify::Result<Event>>,
            Receiver<notify::Result<Event>>,
        ) = mpsc::channel();

        let mut watcher = notify::recommended_watcher(move |res| {
            if let Err(e) = file_tx.send(res) {
                eprintln!("Failed to send file event: {}", e);
            }
        })?;

        watcher.watch(&source_dir, RecursiveMode::Recursive)?;

        let change_sender = tx.clone();
        let source_dir_clone = source_dir.clone();

        // Spawn a task to handle file system events
        tokio::spawn(async move {
            while let Ok(event) = file_rx.recv() {
                match event {
                    Ok(event) => {
                        if let Some(path) = Self::should_trigger_rebuild(&event, &source_dir_clone)
                        {
                            if let Err(e) = change_sender.send(path) {
                                eprintln!("Failed to broadcast file change: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("File watcher error: {}", e);
                    }
                }
            }
        });

        Ok(Self {
            source_dir,
            change_sender: tx,
            _watcher: watcher,
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PathBuf> {
        self.change_sender.subscribe()
    }

    fn should_trigger_rebuild(event: &Event, source_dir: &PathBuf) -> Option<PathBuf> {
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                for path in &event.paths {
                    // Only rebuild for source files
                    if let Some(ext) = path.extension() {
                        match ext.to_string_lossy().as_ref() {
                            "rst" | "md" | "txt" => {
                                // Make sure the file is within our source directory
                                if path.starts_with(source_dir) {
                                    return Some(path.clone());
                                }
                            }
                            _ => continue,
                        }
                    }
                }
            }
            _ => {}
        }
        None
    }
}
