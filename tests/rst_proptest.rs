//! Property tests for the M2 RST parser (ROADMAP §10.6: the parser is total
//! — it never panics on arbitrary input; problems become system_message
//! nodes). First real use of the reserved proptest dev-dependency.
//!
//! Wave 4.5 extended the sweep over the surfaces this milestone added — the
//! py-domain object descriptions and their signature grammar, the std-domain
//! descriptions wave 4 left uncovered, the file-inserting `include` /
//! `literalinclude` family, and the annotation parser reached directly.
//!
//! GENERATOR RULE (learned the expensive way in task 15): a `.`-based regex
//! generator never produces a newline unless it carries `(?s)`, so a sweep
//! written without it silently tests only single-line inputs — exactly the
//! shape these multi-line grammars are least likely to break on. Every
//! free-text generator below is either `(?s)`-flagged or built by joining
//! generated lines with `\n`.
//!
//! M2 wave 5 sub-project 1 (the read transforms) made the property the
//! Sphinx read's: every generator's input also goes through
//! [`parse_and_transform`] — the parse, then every read transform — under
//! a drawn [`TransformConfig`], and the result is printed ([`sphinx_read`]).
//! Two generators are new: transform-shaped documents whose names collide
//! (substitution, target, footnote and citation definitions and references,
//! cycles and case variants included), and the deep-nesting sweep, which
//! nests containers past the parser's 200-level guard and hands the
//! transforms the deepest trees the parser builds. Termination is part of
//! the property — a transform that never ends is as much a bug as one that
//! panics — so every case runs under a per-case time bound
//! ([`CASE_TIMEOUT_MS`]).

use std::path::Path;
use std::sync::OnceLock;

use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use sphinx_ultra::error::BuildWarning;
use sphinx_ultra::py::annotations::{parse_annotation, PyRefContext};
use sphinx_ultra::py::PySigConfig;
use sphinx_ultra::rst::{on_parse_stack, parse_rst, ParseOptions};
use sphinx_ultra::transforms::{parse_and_transform, BuildDate, TransformConfig};

/// The time one case may take before proptest kills it and reports it — a
/// hang, or work growing faster than the document: the slowest of 2,048
/// deep-sweep cases took 6.1 s in a debug build (a 260-level case,
/// `PROPTEST_RNG_SEED=20261001`, [`CASE_TIMES`]; 263 ms the mean of the
/// 515 cases 210 levels deep or more), so the bound leaves ten times that
/// for a loaded machine. Setting it runs each test's cases in a forked
/// child process (proptest's `timeout`, which implies `fork`).
const CASE_TIMEOUT_MS: u32 = 60_000;

/// What every sweep here runs under: 512 cases (`PROPTEST_CASES`
/// overrides), the per-case bound ([`CASE_TIMEOUT_MS`]), and failing seeds
/// kept beside this file, in `tests/rst_proptest.proptest-regressions`
/// (gitignored). That is where proptest's default would put them too, but
/// only after looking for a `lib.rs` or `main.rs` above this file to mirror
/// its path from, finding none — an integration test has neither — and
/// saying so on every run (`FileFailurePersistence::SourceParallel set, but
/// failed to find lib.rs or main.rs`); naming the place directly is silent.
/// A seed replays only as long as the strategies stay as they were, so a
/// failure a sweep finds is kept as a named test instead (as
/// [`a_self_wrapping_substitution_stops_at_the_depth_limit`] is).
fn sweep_config() -> ProptestConfig {
    ProptestConfig {
        cases: 512,
        timeout: CASE_TIMEOUT_MS,
        failure_persistence: Some(Box::new(FileFailurePersistence::WithSource(
            "proptest-regressions",
        ))),
        ..ProptestConfig::default()
    }
}

/// Set (to anything), every deep-sweep case prints how many levels it
/// nests and how long its read took — to stderr, so run with
/// `--nocapture` — which is how a run finds its slowest case against
/// [`CASE_TIMEOUT_MS`].
const CASE_TIMES: &str = "RST_PROPTEST_CASE_TIMES";

/// The Sphinx read of `s` under `config`: the parse, then the read
/// transforms ([`parse_and_transform`]), the tree printed as `pformat` and
/// each record as the build renders it. Totality is the whole property:
/// whatever the parser made of `s`, every transform runs to its end.
fn sphinx_read(s: &str, o: &ParseOptions, config: &TransformConfig) {
    let (tree, records) = parse_and_transform(s, o, config);
    let _ = tree.root.pformat();
    for record in &records {
        let _ = BuildWarning::from_diagnostic(record, "index.rst".into()).render();
    }
}

