//! Sphinx Ultra Builder
//!
//! A high-performance Rust-based Sphinx documentation builder designed for large codebases.

pub mod builder;
pub mod cache;
pub mod config;
pub mod directives;
pub mod document;
pub mod environment;
pub mod error;
pub mod extensions;
pub mod html_builder;
pub mod inventory;
pub mod parser;
pub mod python_config;
pub mod roles;
pub mod search;
pub mod template;
pub mod utils;

pub use builder::{BuildStats, SphinxBuilder};
pub use config::BuildConfig;
pub use directives::{Directive, DirectiveRegistry};
pub use document::Document;
pub use environment::BuildEnvironment;
pub use error::BuildError;
pub use extensions::{ExtensionLoader, SphinxApp, SphinxExtension};
pub use html_builder::HTMLBuilder;
pub use inventory::{InventoryFile, InventoryItem};
pub use parser::Parser;
pub use python_config::{ConfPyConfig, PythonConfigParser};
pub use roles::{Role, RoleRegistry};
pub use search::SearchIndex;
pub use template::TemplateEngine;
pub use utils::{analyze_project, ProjectStats};
