//! # sphinx-ultra
//!
//! A reimplementation of `sphinx-build -b html` in Rust, built for large
//! documentation trees and verified against real Sphinx output.
//!
//! The crate is used through its binary, `sphinx-ultra`, which accepts both
//! its native `build`/`clean`/`stats` subcommands and the `sphinx-build`
//! argument shape (`sphinx-ultra SOURCEDIR OUTPUTDIR -b html`). The library
//! surface below is what that binary is built from: [`SphinxBuilder`] runs
//! the read → merge → resolve → write pipeline over a persisted
//! [`BuildEnvironment`], [`BuildConfig`] loads `conf.py` (a declarative
//! subset), YAML or JSON configuration, and the `rst`, `doctree`, `env`,
//! `py` and `intersphinx` modules are ports of the corresponding docutils
//! and Sphinx subsystems, each pinned by a differential test against the
//! upstream implementation.
//!
//! The project is pre-1.0. The current status of every subsystem, with
//! evidence, is kept in the repository's `docs/IMPLEMENTATION_STATUS.md`.
#![doc(
    html_logo_url = "https://raw.githubusercontent.com/salioglu/sphinx-ultra/main/assets/icon.svg"
)]
#![doc(
    html_favicon_url = "https://raw.githubusercontent.com/salioglu/sphinx-ultra/main/assets/icon.svg"
)]

pub mod builder;
pub mod cache;
pub mod config;
pub mod directives;
pub mod doctree;
pub mod document;
pub mod env;
pub mod error;
pub mod extensions;
pub mod html_builder;
pub mod intersphinx;
pub mod inventory;
pub mod matching;
pub mod parser;
pub mod py;
pub mod python_config;
pub mod rst;
pub mod search;
pub mod template;
pub mod utils;
pub mod validation;

pub use builder::{BuildStats, SphinxBuilder};
pub use config::BuildConfig;
pub use directives::{
    validation::{
        DirectiveValidationResult, DirectiveValidationSystem, DirectiveValidator, ParsedDirective,
        ParsedRole, RoleValidationResult, RoleValidator,
        ValidationStatistics as DirectiveValidationStatistics,
    },
    Directive, DirectiveRegistry,
};
pub use document::Document;
pub use env::BuildEnvironment;
pub use error::BuildError;
pub use extensions::{ExtensionLoader, SphinxApp, SphinxExtension};
pub use html_builder::HTMLBuilder;
pub use inventory::{posix_join, InvObject, Inventory, InventoryFile, InventoryItem};
pub use parser::Parser;
pub use python_config::{ConfPyConfig, PythonConfigParser};
pub use search::SearchIndex;
pub use template::TemplateEngine;
pub use utils::{analyze_project, ProjectStats};
pub use validation::{
    ConstraintEngine, ConstraintValidator, ContentItem, FieldValue, ValidationConfig,
    ValidationContext, ValidationResult, ValidationRule, ValidationSeverity, Validator,
};