/// The read transforms' configuration, drawn over every key a transform
/// branches on: `keep_warnings` (FilterSystemMessages); SmartQuotes on or
/// off, with actions docutils knows and arbitrary ones; a language with a
/// quote table, a regional one, one `smartquotes_excludes` names, one with
/// no table (the `No smart quotes defined` warning) and arbitrary text; a
/// builder the excludes name; `|version|`/`|release|`/`|today|` text, an
/// arbitrary `today_fmt` and any instant as the build date.
fn transform_config() -> impl Strategy<Value = TransformConfig> {
    (
        (any::<bool>(), any::<bool>()),
        prop_oneof![
            Just("qDe".to_string()),
            Just("q".to_string()),
            Just("De".to_string()),
            Just("1".to_string()),
            Just("2".to_string()),
            Just("3".to_string()),
            Just("-1".to_string()),
            Just("0".to_string()),
            Just("qbBdDiew".to_string()),
            "(?s).{0,6}",
        ],
        prop_oneof![
            Just("en".to_string()),
            Just("de".to_string()),
            Just("fr".to_string()),
            Just("de-CH".to_string()),
            Just("ja".to_string()),
            Just("zh_CN".to_string()),
            Just("xx".to_string()),
            Just(String::new()),
            "(?s).{0,12}",
        ],
        prop_oneof![Just("html"), Just("dirhtml"), Just("text"), Just("man")],
        ("(?s).{0,6}", "(?s).{0,6}"),
        prop_oneof![Just(String::new()), "(?s).{0,6}"],
        proptest::option::of("(?s)(%-?[a-zA-Z%]|.){0,6}"),
        any::<i64>(),
    )
        .prop_map(
            |(
                (keep_warnings, smartquotes),
                smartquotes_action,
                language,
                builder,
                (version, release),
                today,
                today_fmt,
                epoch,
            )| TransformConfig {
                smartquotes,
                smartquotes_action,
                keep_warnings,
                language,
                builder: builder.to_string(),
                version,
                release,
                today,
                today_fmt,
                build_date: BuildDate::Epoch(epoch),
                ..TransformConfig::default()
            },
        )
}

fn opts() -> ParseOptions {
    ParseOptions {
        source_path: "<p>".into(),
        sphinx: true,
        docname: "index".into(),
        exclude_patterns: Vec::new(),
        py: Default::default(),
        srcdir: None,
        found_docs: None,
        ..Default::default()
    }
}

/// The include-argument generator for the arbitrary-path sweep: control
/// characters and newlines (the `(?s)` arms), printable text, and the
/// path shapes a `\\PC` regex can never produce — empty, absolute,
/// `..`-traversing (both into a real file outside the scratch project and
/// into nothing), and a genuine member with a traversal prefix.
fn include_argument() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "(?s).{0,40}",
        2 => "\\PC{0,40}",
        1 => "(?s)[\\x00-\\x1f\\x7f]{1,8}",
        1 => Just(String::new()),
        1 => Just("/".to_string()),
        1 => Just("/etc/hosts".to_string()),
        1 => Just("/nonexistent/dir/file.rst".to_string()),
        1 => Just("../".repeat(6) + "etc/hosts"),
        1 => Just("../../nope/../member.rst".to_string()),
        1 => Just("./sub/../member.rst".to_string()),
        1 => "(\\.\\./){0,4}[a-z.]{0,10}",
    ]
}

/// Sphinx-mode options rooted at a real source directory, so the
/// file-inserting directives take their real read path (resolution,
/// encoding, filter chain) instead of the srcdir-less short circuit.
fn opts_in(srcdir: &Path) -> ParseOptions {
    ParseOptions {
        source_path: srcdir.join("index.rst").display().to_string(),
        sphinx: true,
        docname: "index".into(),
        exclude_patterns: Vec::new(),
        py: Default::default(),
        srcdir: Some(srcdir.to_path_buf()),
        found_docs: None,
        ..Default::default()
    }
}

/// A scratch project the include sweeps read from: built once and never
/// mutated. It lives in a `static`, and Rust never drops statics, so the
/// `TempDir` guard's cleanup does not run at exit: one directory per test
/// process is deliberately LEAKED in the system temp dir (panel fix round
/// B, minor — an earlier comment claimed it was dropped with the process):
/// one for each include sweep, whose cases run in a forked child of their
/// own ([`CASE_TIMEOUT_MS`]), and one more for each child a timed-out case
/// replaces. It carries the `sphinx-ultra-proptest-` prefix so the
/// leftovers are recognizable and greppable. Its members cover the shapes the filter
/// chain branches on — markers for `:start-after:`/`:end-before:`, python
/// definitions for `:pyobject:`, an empty file (the zero-line
/// `:number-lines:` width edge), a tab-indented file (`:tab-width:` and
/// `:dedent:`), and a non-UTF-8 file (the `:encoding:` failure path).
fn scratch() -> &'static Path {
    static DIR: OnceLock<tempfile::TempDir> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::Builder::new()
            .prefix("sphinx-ultra-proptest-")
            .tempdir()
            .expect("scratch srcdir");
        let p = dir.path();
        std::fs::write(
            p.join("member.rst"),
            "before\n\n.. MARK-START\n\nmember para\n\n- a\n- b\n\n.. MARK-END\n\nafter\n",
        )
        .unwrap();
        std::fs::write(
            p.join("sample.py"),
            "import os\n\n\nclass C:\n    def m(self):\n        return 1\n\n\ndef f(a, b=2):\n    \"\"\"Doc.\"\"\"\n    return a + b\n",
        )
        .unwrap();
        std::fs::write(p.join("empty.txt"), "").unwrap();
        std::fs::write(p.join("tabs.txt"), "\tone\n\t\ttwo\n   three\n").unwrap();
        std::fs::write(p.join("latin1.txt"), [0xE9u8, b'\n']).unwrap();
        std::fs::write(p.join("index.rst"), "placeholder\n").unwrap();
        dir
    })
    .path()
}

