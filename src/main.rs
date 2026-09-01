use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
use log::{info, warn};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use sphinx_ultra::{analyze_project, BuildConfig, SphinxBuilder};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Configuration file path
    #[arg(short, long, global = true)]
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

        /// Write warnings (and errors) to given file
        #[arg(short = 'w', long)]
        warning_file: Option<PathBuf>,
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

/// sphinx-build compatible argument mode: `sphinx-ultra SOURCEDIR OUTPUTDIR
/// [FILENAMES...] [options]`, what quickstart Makefiles and CI invoke.
#[derive(Parser)]
#[command(
    name = "sphinx-ultra",
    version,
    about = "sphinx-build compatible mode",
    long_about = None
)]
struct SphinxBuildCli {
    /// Source directory (containing conf.py unless -c is given)
    sourcedir: PathBuf,

    /// Output directory
    outputdir: PathBuf,

    /// Specific files to (re)build — accepted, not supported yet
    filenames: Vec<PathBuf>,

    /// Builder to use (only 'html' is supported in 0.4)
    #[arg(short = 'b', long = "builder", default_value = "html")]
    builder: String,

    /// Make-mode: sphinx-ultra -M MODE SOURCEDIR OUTPUTDIR (html writes to
    /// OUTPUTDIR/html, clean empties OUTPUTDIR)
    #[arg(short = 'M', value_name = "MODE")]
    make_mode: Option<String>,

    /// Configuration directory (containing conf.py); a file path also works
    #[arg(short = 'c', long = "conf-dir", value_name = "PATH")]
    confdir: Option<PathBuf>,

    /// Cache ("doctree") directory; defaults to OUTPUTDIR/.sphinx-ultra-cache
    #[arg(short = 'd', long = "doctree-dir", value_name = "PATH")]
    doctreedir: Option<PathBuf>,

    /// Override a configuration value
    #[arg(short = 'D', value_name = "setting=value", value_parser = parse_key_val, action = ArgAction::Append)]
    define: Vec<(String, String)>,

    /// Pass a value into HTML templates (html_context)
    #[arg(short = 'A', value_name = "name=value", value_parser = parse_key_val, action = ArgAction::Append)]
    html_define: Vec<(String, String)>,

    /// Define a tag (consumed by only/ifconfig once M2 lands)
    #[arg(short = 't', long = "tag", value_name = "TAG", action = ArgAction::Append)]
    tags: Vec<String>,

    /// Nitpicky mode: warn about all missing references
    #[arg(short = 'n', long = "nitpicky")]
    nitpicky: bool,

    /// No output on stdout, just warnings on stderr
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,

    /// Don't use a saved environment, always read all files
    #[arg(short = 'E', long = "fresh-env")]
    fresh_env: bool,

    /// Write all files (default: only write new and changed files)
    #[arg(short = 'a', long = "write-all")]
    write_all: bool,

    /// Show full traceback on exception
    #[arg(short = 'T', long = "show-traceback")]
    traceback: bool,

    /// Run in parallel with N processes, or 'auto' for all cores
    #[arg(short = 'j', long = "jobs", value_name = "N", value_parser = parse_jobs)]
    jobs: Option<usize>,

    /// Turn warnings into errors
    #[arg(short = 'W', long = "fail-on-warning")]
    fail_on_warning: bool,

    /// Accepted no-op: collecting all warnings before failing is already the
    /// default (sphinx-build ≥8.1 parity)
    #[arg(long = "keep-going")]
    keep_going: bool,

    /// Write warnings (and errors) to given file
    #[arg(short = 'w', long = "warning-file", value_name = "FILE")]
    warning_file: Option<PathBuf>,

    /// Increase verbosity (can be repeated)
    #[arg(short = 'v', long = "verbose", action = ArgAction::Count)]
    verbose: u8,
}

fn parse_key_val(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((k, v)) if !k.is_empty() => Ok((k.to_string(), v.to_string())),
        _ => Err(format!("expected key=value, got '{s}'")),
    }
}

fn parse_jobs(s: &str) -> Result<usize, String> {
    if s == "auto" {
        Ok(std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1))
    } else {
        s.parse::<usize>()
            .map_err(|_| format!("expected a number or 'auto', got '{s}'"))
    }
}

