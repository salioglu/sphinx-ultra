use anyhow::Result;
use clap::{Parser, Subcommand};
use log::{info, warn};
use std::path::PathBuf;

mod builder;
mod cache;
mod config;
mod document;
mod error;
mod parser;
mod utils;

use builder::SphinxBuilder;
use config::BuildConfig;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Build documentation
    Build {
        /// Source directory
        #[arg(short, long, default_value = ".")]
        source: PathBuf,

        /// Output directory
        #[arg(short, long, default_value = "_build")]
        output: PathBuf,

        /// Number of parallel jobs
        #[arg(short, long)]
        jobs: Option<usize>,

        /// Clean output directory before build
        #[arg(long)]
        clean: bool,

        /// Enable incremental builds
        #[arg(long)]
        incremental: bool,

        /// Turn warnings into errors
        #[arg(short = 'W', long)]
        fail_on_warning: bool,
    },

    /// Clean build artifacts
    Clean {
        /// Output directory
        #[arg(short, long, default_value = "_build")]
        output: PathBuf,
    },

    /// Show build statistics
    Stats {
        /// Source directory
        #[arg(short, long, default_value = ".")]
        source: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    std::env::set_var("RUST_LOG", log_level);
    env_logger::init();

    info!("Sphinx Ultra Builder v{}", env!("CARGO_PKG_VERSION"));

    match cli.command {
        Commands::Build {
            source,
            output,
            jobs,
            clean,
            incremental,
            fail_on_warning,
        } => {
            let mut config = if let Some(ref config_path) = cli.config {
                BuildConfig::from_file(config_path)?
            } else {
                BuildConfig::default()
            };

            // Override config with CLI arguments
            if fail_on_warning {
                config.fail_on_warning = true;
            }

            // Save the fail_on_warning flag before moving config
            let should_fail_on_warning = config.fail_on_warning;

            let mut builder = SphinxBuilder::new(config, source, output)?;

            if let Some(jobs) = jobs {
                builder.set_parallel_jobs(jobs);
            }

            if clean {
                builder.clean().await?;
            }

            if incremental {
                builder.enable_incremental();
            }

            let stats = builder.build().await?;

            // Print warnings in Sphinx-like format
            for warning in &stats.warning_details {
                let file_path = warning.file.display();
                let line_info = if let Some(line) = warning.line {
                    format!(":{}", line)
                } else {
                    String::new()
                };
                warn!("{}{}: WARNING: {}", file_path, line_info, warning.message);
            }

            // Print errors in Sphinx-like format
            for error in &stats.error_details {
                let file_path = error.file.display();
                let line_info = if let Some(line) = error.line {
                    format!(":{}", line)
                } else {
                    String::new()
                };
                eprintln!("{}{}: ERROR: {}", file_path, line_info, error.message);
            }

            // Check for fail-on-warning condition
            if should_fail_on_warning && stats.warnings > 0 {
                eprintln!("Build failed due to warnings (caused by --fail-on-warning)");
                std::process::exit(1);
            }

            // Print final summary
            if stats.warnings > 0 || stats.errors > 0 {
                let status_msg = if stats.errors > 0 {
                    "build succeeded with problems"
                } else {
                    "build succeeded"
                };

                if stats.warnings > 0 && stats.errors > 0 {
                    warn!(
                        "{}, {} warnings, {} errors.",
                        status_msg, stats.warnings, stats.errors
                    );
                } else if stats.warnings > 0 {
                    warn!("{}, {} warnings.", status_msg, stats.warnings);
                } else if stats.errors > 0 {
                    warn!("{}, {} errors.", status_msg, stats.errors);
                }
            }

            info!("Build completed successfully!");
            info!("Files processed: {}", stats.files_processed);
            info!("Files skipped: {}", stats.files_skipped);
            info!("Cache hits: {}", stats.cache_hits);
            info!("Build time: {:?}", stats.build_time);
            info!("Output size: {} MB", stats.output_size_mb);
        }

        Commands::Clean { output } => {
            info!("Cleaning output directory: {}", output.display());
            if output.exists() {
                std::fs::remove_dir_all(&output)?;
                info!("Clean completed");
            } else {
                warn!("Output directory does not exist");
            }
        }

        Commands::Stats { source } => {
            let stats = utils::analyze_project(&source).await?;

            println!("Project Statistics:");
            println!("  Source files: {}", stats.source_files);
            println!("  Total lines: {}", stats.total_lines);
            println!("  Average file size: {} KB", stats.avg_file_size_kb);
            println!("  Largest file: {} KB", stats.largest_file_kb);
            println!("  Directory depth: {}", stats.max_depth);
            println!("  Cross-references: {}", stats.cross_references);
        }
    }

    Ok(())
}