/// One indented directive option line, or a blank/garbage line — the
/// option block itself is part of what must never panic.
fn option_line() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("   :literal:\n".to_string()),
        Just("   :code:\n".to_string()),
        Just("   :code: python\n".to_string()),
        Just("   :number-lines:\n".to_string()),
        Just("   :number-lines: 7\n".to_string()),
        Just("   :number-lines: -3\n".to_string()),
        Just("   :encoding: utf-8\n".to_string()),
        Just("   :encoding: latin-1\n".to_string()),
        Just("   :encoding: no-such-codec\n".to_string()),
        Just("   :tab-width: 4\n".to_string()),
        Just("   :tab-width: -1\n".to_string()),
        Just("   :tab-width: 0\n".to_string()),
        Just("   :start-line: 2\n".to_string()),
        Just("   :start-line: -2\n".to_string()),
        Just("   :end-line: 4\n".to_string()),
        Just("   :end-line: 0\n".to_string()),
        Just("   :start-after: MARK-START\n".to_string()),
        Just("   :start-after: nowhere\n".to_string()),
        Just("   :end-before: MARK-END\n".to_string()),
        Just("   :end-before: nowhere\n".to_string()),
        Just("   :parser: rst\n".to_string()),
        Just("   :class: c\n".to_string()),
        Just("   :name: n\n".to_string()),
        Just("   :lines: 1-3\n".to_string()),
        Just("   :lines: 3-1\n".to_string()),
        Just("   :lines: 0\n".to_string()),
        Just("   :lines: 99-\n".to_string()),
        Just("   :lineno-start: 5\n".to_string()),
        Just("   :lineno-start: -5\n".to_string()),
        Just("   :lineno-match:\n".to_string()),
        Just("   :linenos:\n".to_string()),
        Just("   :emphasize-lines: 1,3\n".to_string()),
        Just("   :emphasize-lines: 0\n".to_string()),
        Just("   :emphasize-lines: bogus\n".to_string()),
        Just("   :dedent:\n".to_string()),
        Just("   :dedent: 2\n".to_string()),
        Just("   :dedent: -2\n".to_string()),
        Just("   :prepend: head\n".to_string()),
        Just("   :append: tail\n".to_string()),
        Just("   :language: python\n".to_string()),
        Just("   :language:\n".to_string()),
        Just("   :force:\n".to_string()),
        Just("   :caption:\n".to_string()),
        Just("   :caption: cap\n".to_string()),
        Just("   :diff: sample.py\n".to_string()),
        Just("   :diff: missing.py\n".to_string()),
        Just("   :pyobject: f\n".to_string()),
        Just("   :pyobject: C.m\n".to_string()),
        Just("   :pyobject: nosuch\n".to_string()),
        Just("   :start-at: class C\n".to_string()),
        Just("   :end-at: return 1\n".to_string()),
        Just("   :no-such-option: x\n".to_string()),
        Just("   :literal\n".to_string()),
        Just("\n".to_string()),
        Just("   not an option\n".to_string()),
    ]
}

/// The file arguments the include sweeps use. A fixed set on purpose: a
/// generated path must never be able to name a file outside the scratch
/// project.
fn include_target() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("member.rst".to_string()),
        Just("sample.py".to_string()),
        Just("empty.txt".to_string()),
        Just("tabs.txt".to_string()),
        Just("latin1.txt".to_string()),
        Just("missing.rst".to_string()),
        Just("/member.rst".to_string()),
        Just("index.rst".to_string()),
        Just(String::new()),
    ]
}

