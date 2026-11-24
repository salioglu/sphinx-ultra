use anyhow::Result;
use clap::{ArgAction, Parser, Subcommand};
use log::{info, warn};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use sphinx_ultra::diagnostics::{Category, Diagnostic, Location, Severity, WarningFilter};
use sphinx_ultra::error::ConfigError;
use sphinx_ultra::rst::SphinxCrash;
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

    /// Builder to use (only 'html' is supported so far)
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

    /// Define a tag, which `.. only::` reads
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
    /// sphinx-build's status stream, stdout: the closing summary and the
    /// "The HTML pages are in …" line, which `-q` silences. Off in the
    /// native subcommand, which reports its summary on its own channels.
    status_stream: bool,
    /// sphinx-build compatible mode: Sphinx's warning stream (bare records,
    /// the same bytes on stderr and in `-w`, which opens before the
    /// configuration loads), its `Configuration error!` report, its closing
    /// summary on the status stream, and no M1 validation pass. The native
    /// `build` subcommand keeps its env_logger output.
    sphinx_build_mode: bool,
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
            status_stream: false,
            sphinx_build_mode: false,
        }
    }
}

/// The warning stream's two halves: stderr, and the `-w` file when one is
/// given — Sphinx's `TeeStripANSI` (`sphinx/cmd/build.py:333-361`).
struct WarningStream {
    /// sphinx-build mode prints each record bare, the same bytes the file
    /// gets; the native subcommand logs it through env_logger.
    bare: bool,
    show_warning_types: bool,
    file: Option<File>,
}

impl WarningStream {
    fn new(bare: bool) -> Self {
        Self {
            bare,
            show_warning_types: true,
            file: None,
        }
    }

    /// Create (or truncate) the `-w` file, and its directory.
    fn open(&mut self, path: Option<&Path>) -> Result<()> {
        let Some(path) = path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.file = Some(
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)?,
        );
        Ok(())
    }

    /// One record, `render(show_warning_types) + "\n"`, to both halves.
    fn record(&mut self, record: &Diagnostic) -> Result<()> {
        let line = record.render(self.show_warning_types);
        if self.bare {
            to_stderr(&format!("{line}\n"));
        } else {
            warn!("{line}");
        }
        self.write_line(&line)
    }

    /// A per-file failure, printed bare in either mode.
    fn error(&mut self, error: &Diagnostic) -> Result<()> {
        let line = error.render(self.show_warning_types);
        to_stderr(&format!("{line}\n"));
        self.write_line(&line)
    }

    /// One record to the file alone (the native subcommand logged it to
    /// stderr when it happened).
    fn write_record(&mut self, record: &Diagnostic) -> Result<()> {
        self.write_line(&record.render(self.show_warning_types))
    }

    /// Text outside the record grammar, verbatim, to both halves.
    fn raw(&mut self, text: &str) -> Result<()> {
        to_stderr(text);
        if let Some(file) = &mut self.file {
            file.write_all(text.as_bytes())?;
        }
        Ok(())
    }

    fn write_line(&mut self, line: &str) -> Result<()> {
        if let Some(file) = &mut self.file {
            writeln!(file, "{line}")?;
        }
        Ok(())
    }
}

/// Writes `text` to stderr in one call. A closed or broken stderr loses the
/// text but does not abort the build — the `-w` file and the exit status
/// still carry the outcome — so a failed write is ignored, as env_logger
/// ignores it.
fn to_stderr(text: &str) {
    let _ = std::io::stderr().write_all(text.as_bytes());
}

/// A loaded configuration, the directory it was read from, and the records
/// Sphinx logs while setting it up: ignored `-D` overrides, then the
/// `config-inited` checks, up to a `config-inited` handler that crashes
/// Sphinx (decision 13), if one does.
struct Configured {
    config: BuildConfig,
    confdir: PathBuf,
    records: Vec<Diagnostic>,
    /// The crash of a `config-inited` handler, which ends the records.
    crash: Option<SphinxCrash>,
}

/// `SOURCE_DATE_EPOCH`, when it is set, as Python's `os.getenv` reads it
/// (a value that is not UTF-8 read lossily).
fn source_date_epoch() -> Option<String> {
    std::env::var_os("SOURCE_DATE_EPOCH").map(|value| value.to_string_lossy().into_owned())
}