/// Decide whether argv is a native invocation (subcommands) or sphinx-build
/// compatible mode (positional SOURCEDIR OUTPUTDIR).
fn wants_sphinx_build_mode(args: &[String]) -> bool {
    let first = match args.get(1) {
        Some(f) => f.as_str(),
        None => return false,
    };
    if matches!(
        first,
        "build" | "clean" | "stats" | "help" | "-h" | "--help" | "-V" | "--version"
    ) {
        return false;
    }
    // A bare first token that isn't a subcommand is a sphinx-build SOURCEDIR
    // (a source dir literally named `build` needs `./build` — documented).
    if !first.starts_with('-') {
        return true;
    }
    // First token is a flag. The only flags that may legally lead a native
    // invocation are the global ones (-v/--verbose, -c/--config); any other
    // leading flag (-W, -w, -j, -M, -D, …) can only be sphinx-build mode —
    // scanning further would misread a positional OUTPUTDIR named `build`.
    if !matches!(first, "-v" | "--verbose" | "-c" | "--config") {
        return true;
    }
    // Ambiguous global flag first: a native subcommand token appearing later
    // keeps the invocation native. (A compat call like `-c conf src build`
    // loses this toss-up — documented with the `./build` escape hatch.)
    !args
        .iter()
        .skip(1)
        .any(|a| matches!(a.as_str(), "build" | "clean" | "stats"))
}

/// Initialize logging with a default filter that a pre-set RUST_LOG overrides
/// (ROADMAP M1: never clobber the user's RUST_LOG).
fn init_logging(default_level: &str) {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default_level))
        .init();
}

/// Everything `run_build` needs, from either CLI mode.
struct RunArgs {
    source: PathBuf,
    output: PathBuf,
    /// Explicit config file (native --config)
    config_file: Option<PathBuf>,
    /// sphinx-build -c: a directory containing conf.py (or a file)
    confdir: Option<PathBuf>,
    overrides: Vec<(String, String)>,
    html_defines: Vec<(String, String)>,
    tags: Vec<String>,
    doctree_dir: Option<PathBuf>,
    nitpicky: bool,
    jobs: Option<usize>,
    clean_first: bool,
    incremental: bool,
    fresh_env: bool,
    fail_on_warning: bool,
    warning_file: Option<PathBuf>,
    /// Print sphinx-build's closing "The HTML pages are in …" line
    print_final_location: bool,
}

impl RunArgs {
    /// Baseline with no flags set; call sites fill in what their mode uses
    /// via struct-update syntax.
    fn base(source: PathBuf, output: PathBuf) -> Self {
        Self {
            source,
            output,
            config_file: None,
            confdir: None,
            overrides: vec![],
            html_defines: vec![],
            tags: vec![],
            doctree_dir: None,
            nitpicky: false,
            jobs: None,
            clean_first: false,
            incremental: false,
            fresh_env: false,
            fail_on_warning: false,
            warning_file: None,
            print_final_location: false,
        }
    }
}