proptest! {
    #![proptest_config(sweep_config())]

    #[test]
    fn parse_never_panics_on_arbitrary_input(s in "\\PC*", config in transform_config()) {
        let _ = parse_rst(&s, &opts());
        sphinx_read(&s, &opts(), &config);
    }

    #[test]
    fn parse_never_panics_on_rst_shaped_input(
        s in proptest::collection::vec(
            prop_oneof![
                Just("Title\n=====\n".to_string()),
                Just("=====\nOver\n=====\n".to_string()),
                Just("- item\n".to_string()),
                Just("-\n".to_string()),
                Just("1. item\n".to_string()),
                Just("(i) item\n".to_string()),
                Just("#. item\n".to_string()),
                Just("   indented\n".to_string()),
                Just("  half\n".to_string()),
                Just("::\n".to_string()),
                Just("para::\n".to_string()),
                Just(".. _t:\n".to_string()),
                Just(".. _t: uri\n".to_string()),
                Just(".. comment\n".to_string()),
                Just("..\n".to_string()),
                Just("__ uri\n".to_string()),
                Just("| line\n".to_string()),
                Just("|\n".to_string()),
                Just(">>> code\n".to_string()),
                Just("term\n    def\n".to_string()),
                Just("term : c\n    def\n".to_string()),
                Just("*emph* and ``lit`` text\n".to_string()),
                Just("*unclosed here\n".to_string()),
                Just("`phrase ref`_ and word_ and anon__\n".to_string()),
                Just(":role:`text` and `bare`\n".to_string()),
                Just(":bogus:`x` end\n".to_string()),
                Just("[1]_ [#]_ [*]_ [cite]_\n".to_string()),
                Just("|sub| and |sub2|__\n".to_string()),
                Just("https://example.com/ and foo@bar.example\n".to_string()),
                Just(".. [1] footnote\n".to_string()),
                Just(".. [#lbl] auto\n".to_string()),
                Just(":field: value\n".to_string()),
                Just("-a  option desc\n".to_string()),
                Just("+----+----+\n".to_string()),
                Just("| A  | B  |\n".to_string()),
                Just("+====+====+\n".to_string()),
                Just("=====  =====\n".to_string()),
                Just("A      B\n".to_string()),
                Just("_`inline target` here\n".to_string()),
                Just("`text <https://x/>`_ ref\n".to_string()),
                Just("----\n".to_string()),
                Just("---\n".to_string()),
                Just("-- attribution\n".to_string()),
                Just("\n".to_string()),
                Just("\t\ttabs\n".to_string()),
                Just("> quoted\n".to_string()),
                Just("text\n".to_string()),
            ], 0..40).prop_map(|v| v.concat()),
        config in transform_config(),
    ) {
        let _ = parse_rst(&s, &opts());
        sphinx_read(&s, &opts(), &config);
    }

    #[test]
    fn parse_handles_multibyte_boundaries(
        s in "[αβ✓🎉a\\-=\\n \\|•‣⁃ß]{0,200}",
        config in transform_config(),
    ) {
        let _ = parse_rst(&s, &opts());
        sphinx_read(&s, &opts(), &config);
    }

    #[test]
    fn pformat_never_panics_after_parse(s in "\\PC{0,300}", config in transform_config()) {
        let tree = parse_rst(&s, &opts());
        let _ = tree.root.pformat();
        sphinx_read(&s, &opts(), &config);
    }

    #[test]
    fn parse_never_panics_on_multiline_arbitrary_input(
        v in proptest::collection::vec("\\PC{0,40}", 0..30),
        config in transform_config(),
    ) {
        let s = v.join("\n");
        let _ = parse_rst(&s, &opts());
        sphinx_read(&s, &opts(), &config);
    }

    // ------------------------------------------------------------------
    // wave 4.5: object descriptions (py + std), signatures, annotations
    // ------------------------------------------------------------------

    /// Arbitrary py signatures through every py object directive, with an
    /// arbitrary option block and an arbitrary doc-field body. The
    /// signature generator is `(?s)`-flagged, so a "signature" here really
    /// can be several lines — the multi-line signature path
    /// (`\`-continuation, blank-line termination) is the point.
    #[test]
    fn py_directives_never_panic_on_arbitrary_signatures(
        kind in prop_oneof![
            Just("py:function"), Just("py:data"), Just("py:class"),
            Just("py:exception"), Just("py:method"), Just("py:classmethod"),
            Just("py:staticmethod"), Just("py:attribute"), Just("py:property"),
            Just("py:type"), Just("py:decorator"), Just("py:decoratormethod"),
            Just("py:module"), Just("py:currentmodule"),
        ],
        sig in "(?s).{0,70}",
        options in proptest::collection::vec(
            prop_oneof![
                Just("   :no-index:\n".to_string()),
                Just("   :no-index-entry:\n".to_string()),
                Just("   :no-contents-entry:\n".to_string()),
                Just("   :no-typesetting:\n".to_string()),
                Just("   :module: pkg.mod\n".to_string()),
                Just("   :module:\n".to_string()),
                Just("   :canonical: pkg.mod.Other\n".to_string()),
                Just("   :async:\n".to_string()),
                Just("   :abstractmethod:\n".to_string()),
                Just("   :classmethod:\n".to_string()),
                Just("   :staticmethod:\n".to_string()),
                Just("   :final:\n".to_string()),
                Just("   :type: int\n".to_string()),
                Just("   :value: 3\n".to_string()),
                Just("   :platform: Unix\n".to_string()),
                Just("   :synopsis: s\n".to_string()),
                Just("   :deprecated:\n".to_string()),
                Just("   :single-line-parameter-list:\n".to_string()),
                Just("   :single-line-type-parameter-list:\n".to_string()),
                Just("   :bogus: x\n".to_string()),
            ], 0..5),
        body in proptest::collection::vec(
            prop_oneof![
                Just("   :param x: a value\n".to_string()),
                Just("   :param int x: a typed value\n".to_string()),
                Just("   :type x: int\n".to_string()),
                Just("   :returns: something\n".to_string()),
                Just("   :rtype: int\n".to_string()),
                Just("   :raises ValueError: when bad\n".to_string()),
                Just("   :var y: a variable\n".to_string()),
                Just("   :meta private:\n".to_string()),
                Just("   :unknown field: text\n".to_string()),
                Just("   :param:\n".to_string()),
                Just("   body paragraph\n".to_string()),
                Just("\n".to_string()),
            ], 0..6),
        config in transform_config(),
    ) {
        let mut src = format!(".. {kind}:: {sig}\n");
        for line in &options {
            src.push_str(line);
        }
        src.push('\n');
        for line in &body {
            src.push_str(line);
        }
        let tree = parse_rst(&src, &opts());
        let _ = tree.root.pformat();
        sphinx_read(&src, &opts(), &config);
    }

    /// The wave-4 std-domain description surface, which the totality sweep
    /// never covered either (final-panel backlog item). Same shape as the
    /// py sweep: arbitrary signature, arbitrary options, arbitrary fields.
    ///
    /// GENERATOR RULE, second edition (fix round 1): the fixed body lines
    /// below are all ASCII at three fixed indents, so the one branch that
    /// actually broke totality — `glossary`'s `line[indent_len:]` slicing a
    /// continuation line indented LESS than the entry's first definition
    /// line, with a multi-byte character straddling the offset — was
    /// unreachable by construction. The last arm draws an arbitrary indent
    /// (1..10, so the block's common indent varies and post-dedent lines
    /// land at every relative depth) over text that can carry 2-, 3- and
    /// 4-byte characters.
    #[test]
    fn std_directives_never_panic_on_arbitrary_signatures(
        kind in prop_oneof![
            Just("describe"), Just("object"), Just("envvar"), Just("confval"),
            Just("option"), Just("cmdoption"), Just("glossary"), Just("productionlist"),
        ],
        sig in "(?s).{0,70}",
        body in proptest::collection::vec(
            prop_oneof![
                Just("   :type: int\n".to_string()),
                Just("   :default: 3\n".to_string()),
                Just("   :param x: a value\n".to_string()),
                Just("   :meta private:\n".to_string()),
                Just("   term\n".to_string()),
                Just("      definition\n".to_string()),
                Just("   .. a comment\n".to_string()),
                Just("   body\n".to_string()),
                Just("\n".to_string()),
                (1usize..10, "[a-zé漢🐍 .:]{0,10}")
                    .prop_map(|(n, t)| format!("{}{t}\n", " ".repeat(n))),
            ], 0..8),
        config in transform_config(),
    ) {
        let mut src = format!(".. {kind}:: {sig}\n\n");
        for line in &body {
            src.push_str(line);
        }
        let tree = parse_rst(&src, &opts());
        let _ = tree.root.pformat();
        sphinx_read(&src, &opts(), &config);
    }

    /// The annotation parser reached directly, without a directive around
    /// it: `parse_annotation` runs the py expression parser and falls back
    /// to a plain-text node, and neither path may panic. The Sphinx read
    /// reaches it through a `:type:` option.
    #[test]
    fn parse_annotation_never_panics(s in "(?s).{0,80}", config in transform_config()) {
        let ctx = PyRefContext { module: Some("m".into()), class_: Some("C".into()), ..Default::default() };
        for cfg in [PySigConfig::default(), PySigConfig {
            python_use_unqualified_type_names: true,
            python_display_short_literal_types: true,
            ..PySigConfig::default()
        }] {
            for node in parse_annotation(&s, &ctx, &cfg) {
                let _ = node.pformat();
            }
        }
        sphinx_read(&format!(".. py:data:: x\n   :type: {s}\n"), &opts(), &config);
    }

    /// Signature text reached through the directive with the two config
    /// families that change the signature grammar's output shape.
    #[test]
    fn py_signature_config_variants_never_panic(
        sig in "(?s).{0,60}",
        config in transform_config(),
    ) {
        let mut o = opts();
        o.py = PySigConfig {
            maximum_signature_line_length: Some(1),
            python_maximum_signature_line_length: Some(1),
            add_function_parentheses: false,
            python_use_unqualified_type_names: true,
            python_display_short_literal_types: true,
            ..PySigConfig::default()
        };
        let src = format!(".. py:function:: {sig}\n");
        let tree = parse_rst(&src, &o);
        let _ = tree.root.pformat();
        sphinx_read(&src, &o, &config);
    }

    // ------------------------------------------------------------------
    // wave 4.5: the file-inserting directives
    // ------------------------------------------------------------------

    /// Arbitrary `include` option combinations against the scratch project.
    /// Options are drawn WITH repetition and without filtering, so mutually
    /// contradictory blocks (`:literal:` + `:code:`, `:start-line:` past
    /// `:end-line:`, an unknown codec, a missing file) are all in range.
    #[test]
    fn include_never_panics_on_arbitrary_option_blocks(
        target in include_target(),
        options in proptest::collection::vec(option_line(), 0..6),
        tail in "(?s).{0,40}",
        config in transform_config(),
    ) {
        let mut src = format!(".. include:: {target}\n");
        for line in &options {
            src.push_str(line);
        }
        src.push('\n');
        src.push_str(&tail);
        let tree = parse_rst(&src, &opts_in(scratch()));
        let _ = tree.root.pformat();
        sphinx_read(&src, &opts_in(scratch()), &config);
    }

    /// The same sweep for `literalinclude`, whose filter chain (`:lines:`,
    /// `:pyobject:`, `:start-at:`/`:end-before:`, `:diff:`, `:dedent:`,
    /// `:prepend:`/`:append:`) is the longer one.
    #[test]
    fn literalinclude_never_panics_on_arbitrary_option_blocks(
        target in include_target(),
        options in proptest::collection::vec(option_line(), 0..6),
        config in transform_config(),
    ) {
        let mut src = format!(".. literalinclude:: {target}\n");
        for line in &options {
            src.push_str(line);
        }
        let tree = parse_rst(&src, &opts_in(scratch()));
        let _ = tree.root.pformat();
        sphinx_read(&src, &opts_in(scratch()), &config);
    }

    /// Arbitrary text as the include argument, against a real srcdir. The
    /// property is TOTALITY and nothing more: whatever the argument —
    /// printable Unicode, control characters and newlines (`(?s).` draws
    /// the whole Unicode range, which `\PC` never does), an empty
    /// argument, an absolute path, a `..` traversal that leaves the
    /// project, a real member — path resolution and the read either splice
    /// content or degrade to a `system_message`, never a panic. It does
    /// NOT pin "never reads outside the project": neither sphinx nor this
    /// crate has that property (docutils' `Include` opens whatever path
    /// `relfn2path` produces, `..` and absolute forms included), and a
    /// traversal case is drawn here precisely so the read path past the
    /// srcdir is exercised.
    #[test]
    fn include_never_panics_on_arbitrary_paths(
        arg in include_argument(),
        config in transform_config(),
    ) {
        let src = format!(".. include:: {arg}\n");
        let tree = parse_rst(&src, &opts_in(scratch()));
        let _ = tree.root.pformat();
        sphinx_read(&src, &opts_in(scratch()), &config);
    }
}