/// A configuration-time record: Sphinx logs these with neither a location
/// nor a type.
fn config_record(message: String) -> Diagnostic {
    Diagnostic {
        location: Location::None,
        severity: Severity::Warning,
        message,
        category: Category::None,
    }
}

/// `path` the way Sphinx spells a directory in an error message:
/// `Path.resolve()`, absolute with symlinks resolved.
fn resolved(path: &Path) -> PathBuf {
    sphinx_ultra::utils::canonicalize_simplified(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Sphinx's report of a `ConfigError` (`handle_exception`,
/// `sphinx/_cli/util/errors.py:162-245`) as the HTML oracle records it: the
/// traceback context reduced to the exception line, which is all this crate
/// can say. sphinx-build mode writes it to both halves of the stream, as
/// Sphinx writes it to its error stream, the tee of stderr and `-w`.
fn config_error_report(error: &anyhow::Error) -> String {
    format!("\nConfiguration error!\n\n    sphinx.errors.ConfigError: {error:#}\n")
}

/// Load the configuration and apply the command line to it. A failure here
/// is a configuration error, which sphinx-build mode reports the way Sphinx
/// reports its `ConfigError`: `Configuration error!`, exit 2.
fn configure(args: &RunArgs) -> Result<Configured> {
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
                resolved(confdir).display()
            );
        }
        BuildConfig::from_file(&conf_path)?
    } else {
        // sphinx-build requires a configuration; silently building a
        // mistyped path with defaults would be the opposite of drop-in
        // behavior. (The sphinx-ultra.* files are this crate's native
        // extension of the rule; the native subcommand falls back to
        // defaults.)
        if args.sphinx_build_mode
            && ![
                "conf.py",
                "sphinx-ultra.yaml",
                "sphinx-ultra.yml",
                "sphinx-ultra.json",
            ]
            .iter()
            .any(|file| args.source.join(file).exists())
        {
            anyhow::bail!(
                "config directory doesn't contain a conf.py file ({})",
                resolved(&args.source).display()
            );
        }
        BuildConfig::auto_detect(&args.source)?
    };

    // Override config with CLI arguments (config file < -D < dedicated flags).
    // Ignored overrides are warnings in sphinx-build too — they must count
    // toward -W and reach the -w file.
    let mut records = Vec::new();
    for (key, value) in &args.overrides {
        if let Some(message) = config.apply_override(key, value)? {
            records.push(config_record(message));
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

    let confdir = match (&args.config_file, &args.confdir) {
        (Some(config_file), _) => config_file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| args.source.clone()),
        (None, Some(confdir)) if confdir.is_dir() => confdir.clone(),
        (None, Some(conf_py)) => conf_py
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| args.source.clone()),
        (None, None) => args.source.clone(),
    };
    // The `config-inited` handlers, in Sphinx's order (priority, then
    // registration), over the finished configuration — after conf.py AND
    // after every override; a rejected value only warns, and the build
    // continues with it: `deprecate_source_encoding` (790), then, at 800,
    // the HTML builder's logo and favicon checks, which drop a missing
    // file's setting (`sphinx.builders.html` registers them before
    // `sphinx.config` registers its own), ...
    records.extend(
        config
            .validate_source_encoding()
            .into_iter()
            .map(config_record),
    );
    records.extend(
        config
            .validate_html_logo_and_favicon(&confdir)
            .into_iter()
            .map(config_record),
    );
    // ... `correct_copyright_year`, which crashes on a `SOURCE_DATE_EPOCH`
    // `int()` cannot read, ending the records ...
    let crash = source_date_epoch()
        .as_deref()
        .and_then(sphinx_ultra::read::source_date_epoch_config_crash);
    // ... then the type checks and `check_primary_domain`.
    if crash.is_none() {
        records.extend(
            config
                .validate_confval_types()
                .into_iter()
                .map(config_record),
        );
    }
    Ok(Configured {
        config,
        confdir,
        records,
        crash,
    })
}