/// Shared build driver for both CLI modes. Returns the process exit code.
async fn run_build(args: RunArgs) -> Result<i32> {
    let mut config = if let Some(ref config_path) = args.config_file {
        BuildConfig::from_file(config_path)?
    } else if let Some(ref confdir) = args.confdir {
        let conf_path = if confdir.is_dir() {
            confdir.join("conf.py")
        } else {
            confdir.clone()
        };
        if !conf_path.exists() {
            anyhow::bail!(
                "config directory doesn't contain a conf.py file ({})",
                confdir.display()
            );
        }
        BuildConfig::from_file(&conf_path)?
    } else {
        // Try to auto-detect configuration (including conf.py)
        BuildConfig::auto_detect(&args.source)?
    };

    // Override config with CLI arguments (config file < -D < dedicated flags).
    // Ignored overrides are warnings in sphinx-build too — they must count
    // toward -W and reach the -w file.
    let mut config_warnings: Vec<String> = Vec::new();
    for (key, value) in &args.overrides {
        if let Some(message) = config.apply_override(key, value)? {
            warn!("{}", message);
            config_warnings.push(message);
        }
    }
    for (key, value) in &args.html_defines {
        config
            .html_context
            .insert(key.clone(), serde_json::Value::String(value.clone()));
    }
    config.tags.extend(args.tags.iter().cloned());
    if args.nitpicky {
        config.nitpicky = true;
    }
    if args.doctree_dir.is_some() {
        config.doctree_dir = args.doctree_dir.clone();
    }
    if args.fail_on_warning {
        config.fail_on_warning = true;
    }

    // Save the fail_on_warning flag before moving config
    let should_fail_on_warning = config.fail_on_warning;

    let mut builder = SphinxBuilder::new(config, args.source, args.output.clone())?;

    if let Some(jobs) = args.jobs {
        builder.set_parallel_jobs(jobs);
    }

    if args.clean_first {
        builder.clean().await?;
    }

    if args.fresh_env {
        builder.fresh_env()?;
    }

    if args.incremental {
        builder.enable_incremental();
    }

    let stats = builder.build().await?;

    // Handle warning file output if specified
    let mut warning_file_handle = if let Some(ref warning_file_path) = args.warning_file {
        // Create parent directories if they don't exist
        if let Some(parent) = warning_file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Some(
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(warning_file_path)?,
        )
    } else {
        None
    };

    // Config-time warnings (already logged when they occurred) go to the
    // warning file and count toward the totals below.
    if let Some(ref mut file) = warning_file_handle {
        for message in &config_warnings {
            writeln!(file, "WARNING: {}", message)?;
        }
    }
    let total_warnings = stats.warnings + config_warnings.len();

    // Print warnings in Sphinx-like format
    for warning in &stats.warning_details {
        let warning_msg = warning.render();

        // Write to warning file if specified
        if let Some(ref mut file) = warning_file_handle {
            writeln!(file, "{}", warning_msg)?;
        }

        warn!("{}", warning_msg);
    }

    // Print errors in Sphinx-like format
    for error in &stats.error_details {
        let file_path = error.file.display();
        let line_info = if let Some(line) = error.line {
            format!(":{}", line)
        } else {
            String::new()
        };
        let error_msg = format!("{}{}: ERROR: {}", file_path, line_info, error.message);

        // Write to warning file if specified (errors also go to warning file in Sphinx)
        if let Some(ref mut file) = warning_file_handle {
            writeln!(file, "{}", error_msg)?;
        }

        eprintln!("{}", error_msg);
    }

    // Flush and close the warning file
    if let Some(mut file) = warning_file_handle {
        file.flush()?;
    }

    let plural = |n: usize| if n == 1 { "" } else { "s" };

    // sphinx-build 9.1 parity: -W collects every warning (keep-going is the
    // default since Sphinx 8.1) and fails the build afterwards.
    if should_fail_on_warning && total_warnings > 0 {
        eprintln!(
            "build finished with problems, {} warning{} (with warnings treated as errors).",
            total_warnings,
            plural(total_warnings)
        );
        return Ok(1);
    }

    // Deliberately STRICTER than sphinx-build here: real sphinx-build 9.1
    // exits 0 on logged ERRORs unless -W is set. Unreadable or unparseable
    // sources failing CI silently is exactly the M1 trust problem, so builds
    // with errors exit 1.
    if stats.errors > 0 {
        eprintln!(
            "build finished with problems, {} error{}{}.",
            stats.errors,
            plural(stats.errors),
            if total_warnings > 0 {
                format!(", {} warning{}", total_warnings, plural(total_warnings))
            } else {
                String::new()
            }
        );
        return Ok(1);
    }

    // Print final summary
    if total_warnings > 0 {
        warn!(
            "build succeeded, {} warning{}.",
            total_warnings,
            plural(total_warnings)
        );
    }

    info!("Build completed successfully!");
    info!("Files processed: {}", stats.files_processed);
    info!("Files skipped: {}", stats.files_skipped);
    info!("Cache hits: {}", stats.cache_hits);
    info!("Build time: {:?}", stats.build_time);
    info!("Output size: {} MB", stats.output_size_mb);

    if args.print_final_location {
        println!("\nThe HTML pages are in {}.", args.output.display());
    }

    Ok(0)
}