// ----------------------------------------------------------------------
// M2 wave 5 sub-project 1: the read transforms
// ----------------------------------------------------------------------

/// A name the transform-shaped snippets share, so that definitions,
/// targets, labels and references collide: case variants (a substitution
/// falls back to its case-insensitive match, and a cycle through two names
/// that fold alike is docutils' endless one), names that normalize alike
/// (a space, an NBSP, a doubled space), a number (a manual footnote's
/// label) and nothing at all.
fn name() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("a"),
        Just("A"),
        Just("b"),
        Just("B"),
        Just("a b"),
        Just("a\u{a0}b"),
        Just("a  b"),
        Just("1"),
        Just(""),
    ]
}

/// Three [`name`]s.
fn names() -> impl Strategy<Value = (&'static str, &'static str, &'static str)> {
    (name(), name(), name())
}

/// One transform-shaped block: what every read transform reads or
/// rewrites, its names drawn from [`name`]. Substitution definitions that
/// nest, cycle, trim and fold case; substitution, hyperlink, anonymous,
/// footnote and citation references, wrapped and embedded; explicit,
/// indirect, external and anonymous targets; auto-numbered, labelled,
/// symbol and manual footnotes and citations; bibliographic fields (DocInfo
/// reads only a leading field list); transitions; doctest blocks, bare and
/// quoted; quotes, dashes and ellipses, escaped and literal; language
/// classes; module targets, index entries, captioned figures, tables and
/// code blocks, toctrees, terms; the default substitutions.
fn transform_block() -> impl Strategy<Value = String> {
    prop_oneof![
        names().prop_map(|(n, m, k)| format!(".. |{n}| replace:: {m} |{k}| \"q\"\n")),
        names().prop_map(|(n, _, k)| format!(".. |{n}| replace:: |{k}|_ x\n")),
        names().prop_map(|(n, m, _)| format!(".. |{n}| replace:: `{m}`_ *e* |{n}|\n")),
        (
            name(),
            prop_oneof![Just(":trim:"), Just(":ltrim:"), Just(":rtrim:")]
        )
            .prop_map(|(n, opt)| format!(".. |{n}| unicode:: U+2014 U+00A0\n   {opt}\n")),
        name().prop_map(|n| format!(".. |{n}| image:: x.png\n   :target: {n}_\n")),
        names().prop_map(|(n, m, k)| format!("See |{n}| and |{m}|_ and |{k}|__ and \\|{n}|.\n")),
        name().prop_map(|n| format!(".. _{n}:\n")),
        names().prop_map(|(n, m, _)| format!(".. _{n}: {m}_\n")),
        names().prop_map(|(n, m, _)| format!(".. _{n}: `{m}`_\n")),
        names().prop_map(|(n, m, _)| format!(".. _{n}: http://x.example/{m}\n")),
        Just(".. __: http://anon.example/\n".to_string()),
        name().prop_map(|n| format!(".. __: {n}_\n")),
        names().prop_map(|(n, m, k)| {
            format!("`{n}`_ and `{m}`__ and `t <{k}_>`_ and `u <http://x>`__ and {n}_\n")
        }),
        name().prop_map(|n| format!("An _`{n}` inline target and _`{n}` again.\n")),
        name().prop_map(|n| format!(".. [#{n}] Labelled.\n")),
        Just(".. [#] Auto.\n".to_string()),
        Just(".. [*] Symbol.\n".to_string()),
        Just(".. [1] Manual.\n".to_string()),
        Just(".. [cite] Citation.\n".to_string()),
        name().prop_map(|n| format!("Refs [#{n}]_ [#]_ [*]_ [1]_ [cite]_ [nocite]_.\n")),
        Just(":orphan:\n:tocdepth: 2\n:nocomments:\n".to_string()),
        Just(":author: J. Doe\n:authors: A; B, C\n:version: 1. x\n".to_string()),
        Just(":date: $Date: 2026/09/30 $\n:abstract: Sum.\n:dedication: D\n".to_string()),
        name().prop_map(|n| format!(":field: |{n}| `{n}`_\n:tocdepth: x\n")),
        Just("----\n".to_string()),
        Just(">>> 1 + 1\n2\n".to_string()),
        Just("Quote:\n\n   >>> x\n   >>> y\n".to_string()),
        Just("Title\n=====\n".to_string()),
        Just("Sub\n---\n".to_string()),
        Just(
            "\"Quotes\" -- dashes --- and... 'single' ``\"lit\"`` \\\"esc\\\" \\--.\n".to_string()
        ),
        Just("'80s \"a 'b' c\" x\u{a0}\"y\" \u{2013}\"z\"\n".to_string()),
        prop_oneof![Just("de"), Just("fr"), Just("ja"), Just("xx"), Just("")]
            .prop_map(|l| format!(".. rst-class:: language-{l}\n\n\"Q\" -- 'q'\n")),
        Just(".. py:module:: m\n".to_string()),
        Just(".. index:: single: x\n".to_string()),
        Just(".. figure:: x.png\n\n   \"Caption\"\n".to_string()),
        Just(".. table:: Cap\n\n   = =\n   a b\n   = =\n".to_string()),
        Just(".. code-block:: python\n   :caption: c\n\n   x = 1\n".to_string()),
        Just(".. toctree::\n   :caption: \"C\"\n\n   T <doc>\n".to_string()),
        Just("term : classifier\n   \"def\"\n".to_string()),
        Just("|today| |version| |release| |translation progress|\n".to_string()),
        Just(".. only:: html\n\n   ----\n\n   S\n   -\n".to_string()),
        Just(".. note::\n\n   |a| `a`_ [#]_ \"n\"\n".to_string()),
        Just("\n".to_string()),
        Just("\n\n".to_string()),
    ]
}

