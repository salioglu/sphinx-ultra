//! Sphinx Ultra Builder
//!
//! A high-performance Rust-based Sphinx documentation builder designed for large codebases.

pub mod builder;
pub mod cache;
pub mod config;
pub mod document;
pub mod environment;
pub mod error;
pub mod html_builder;
pub mod inventory;
pub mod parser;
pub mod search;
pub mod server;
pub mod template;
pub mod utils;
pub mod watcher;

pub use builder::{BuildStats, SphinxBuilder};
pub use config::BuildConfig;
pub use document::Document;
pub use environment::BuildEnvironment;
pub use error::BuildError;
pub use html_builder::HTMLBuilder;
pub use inventory::{InventoryFile, InventoryItem};
pub use search::SearchIndex;
pub use template::TemplateEngine;