/// sphinx-build compatible mode entry point. Returns the process exit code.
async fn run_sphinx_build_mode(sb: SphinxBuildCli) -> i32 {
    let default_level = if sb.quiet {
        "warn"
    } else {
        match sb.verbose {
            0 => "info",
            1 => "debug",
            _ => "trace",
        }
    };
    init_logging(default_level);

    // Refuse to build or clean into the source tree (sphinx-build parity:
    // make-mode errors "is same as source directory!" / "directory contains
    // source directory!" with exit 1).
    if let Some(reason) = output_overlaps_source(&sb.sourcedir, &sb.outputdir) {
        eprintln!("Error: {reason}");
        return 1;
    }

    // Make-mode dispatch first: -M MODE decides what happens at all.
    // Make-mode keeps the cache beside the html dir (sphinx-build puts
    // doctrees there), never inside the deployable OUTPUTDIR/html tree.
    let mut make_mode_cache_dir = None;
    let (output, is_build) = if let Some(ref mode) = sb.make_mode {
        match mode.as_str() {
            "html" => {
                make_mode_cache_dir = Some(sb.outputdir.join(".sphinx-ultra-cache"));
                (sb.outputdir.join("html"), true)
            }
            "clean" => {
                // sphinx-build returns 0 silently when there is nothing to clean.
                if sb.outputdir.exists() {
                    println!("Removing everything under '{}'...", sb.outputdir.display());
                    if let Err(e) = remove_dir_contents(&sb.outputdir) {
                        eprintln!("Error: {e:#}");
                        return 2;
                    }
                }
                return 0;
            }
            other => {
                eprintln!(
                    "make-mode target '{other}' is not supported yet — sphinx-ultra 0.4 supports 'html' and 'clean' only"
                );
                return 2;
            }
        }
    } else {
        if sb.builder != "html" {
            eprintln!(
                "builder '{}' is not supported yet — sphinx-ultra 0.4 supports 'html' only",
                sb.builder
            );
            return 2;
        }
        (sb.outputdir.clone(), true)
    };
    debug_assert!(is_build);

    // sphinx-build requires a configuration; silently building a mistyped
    // path with defaults would be the opposite of drop-in behavior. (The
    // sphinx-ultra.* probes are our native extension of the rule.)
    if sb.confdir.is_none()
        && ![
            "conf.py",
            "sphinx-ultra.yaml",
            "sphinx-ultra.yml",
            "sphinx-ultra.json",
        ]
        .iter()
        .any(|f| sb.sourcedir.join(f).exists())
    {
        eprintln!(
            "Error: config directory doesn't contain a conf.py file ({})",
            sb.sourcedir.display()
        );
        return 2;
    }

    info!(
        "Running sphinx-ultra v{} (sphinx-build compatible mode)",
        env!("CARGO_PKG_VERSION")
    );

    if !sb.filenames.is_empty() {
        warn!("building specific files is not supported yet; building the full project");
    }

    // sphinx-build is incremental by default; -E discards the saved
    // environment first and -a rewrites everything.
    //
    // Deliberate divergence, kept from M1: sphinx's `-a` only forces the
    // *write* set to every document and still reads incrementally, while
    // here it switches the document cache off, so the build reads
    // everything as well (and reports `Cache hits: 0`, which
    // `sphinx_build_incremental_by_default_and_fresh_env` pins). Reading
    // more than sphinx would is slower, never wrong: every document read is
    // re-derived from its own source. It stays this way until the write set
    // is observable — the real HTML writer, which compares each output file
    // against its sources; that is the wave that can separate "write all"
    // from "read all" and revisit the pin.
    let incremental = !sb.write_all;

    let args = RunArgs {
        confdir: sb.confdir.clone(),
        overrides: sb.define.clone(),
        html_defines: sb.html_define.clone(),
        tags: sb.tags.clone(),
        doctree_dir: sb.doctreedir.clone().or(make_mode_cache_dir),
        nitpicky: sb.nitpicky,
        jobs: sb.jobs,
        incremental,
        fresh_env: sb.fresh_env,
        fail_on_warning: sb.fail_on_warning,
        warning_file: sb.warning_file.clone(),
        print_final_location: !sb.quiet,
        ..RunArgs::base(sb.sourcedir.clone(), output)
    };

    match run_build(args).await {
        Ok(code) => code,
        Err(e) => {
            // sphinx-build exits 2 on exceptions; -T controls traceback detail.
            if sb.traceback {
                eprintln!("Error: {e:?}");
            } else {
                eprintln!("Error: {e:#}");
            }
            2
        }
    }
}

/// Refuse output locations that would clobber the sources: equal dirs, or an
/// output dir that is an ancestor of the source dir. Compared canonicalized
/// so `docs` vs `./docs/` still match; a nonexistent output dir cannot
/// overlap an existing source.
fn output_overlaps_source(source: &std::path::Path, output: &std::path::Path) -> Option<String> {
    let source = source.canonicalize().ok()?;
    let output = output.canonicalize().ok()?;
    if source == output {
        Some(format!(
            "'{}' is same as source directory!",
            output.display()
        ))
    } else if source.starts_with(&output) {
        Some(format!(
            "'{}' directory contains source directory!",
            output.display()
        ))
    } else {
        None
    }
}