/// The openers of a deep-nesting level that nest in each other: how a
/// level opens, and how far in its content — the next level — is indented.
/// `\n` ends an opener that takes no text on its line (the level's text
/// becomes its first paragraph); an empty opener is a paragraph whose block
/// quote the next level is; `term` is a definition-list item, its
/// definition following with no blank line.
const NESTING_OPENERS: [(&str, usize); 14] = [
    ("- ", 2),
    ("#. ", 3),
    ("(i) ", 4),
    ("", 3),
    (":f: ", 3),
    ("term", 3),
    (".. [#] ", 3),
    (".. [c] ", 3),
    (".. note:: ", 3),
    (".. admonition:: ", 3),
    (".. container:: c\n", 3),
    (".. only:: html\n", 3),
    (".. compound::\n", 3),
    (".. py:function:: f()\n", 3),
];

/// Openers that end a nesting chain: `topic` and `sidebar` are refused
/// inside body elements (docutils' "may not be used within topics or body
/// elements"), and `versionadded` joins its content into one paragraph
/// where Sphinx parses it as body elements (a known parser gap).
const STOPPING_OPENERS: [(&str, usize); 3] = [
    (".. topic:: ", 3),
    (".. sidebar:: ", 3),
    (".. versionadded:: 1.0 ", 3),
];