/// Shared build driver for both CLI modes. Returns the process exit code.
async fn run_build(args: RunArgs) -> Result<i32> {
    let sphinx_mode = args.sphinx_build_mode;
    let mut stream = WarningStream::new(sphinx_mode);
    // sphinx-build opens the warning file before the application, and so
    // the configuration, exists (`_parse_logging`): a configuration error
    // lands in it too. The native subcommand opens it once the build is done.
    if sphinx_mode {
        stream.open(args.warning_file.as_deref())?;
    }

    let Configured {
        mut config,
        confdir,
        records: config_records,
        crash: config_crash,
    } = match configure(&args) {
        Ok(configured) => configured,
        Err(error) if sphinx_mode => {
            stream.raw(&config_error_report(&error))?;
            return Ok(2);
        }
        Err(error) => return Err(error),
    };

    // Sphinx reads `SOURCE_DATE_EPOCH` first while it sets its application
    // up, before it logs anything about the configuration: a value it
    // cannot read crashes it with no record before the report (decision
    // 13), in both modes.
    if let Some(crash) = source_date_epoch()
        .and_then(|epoch| sphinx_ultra::read::source_date_epoch_import_crash(&epoch))
    {
        if !sphinx_mode {
            stream.open(args.warning_file.as_deref())?;
        }
        stream.raw(&crash.report())?;
        return Ok(2);
    }

    let filter = WarningFilter::from_config(&config);
    stream.show_warning_types = filter.show_warning_types();
    // Configuration records have no type, so `suppress_warnings` keeps them
    // all; they go through the filter like every other record.
    let config_records: Vec<Diagnostic> = config_records
        .into_iter()
        .filter(|record| !filter.is_suppressed(&record.category))
        .collect();

    if sphinx_mode {
        // M1's directive and role validators report problems Sphinx never
        // prints — the parser already holds docutils' own messages for the
        // same input — so sphinx-build mode never runs them.
        config.validate_directives = false;
        // Sphinx points docutils at `<confdir>/docutils.conf`
        // (`patch_docutils`, `sphinx/util/docutils.py:217-229`). This crate
        // reads no docutils.conf, and says so outside the warning stream:
        // not a record, not in the `-w` file, not counted by `-W`.
        let docutils_conf = confdir.join("docutils.conf");
        if docutils_conf.is_file() {
            to_stderr(&format!(
                "NOTICE: docutils.conf is not read by sphinx-ultra: {}\n",
                docutils_conf.display()
            ));
        }
    }

    // Configuration records open the stream: Sphinx logs them at
    // `config-inited`, before the builder exists.
    for record in &config_records {
        if sphinx_mode {
            stream.record(record)?;
        } else {
            warn!("{}", record.message);
        }
    }
    // A `config-inited` handler that crashed Sphinx ends the build after
    // the records logged before it (decision 13).
    if let Some(crash) = config_crash {
        if !sphinx_mode {
            stream.open(args.warning_file.as_deref())?;
            for record in &config_records {
                stream.write_record(record)?;
            }
        }
        stream.raw(&crash.report())?;
        return Ok(2);
    }

    // `config-inited`'s last handler, the one `sphinx.ext.intersphinx`
    // registers: an `ERROR` record for each rejected `intersphinx_mapping`
    // entry, then the `ConfigError` (`ext/intersphinx/_load.py:64-136`).
    if let Err(error) = config.init_intersphinx_mapping() {
        if !sphinx_mode {
            for message in &error.errors {
                log::error!("{message}");
            }
            return Err(error.into());
        }
        for message in &error.errors {
            stream.record(&Diagnostic {
                location: Location::None,
                severity: Severity::Error,
                message: message.clone(),
                category: Category::None,
            })?;
        }
        stream.raw(&config_error_report(&anyhow::Error::new(error)))?;
        return Ok(2);
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

    let stats = match builder.build().await {
        Ok(stats) => stats,
        // A `ConfigError` raised once the build has started (the intersphinx
        // inventories' invariants, checked as they load) gets the same
        // report as one raised while the configuration loads, after the
        // configuration records, as in Sphinx.
        Err(error) if sphinx_mode && error.downcast_ref::<ConfigError>().is_some() => {
            stream.raw(&config_error_report(&error))?;
            return Ok(2);
        }
        // An input that crashes Sphinx 9.1.0 fails the build as Sphinx's
        // crash does (decision 13), in both modes: the records Sphinx
        // prints before the crash — the configuration's, then the build's
        // so far — then a report naming the exception, and exit status 2.
        Err(error) if error.downcast_ref::<SphinxCrash>().is_some() => {
            let crash = error.downcast_ref::<SphinxCrash>().expect("matched above");
            if !sphinx_mode {
                stream.open(args.warning_file.as_deref())?;
                for record in &config_records {
                    stream.write_record(record)?;
                }
            }
            for record in &builder.recorded_warnings() {
                stream.record(record)?;
            }
            stream.raw(&crash.report())?;
            return Ok(2);
        }
        Err(error) => return Err(error),
    };

    if !sphinx_mode {
        stream.open(args.warning_file.as_deref())?;
        for record in &config_records {
            stream.write_record(record)?;
        }
    }
    // The build's records, in the order Sphinx emits them, then the
    // per-file failures (which also reach the warning file in Sphinx).
    for record in &stats.warning_details {
        stream.record(record)?;
    }
    for error in &stats.error_details {
        stream.error(&Diagnostic::from(error))?;
    }

    let total_warnings = config_records.len() + stats.warnings;
    let plural = |n: usize| if n == 1 { "" } else { "s" };

    // Sphinx logs its closing summary at INFO, on its status stream
    // (`sphinx/application.py:452-481`): stdout, which `-q` silences, and
    // never the warning stream or `-w`. The native subcommand keeps its own
    // channels.
    let status = |line: &str| {
        if args.status_stream {
            println!("{line}");
        }
    };

    // The exit status follows Sphinx's (`sphinx/cmd/build.py:395-452`,
    // `sphinx/application.py:453-481`): records never change it, whatever
    // their severity, except under `-W`, which fails the build once every
    // record is out (keep-going is the default since Sphinx 8.1).
    if should_fail_on_warning && total_warnings > 0 {
        let summary = format!(
            "build finished with problems, {} warning{} (with warnings treated as errors).",
            total_warnings,
            plural(total_warnings)
        );
        if sphinx_mode {
            status(&summary);
        } else {
            eprintln!("{summary}");
        }
        return Ok(1);
    }

    // Deliberately STRICTER than sphinx-build here, and only here: a
    // genuine per-file failure (an unreadable or undecodable source, an
    // output that could not be written) exits 1. Such failures are never
    // records of the warning stream; letting them pass CI silently is
    // exactly the M1 trust problem.
    if stats.errors > 0 {
        let summary = format!(
            "build finished with problems, {} error{}{}.",
            stats.errors,
            plural(stats.errors),
            if total_warnings > 0 {
                format!(", {} warning{}", total_warnings, plural(total_warnings))
            } else {
                String::new()
            }
        );
        if sphinx_mode {
            status(&summary);
        } else {
            eprintln!("{summary}");
        }
        return Ok(1);
    }

    // Print final summary
    if total_warnings > 0 {
        let summary = format!(
            "build succeeded, {} warning{}.",
            total_warnings,
            plural(total_warnings)
        );
        if sphinx_mode {
            status(&summary);
        } else {
            warn!("{summary}");
        }
    } else if sphinx_mode {
        status("build succeeded.");
    }

    info!("Build completed successfully!");
    info!("Files processed: {}", stats.files_processed);
    info!("Files skipped: {}", stats.files_skipped);
    info!("Cache hits: {}", stats.cache_hits);
    info!("Build time: {:?}", stats.build_time);
    info!("Output size: {} MB", stats.output_size_mb);

    // Sphinx's epilog, after an empty status line.
    status("");
    status(&format!("The HTML pages are in {}.", args.output.display()));

    Ok(0)
}

/// sphinx-build compatible mode entry point. Returns the process exit code.
async fn run_sphinx_build_mode(sb: SphinxBuildCli) -> i32 {
    // Sphinx writes its status to stdout, so its stderr carries the warning
    // stream alone (decision 8): this crate's own progress log stays off it
    // unless `-v` asks for it (`-q` or not), and `RUST_LOG` still wins.
    let default_level = match sb.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
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
                    "make-mode target '{other}' is not supported yet — sphinx-ultra supports 'html' and 'clean' only"
                );
                return 2;
            }
        }
    } else {
        if sb.builder != "html" {
            eprintln!(
                "builder '{}' is not supported yet — sphinx-ultra supports 'html' only",
                sb.builder
            );
            return 2;
        }
        (sb.outputdir.clone(), true)
    };
    debug_assert!(is_build);

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
        status_stream: !sb.quiet,
        sphinx_build_mode: true,
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
    let source = sphinx_ultra::utils::canonicalize_simplified(source).ok()?;
    let output = sphinx_ultra::utils::canonicalize_simplified(output).ok()?;
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