/// Remove the contents of `dir` but keep the directory itself
/// (sphinx-build -M clean behavior).
fn remove_dir_contents(dir: &std::path::Path) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();

    if wants_sphinx_build_mode(&raw_args) {
        let sb = SphinxBuildCli::parse();
        let code = run_sphinx_build_mode(sb).await;
        std::process::exit(code);
    }

    let cli = Cli::parse();

    // Initialize logging (a pre-set RUST_LOG wins over -v)
    init_logging(if cli.verbose { "debug" } else { "info" });

    info!("Sphinx Ultra Builder v{}", env!("CARGO_PKG_VERSION"));

    match cli.command {
        Commands::Build {
            source,
            output,
            jobs,
            clean,
            incremental,
            fail_on_warning,
            warning_file,
        } => {
            let code = run_build(RunArgs {
                config_file: cli.config,
                jobs,
                clean_first: clean,
                incremental,
                fail_on_warning,
                warning_file,
                ..RunArgs::base(source, output)
            })
            .await?;
            if code != 0 {
                std::process::exit(code);
            }
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
            let stats = analyze_project(&source).await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("sphinx-ultra")
            .chain(args.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn dispatch_native_subcommands() {
        assert!(!wants_sphinx_build_mode(&argv(&["build", "-s", "docs"])));
        assert!(!wants_sphinx_build_mode(&argv(&["clean"])));
        assert!(!wants_sphinx_build_mode(&argv(&["stats"])));
        assert!(!wants_sphinx_build_mode(&argv(&["--help"])));
        assert!(!wants_sphinx_build_mode(&argv(&["-V"])));
        assert!(!wants_sphinx_build_mode(&argv(&[])));
    }

    #[test]
    fn dispatch_native_global_flags_before_subcommand() {
        assert!(!wants_sphinx_build_mode(&argv(&[
            "--config", "conf.py", "build", "-s", "docs"
        ])));
        assert!(!wants_sphinx_build_mode(&argv(&["--verbose", "build"])));
    }

    #[test]
    fn dispatch_positional_paths_go_compat() {
        assert!(wants_sphinx_build_mode(&argv(&["docs", "_build"])));
        assert!(wants_sphinx_build_mode(&argv(&["./build", "out"])));
        assert!(wants_sphinx_build_mode(&argv(&["/abs/src", "/abs/out"])));
    }

    #[test]
    fn dispatch_compat_only_flags_win() {
        assert!(wants_sphinx_build_mode(&argv(&[
            "-M", "html", "src", "out"
        ])));
        assert!(wants_sphinx_build_mode(&argv(&[
            "-b", "html", "src", "out"
        ])));
        // even with an output dir literally named "build"
        assert!(wants_sphinx_build_mode(&argv(&[
            "-M", "html", "src", "build"
        ])));
    }

    #[test]
    fn dispatch_nonglobal_leading_flag_is_compat_even_with_build_positional() {
        // -W/-w/-j can't lead a native invocation, so an OUTPUTDIR literally
        // named `build` must not drag these into the native parser.
        assert!(wants_sphinx_build_mode(&argv(&["-W", "docs", "build"])));
        assert!(wants_sphinx_build_mode(&argv(&[
            "-w", "log", "docs", "build"
        ])));
        assert!(wants_sphinx_build_mode(&argv(&[
            "-j", "2", "docs", "stats"
        ])));
    }

    #[test]
    fn jobs_parser_accepts_auto_and_numbers() {
        assert!(parse_jobs("auto").unwrap() >= 1);
        assert_eq!(parse_jobs("4").unwrap(), 4);
        assert!(parse_jobs("many").is_err());
    }

    #[test]
    fn key_val_parser() {
        assert_eq!(
            parse_key_val("a=b").unwrap(),
            ("a".to_string(), "b".to_string())
        );
        assert_eq!(
            parse_key_val("exclude_patterns=a,b").unwrap(),
            ("exclude_patterns".to_string(), "a,b".to_string())
        );
        assert!(parse_key_val("novalue").is_err());
        assert!(parse_key_val("=x").is_err());
    }
}