/// A level that keeps the chain nesting.
fn nesting_opener() -> impl Strategy<Value = (&'static str, usize)> {
    proptest::sample::select(&NESTING_OPENERS[..])
}

/// Any level — the innermost one, where a stopping opener ends nothing.
fn any_opener() -> impl Strategy<Value = (&'static str, usize)> {
    proptest::sample::select([&NESTING_OPENERS[..], &STOPPING_OPENERS[..]].concat())
}

/// One deep-nesting level: its opener, its text, and the transform-shaped
/// blocks beside its content.
type Level = ((&'static str, usize), &'static str, Vec<String>);

/// `depth` levels whose every opener but the innermost nests, so a chain
/// longer than the parser's 200-level guard reaches it.
fn nesting_levels(depth: usize) -> impl Strategy<Value = Vec<Level>> {
    let level = |opener: BoxedStrategy<(&'static str, usize)>| {
        (
            opener,
            inline_payload(),
            proptest::collection::vec(transform_block(), 0..2),
        )
    };
    (
        proptest::collection::vec(level(nesting_opener().boxed()), depth - 1),
        level(any_opener().boxed()),
    )
        .prop_map(|(mut levels, innermost)| {
            levels.push(innermost);
            levels
        })
}

/// One line of text the transforms read, for a nesting level.
fn inline_payload() -> impl Strategy<Value = &'static str> {
    prop_oneof![
        Just("\"q\" -- x..."),
        Just("|s| and |c0|_"),
        Just("`t`_ and anon__"),
        Just("[#]_ [*]_ [cite]_"),
        Just("_`inner` target"),
        Just("|today|"),
        Just("plain"),
    ]
}

/// The tree the deep-nesting sweep reads: `levels` nested containers, each
/// with its text and the transform-shaped blocks beside its content, and a
/// substitution chain whose every link wraps the next in a reference
/// (`.. |cK| replace:: |cK+1|_`) — nesting inline as deep as the chain is
/// long, as substitution expansion builds it (docutils' expansion grows
/// with the square of a chain's length, so a short one).
fn nested_document(levels: &[Level], chain: usize, tail: &[String]) -> String {
    let mut src = String::new();
    let mut indent = 0usize;
    for ((open, width), text, blocks) in levels {
        let pad = " ".repeat(indent);
        let inner = " ".repeat(indent + width);
        match *open {
            "term" => src.push_str(&format!("{pad}term {text}\n")),
            open if open.ends_with('\n') => {
                src.push_str(&format!("{pad}{open}\n{inner}{text}\n\n"));
            }
            open => src.push_str(&format!("{pad}{open}{text}\n\n")),
        }
        for block in blocks {
            for line in block.lines() {
                if !line.is_empty() {
                    src.push_str(&inner);
                }
                src.push_str(line);
                src.push('\n');
            }
            src.push('\n');
        }
        indent += width;
    }
    src.push('\n');
    for link in 0..chain {
        src.push_str(&format!(".. |c{link}| replace:: \"x\" |c{}|_\n", link + 1));
    }
    src.push_str(&format!(
        ".. |c{chain}| replace:: end\n.. |s| replace:: S\n"
    ));
    src.push_str(".. _t: http://x.example/\n.. [cite] C\n.. [#] F\n.. [*] G\n\n");
    for block in tail {
        src.push_str(block);
        src.push('\n');
    }
    src
}

proptest! {
    #![proptest_config(sweep_config())]

    /// Transform-shaped documents: [`transform_block`]s in any order, any
    /// number of times, so that each kind of definition meets its
    /// references before and after it, defined twice or never, and the
    /// names collide across kinds.
    #[test]
    fn transforms_never_panic_on_transform_shaped_input(
        blocks in proptest::collection::vec(transform_block(), 0..24),
        separators in proptest::collection::vec(prop_oneof![Just(""), Just("\n")], 24),
        config in transform_config(),
    ) {
        let mut src = String::new();
        for (block, separator) in blocks.iter().zip(&separators) {
            src.push_str(block);
            src.push_str(separator);
        }
        sphinx_read(&src, &opts(), &config);
    }

    /// The deepest trees the parser builds, through the transforms. A
    /// quarter of the cases nest 210 to 260 levels — past the parser's
    /// 200-level guard, which replaces deeper content with an ERROR — and
    /// the rest 1 to 39; every level but the innermost is one of the 14
    /// openers that nest in each other ([`NESTING_OPENERS`]), the innermost
    /// any of them or one of the 3 that end a chain ([`STOPPING_OPENERS`]).
    /// Every level carries transform-shaped blocks, and an inline chain of
    /// wrapped substitution references rides along ([`read_deep`]). A deep
    /// case must print the guard's ERROR, so the sweep cannot quietly stop
    /// reaching it.
    #[test]
    fn transforms_survive_the_deep_nesting_sweep(
        levels in prop_oneof![3 => 1usize..40, 1 => 210usize..=260]
            .prop_flat_map(nesting_levels),
        chain in 0usize..24,
        tail in proptest::collection::vec(transform_block(), 0..6),
        config in transform_config(),
    ) {
        let src = nested_document(&levels, chain, &tail);
        let started = std::time::Instant::now();
        let records = read_deep(src, config);
        if std::env::var_os(CASE_TIMES).is_some() {
            eprintln!(
                "deep sweep case: {} levels, {} ms",
                levels.len(),
                started.elapsed().as_millis()
            );
        }
        if levels.len() >= 210 {
            let guard = records
                .iter()
                .filter(|text| text.as_str() == GUARD_RECORD)
                .count();
            prop_assert!(guard >= 1, "{} levels, no guard record", levels.len());
        }
    }
}

/// The text of the parser's nesting-guard record.
const GUARD_RECORD: &str = "Maximum nesting depth exceeded; deeper content skipped.";

/// The deep sweep's timeout at 2,048 cases (60.2 s, twice; shrunk to 220
/// levels holding `.. |a| replace:: |a|_ x` and a `.. |A|` definition),
/// down to its cause. A definition wrapping a reference to itself, under a
/// name another definition folds onto, escapes docutils' circularity test
/// and doubles at each expansion of its own reference, its references
/// nesting twice as deep each time: on to 8,192 levels before the
/// line-length limit — where Sphinx dies of RecursionError from 1,024 on
/// (probed) — and every transform after it walked each copy in time
/// quadratic in that depth. The expansion stops at Substitutions' depth
/// limit: the definition's own last reference and the paragraph's four
/// are reported instead of expanded.
#[test]
fn a_self_wrapping_substitution_stops_at_the_depth_limit() {
    let src = "|a| |a| |a| |a|\n\n.. |a| replace:: |a|_ x\n.. |A| replace:: y\n";
    for smartquotes in [false, true] {
        let config = TransformConfig {
            smartquotes,
            ..TransformConfig::default()
        };
        let records = read_deep(src.to_string(), config);
        let limit = "Substitution definition \"a\" exceeds the maximum nesting depth.";
        let guard = records.iter().filter(|text| *text == limit).count();
        assert_eq!(guard, 5, "smartquotes={smartquotes}");
    }
}

/// [`sphinx_read`] for the deep-nesting sweep, returning the printed
/// records' texts — all of it on one thread with the build's parse stack
/// ([`on_parse_stack`]): the parse and the transforms, which run in place
/// there, and the print (`pformat` and the tree's drop recurse once a
/// level, test-side work).
fn read_deep(src: String, config: TransformConfig) -> Vec<String> {
    on_parse_stack(move || {
        let (tree, records) = parse_and_transform(&src, &opts(), &config);
        let _ = tree.root.pformat();
        records
            .iter()
            .map(|record| {
                let _ = BuildWarning::from_diagnostic(record, "index.rst".into()).render();
                record.text.clone()
            })
            .collect()
    })
}

/// Each of the [`NESTING_OPENERS`] reaches the parser's guard on its own:
/// 260 levels of it print the guard's ERROR exactly once, and the
/// transforms then run over the deepest tree that opener builds. (The
/// [`STOPPING_OPENERS`] would not: `topic` and `sidebar` stop at the second
/// level, `versionadded` does not nest at all.)
#[test]
fn the_deep_nesting_documents_reach_the_guard() {
    for opener in NESTING_OPENERS {
        let levels = vec![(opener, "\"q\" |s| `t`_ [#]_", Vec::new()); 260];
        let records = read_deep(nested_document(&levels, 8, &[]), TransformConfig::default());
        let guard = records
            .iter()
            .filter(|text| text.as_str() == GUARD_RECORD)
            .count();
        assert_eq!(guard, 1, "{opener:?}: {records:?}");
    }
}

#[test]
fn deep_nesting_does_not_overflow_stack() {
    // 6000 levels of nested bullet lists: far past the MAX_NEST_DEPTH guard
    // (200), which drops deeper content with an ERROR message instead of
    // overflowing the stack (docutils crashes with RecursionError here).
    let mut s = String::new();
    for depth in 0..6000 {
        s.push_str(&"  ".repeat(depth));
        s.push_str("- x\n");
    }
    let tree = parse_rst(&s, &opts());
    let out = tree.root.pformat();
    assert_eq!(
        out.matches("Maximum nesting depth exceeded; deeper content skipped.")
            .count(),
        1,
        "depth guard must fire exactly once"
    );
}

#[test]
fn pathological_wide_inputs() {
    // Very long single lines and very many siblings.
    let long_line = "x".repeat(100_000);
    let _ = parse_rst(&long_line, &opts());
    let adornment = "=".repeat(100_000);
    let _ = parse_rst(&format!("{long_line}\n{adornment}\n"), &opts());
    let many_paras = "para\n\n".repeat(20_000);
    let _ = parse_rst(&many_paras, &opts());
    let many_targets = ".. _t:\n".repeat(5_000);
    let _ = parse_rst(&many_targets, &opts());
}
