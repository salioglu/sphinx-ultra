//! The post-transforms Sphinx applies to a document as a builder writes
//! it (`env.apply_post_transforms`, `SP/sphinx/environment/__init__.py:759-776`),
//! over a copy of the post-read doctree, before the toctrees are resolved
//! (`get_and_resolve_doctree`, `:668-717`). Of the twenty a default build
//! registers, five can change an html or a dummy build's tree [TW §2.3];
//! [`apply_post_transforms`] runs them in priority order:
//!
//! - 10 `ReferencesResolver` ([`crate::env::resolve::resolve_document`]);
//! - 50 `OnlyNodeTransform` ([`only_nodes`]);
//! - 200 `PropagateDescDomain` ([`propagate_desc_domain`]);
//! - 400 `HighlightLanguageTransform` ([`highlight_language`]);
//! - 401 `TrimDoctestFlagsTransform` ([`trim_doctest_flags`]).
//!
//! The rest are no-ops or unsupported under those builders. The
//! transforms run over the owned doctree a build resolves, not over the
//! read's arena: they move whole subtrees and read no registry.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::tags::Tags;
use crate::diagnostics::{Category, Diagnostic, Severity};
use crate::doctree::{kinds, AttrValue, Doctree, Node, Span};
use crate::env::resolve::{
    resolve_document, take_basic_atts, update_basic_atts, NitpickConfig, Resolver,
};
use crate::rst::LogLocation;

/// What the post-transforms read besides the document and the resolver.
pub struct PostTransformConfig<'a> {
    /// The build's tags (`env._tags`): the `-t` tags and the builder's own,
    /// which `OnlyNodeTransform` evaluates `only` expressions against.
    pub tags: &'a Tags,
    /// `highlight_language` (`SP/sphinx/config.py:257`): the language
    /// `HighlightLanguageTransform` stamps until a `.. highlight::`
    /// replaces it.
    pub highlight_language: &'a str,
    /// `trim_doctest_flags` (`SP/sphinx/config.py:267`): whether
    /// `TrimDoctestFlagsTransform` strips doctest flags.
    pub trim_doctest_flags: bool,
    /// The project's source directory, against which a warning about a
    /// node names its source ([`crate::rst::node_source_path`]).
    pub source_dir: &'a Path,
}

/// What applying the post-transforms to one document gave.
pub struct PostTransformed {
    /// The records the post-transforms logged, in the order Sphinx prints
    /// them: the reference resolver's, then `OnlyNodeTransform`'s.
    pub records: Vec<Diagnostic>,
    /// References into a domain this build has no resolver for
    /// ([`crate::env::resolve::DocumentResolution`]).
    pub unresolvable_domain_refs: usize,
}

/// `env.apply_post_transforms(doctree, docname)` under the html or the
/// dummy builder: the five post-transforms that can change the tree, in
/// priority order, over `doctree`, the copy of `docname`'s post-read
/// doctree the builder writes; `source_path` is the document's source.
pub fn apply_post_transforms(
    resolver: &Resolver<'_>,
    nitpick: &NitpickConfig<'_>,
    config: &PostTransformConfig<'_>,
    docname: &str,
    source_path: &Path,
    doctree: &mut Doctree,
) -> PostTransformed {
    let resolution = resolve_document(
        resolver,
        nitpick,
        docname,
        doctree,
        source_path,
        config.source_dir,
    );
    let mut records: Vec<Diagnostic> = resolution
        .warnings
        .into_iter()
        .map(Diagnostic::from)
        .collect();
    let Doctree { root, sources } = doctree;
    let only = OnlyCx {
        tags: config.tags,
        sources,
        source_dir: config.source_dir,
        docname,
    };
    only_nodes(&only, root, &mut records);
    propagate_desc_domain(root);
    highlight_language(root, config.highlight_language);
    trim_doctest_flags(root, config.trim_doctest_flags);
    PostTransformed {
        records,
        unresolvable_domain_refs: resolution.unresolvable_domain_refs,
    }
}

/// What `OnlyNodeTransform` reads besides the tree.
struct OnlyCx<'a> {
    tags: &'a Tags,
    sources: &'a [String],
    source_dir: &'a Path,
    docname: &'a str,
}

/// 50 `OnlyNodeTransform` (`SP/sphinx/transforms/post_transforms/__init__.py:324-332`),
/// `process_only_nodes` (`SP/sphinx/util/nodes.py:719-742`), over
/// `parent`'s children: each `only` node whose `expr` the tags satisfy is
/// replaced by its children, or by a `comment` when it has none, and every
/// other one by a `comment`, docutils' `replace_self` giving the first
/// replacement node the `only` node's ids, classes, names and dupnames.
/// An expression Sphinx cannot evaluate keeps the content and warns at
/// the node, with the exception's text ([`Tags::eval_condition`]).
///
/// `process_only_nodes` walks a live `findall`: it meets an `only` node
/// before what it holds, and goes on into the node's own children after
/// replacing it. A kept node's children are the nodes put in its place,
/// whose `only` nodes are evaluated next; a dropped node's are left out of
/// the tree, but their `only` nodes are still evaluated, warnings
/// included (probed on Sphinx 9.1.0).
fn only_nodes(cx: &OnlyCx<'_>, parent: &mut Node, records: &mut Vec<Diagnostic>) {
    let mut at = 0;
    while at < parent.children.len() {
        if parent.children[at].kind != kinds::ONLY {
            only_nodes(cx, &mut parent.children[at], records);
            at += 1;
            continue;
        }
        let mut only = parent.children.remove(at);
        let expr = match only.get("expr") {
            Some(AttrValue::Str(expr)) => cx.tags.eval_condition(expr),
            // `node['expr']` raising `KeyError('expr')`.
            _ => Err("'expr'".to_string()),
        };
        let keep = expr.unwrap_or_else(|error| {
            records.push(only_warning(cx, &only, &error));
            true
        });
        let basic = take_basic_atts(&mut only);
        let mut replacement = if keep && !only.children.is_empty() {
            std::mem::take(&mut only.children)
        } else {
            if !keep {
                only_nodes(cx, &mut only, records);
            }
            vec![comment()]
        };
        if let Some(first) = replacement
            .first_mut()
            .filter(|first| first.kind != kinds::TEXT)
        {
            update_basic_atts(first, basic);
        }
        parent.children.splice(at..at, replacement);
    }
}

/// `_only_node_keep_children`'s warning about `only`, `location=node`:
/// `Only.run` stamps the node with the directive's own source and line
/// (`set_source_info`), which `get_node_location` makes absolute.
fn only_warning(cx: &OnlyCx<'_>, only: &Node, error: &str) -> Diagnostic {
    let location = match only.span.line {
        0 => LogLocation::None,
        line => LogLocation::Node {
            source: only.span.source,
            line,
        },
    };
    Diagnostic {
        location: location.render(cx.sources, cx.source_dir, cx.docname),
        severity: Severity::Warning,
        message: format!("exception while evaluating only directive expression: {error}"),
        category: Category::None,
    }
}

/// `nodes.comment()`, which an `only` node gives way to.
fn comment() -> Node {
    let mut comment = Node::elem(kinds::COMMENT, Span::ZERO);
    comment.set("xml:space", AttrValue::Str("preserve".to_string()));
    comment
}

/// 200 `PropagateDescDomain` (`SP/sphinx/transforms/post_transforms/__init__.py:382-389`):
/// every `desc_signature` takes its `desc`'s domain as a class, when the
/// description names one (`describe` and `object` have `domain=""`).
fn propagate_desc_domain(node: &mut Node) {
    if node.kind == "desc" {
        if let Some(AttrValue::Str(domain)) = node.get("domain") {
            if !domain.is_empty() {
                let domain = domain.clone();
                for child in &mut node.children {
                    if child.kind == "desc_signature" {
                        child.attrs.classes.push(domain.clone());
                    }
                }
            }
        }
    }
    for child in &mut node.children {
        propagate_desc_domain(child);
    }
}

/// The highlight setting in force: `HighlightSetting(language, force,
/// lineno_threshold)` (`SP/sphinx/transforms/post_transforms/code.py:24-27`).
struct HighlightSetting {
    language: AttrValue,
    force: AttrValue,
    lineno_threshold: i64,
}

/// 400 `HighlightLanguageTransform` (`SP/sphinx/transforms/post_transforms/code.py:30-86`)
/// [PYG §3.3]: a walk in document order, whatever the nesting, with
/// `highlight_language`, no `force` and `sys.maxsize` as the setting until
/// a `highlightlang` node (`.. highlight::`) replaces it. A
/// `literal_block` without `language` takes the setting's language and,
/// with it, its `force`; one without `linenos` takes
/// `astext().count('\n') >= lineno_threshold - 1`. A `doctest_block` is
/// not a `literal_block`. Then every `highlightlang` node leaves the tree.
/// (The setting a `start_of_file` node pushes belongs to the builders that
/// assemble documents, not to html or dummy.)
fn highlight_language(root: &mut Node, default_language: &str) {
    fn stamp(node: &mut Node, setting: &mut HighlightSetting) {
        match node.kind {
            "highlightlang" => {
                *setting = HighlightSetting {
                    language: node
                        .get("lang")
                        .cloned()
                        .unwrap_or(AttrValue::Str(String::new())),
                    force: node.get("force").cloned().unwrap_or(AttrValue::Int(0)),
                    lineno_threshold: match node.get("linenothreshold") {
                        Some(AttrValue::Int(threshold)) => *threshold,
                        _ => i64::MAX,
                    },
                };
            }
            kinds::LITERAL_BLOCK => {
                if node.get("language").is_none() {
                    node.set("language", setting.language.clone());
                    node.set("force", setting.force.clone());
                }
                if node.get("linenos").is_none() {
                    let lines = node.astext_docutils().matches('\n').count();
                    let numbered = i64::try_from(lines).unwrap_or(i64::MAX)
                        >= setting.lineno_threshold.saturating_sub(1);
                    node.set("linenos", AttrValue::Int(i64::from(numbered)));
                }
            }
            _ => {}
        }
        for child in &mut node.children {
            stamp(child, setting);
        }
    }
    fn remove_highlightlang(node: &mut Node) {
        node.children.retain(|child| child.kind != "highlightlang");
        for child in &mut node.children {
            remove_highlightlang(child);
        }
    }
    let mut setting = HighlightSetting {
        language: AttrValue::Str(default_language.to_string()),
        force: AttrValue::Int(0),
        lineno_threshold: i64::MAX,
    };
    stamp(root, &mut setting);
    remove_highlightlang(root);
}

/// `blankline_re` (`SP/sphinx/ext/doctest.py:41`), `re.MULTILINE`,
/// Python's `\s` with the four separators `\x1c`-`\x1f`.
static BLANKLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[\s\x1c-\x1f]*<BLANKLINE>").expect("the BLANKLINE pattern compiles")
});

/// `doctestopt_re` (`SP/sphinx/ext/doctest.py:42`), `re.MULTILINE`.
static DOCTESTOPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)[ \t]*#[\s\x1c-\x1f]*doctest:.+$").expect("the doctest pattern compiles")
});

/// 401 `TrimDoctestFlagsTransform` (`SP/sphinx/transforms/post_transforms/code.py:89-132`)
/// [PYG §3.3]: every `literal_block` a Python console session is in
/// ([`is_pyconsole`]), and every `doctest_block`, loses its `<BLANKLINE>`
/// lines and its `# doctest:` comments, unless its `trim_flags`, or else
/// `trim_doctest_flags`, says not to: the trimmed rawsource replaces both
/// the rawsource and the children, as one Text node.
fn trim_doctest_flags(node: &mut Node, configured: bool) {
    let trimmed = match node.kind {
        kinds::LITERAL_BLOCK => is_pyconsole(node),
        kinds::DOCTEST_BLOCK => true,
        _ => false,
    };
    if trimmed {
        strip_doctest_flags(node, configured);
    }
    for child in &mut node.children {
        trim_doctest_flags(child, configured);
    }
}

/// `strip_doctest_flags` (`code.py:104-113`); `configured` is
/// `trim_doctest_flags`.
fn strip_doctest_flags(node: &mut Node, configured: bool) {
    let trim = match node.get("trim_flags") {
        Some(flag) => flag.is_truthy(),
        None => configured,
    };
    if !trim {
        return;
    }
    let source = node.rawsource().unwrap_or_default();
    let source = BLANKLINE.replace_all(source, "");
    let source = DOCTESTOPT.replace_all(&source, "").into_owned();
    node.set_rawsource(&source);
    node.children = vec![Node::text_node(source, node.span)];
}

/// `TrimDoctestFlagsTransform.is_pyconsole` (`code.py:115-132`): a
/// literal block whose text is its source (not a parsed-literal with
/// markup) and whose language is `pycon` or `pycon3`, a Python one (or
/// `default`) with a source opening with `>>>`, or `guess` when Pygments
/// would guess its Python console lexer.
fn is_pyconsole(node: &Node) -> bool {
    let rawsource = node.rawsource().unwrap_or_default();
    if rawsource != node.astext_docutils() {
        return false;
    }
    match node.get("language") {
        Some(AttrValue::Str(language)) => match language.as_str() {
            "pycon" | "pycon3" => true,
            "py" | "python" | "py3" | "python3" | "default" => rawsource.starts_with(">>>"),
            "guess" => guesses_python_console(rawsource),
            _ => false,
        },
        _ => false,
    }
}

/// Whether `pygments.lexers.guess_lexer(text)` gives a
/// `PythonConsoleLexer`: only through a Vim modeline naming it, because the
/// lexer's text analysis scores 0 and so never wins the guess
/// (Pygments 2.21.0, `SP/pygments/lexers/__init__.py:304-340`).
fn guesses_python_console(text: &str) -> bool {
    modeline_filetype(text).is_some_and(|filetype| {
        matches!(filetype.to_lowercase().as_str(), "pycon" | "python-console")
    })
}

/// `modeline_re` (`SP/pygments/modeline.py:16-19`), Python's `\s` with the
/// four separators `\x1c`-`\x1f`.
static MODELINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:vi|vim|ex)(?:[<=>]?\d*)?:.*(?:ft|filetype|syn|syntax)=([^:\s\x1c-\x1f]+)")
        .expect("the modeline pattern compiles")
});

/// `get_filetype_from_buffer` (`SP/pygments/modeline.py:29-43`): the
/// filetype of the first modeline among the last five lines, from the
/// last, then among the first six, from the sixth.
fn modeline_filetype(text: &str) -> Option<&str> {
    fn filetype(line: &str) -> Option<&str> {
        MODELINE
            .captures(line)
            .and_then(|captures| captures.get(1))
            .map(|found| found.as_str())
    }
    let lines = crate::utils::py_splitlines(text);
    lines
        .iter()
        .rev()
        .take(5)
        .chain(lines.iter().take(6).rev())
        .find_map(|line| filetype(line))
}

#[cfg(test)]
mod tests {
    use crate::builder::BuilderKind;
    use crate::config::BuildConfig;
    use crate::read::{read_standalone, resolve_standalone, ReadSettings};
    use crate::rst::ParseOptions;
    use crate::transforms::test_support::{index_rst, DOC};

    /// The read settings of a build with M1's validation off and
    /// `trim_doctest_flags` as given.
    fn settings(trim_doctest_flags: bool) -> ReadSettings {
        ReadSettings::from_config(&BuildConfig {
            validate_directives: false,
            trim_doctest_flags,
            ..BuildConfig::default()
        })
    }

    /// `rst` read as `<SRCDIR>/index.rst` and resolved as `kind` resolves
    /// it ([`resolve_standalone`]): the resolved pformat less its document
    /// line, and the resolve pass's records, rendered.
    fn resolved_with(
        rst: &str,
        parse: &ParseOptions,
        settings: &ReadSettings,
        kind: BuilderKind,
    ) -> (String, Vec<String>) {
        let read = read_standalone(rst, parse, settings);
        let (doctree, records) = resolve_standalone(&read, settings, kind);
        let pformat = doctree.root.pformat();
        let body = pformat
            .split_once('\n')
            .map(|(_, body)| body.to_string())
            .unwrap_or_default();
        let records = records.iter().map(|record| record.render(true)).collect();
        (body, records)
    }

    fn resolved(rst: &str, kind: BuilderKind) -> (String, Vec<String>) {
        resolved_with(rst, &index_rst(), &settings(true), kind)
    }

    /// The resolved document is `<SRCDIR>/index.rst`'s.
    #[test]
    fn the_resolved_document_keeps_its_document_line() {
        let read = read_standalone("Text.\n", &index_rst(), &settings(true));
        let (doctree, _) = resolve_standalone(&read, &settings(true), BuilderKind::Dummy);
        assert!(doctree.root.pformat().starts_with(DOC));
    }

    /// `OnlyNodeTransform` (50) keeps an `only` node's content where the
    /// builder's tags satisfy its expression and puts a `comment` in its
    /// place where they do not: `sphinx_read-0248` and `-0249` under the
    /// HTML builder and under the dummy builder, probed on Sphinx 9.1.0
    /// (`get_and_resolve_doctree`).
    #[test]
    fn only_node_keeps_or_comments() {
        for (rst, text) in [
            (
                ".. only:: html\n\n   HTML only content.\n",
                "HTML only content.",
            ),
            (
                ".. only:: html and not epub\n\n   Complex expr content.\n",
                "Complex expr content.",
            ),
        ] {
            assert_eq!(
                resolved(rst, BuilderKind::Html),
                (format!("    <paragraph>\n        {text}\n"), Vec::new()),
                "{rst:?}"
            );
            assert_eq!(
                resolved(rst, BuilderKind::Dummy),
                (
                    "    <comment xml:space=\"preserve\">\n".to_string(),
                    Vec::new()
                ),
                "{rst:?}"
            );
        }
    }

    /// An `only` node's replacement takes its ids and names (docutils'
    /// `replace_self`), a kept node with no content and a dropped one both
    /// become a `comment`, nested `only` nodes are evaluated outer first,
    /// those inside a dropped one included, and an expression Sphinx cannot
    /// evaluate keeps the content and warns at the node, with the text of
    /// the exception. Probed on Sphinx 9.1.0.
    #[test]
    fn only_nodes_pass_on_their_ids_and_warn_on_bad_expressions() {
        let html = resolved(
            "Title\n=====\n\n.. _tgt:\n\n.. only:: html &&\n\n   Kept para.\n\n   Second para.\n\n\
             .. only:: latex\n\n   .. only:: (\n\n      inner\n\nEnd.\n",
            BuilderKind::Html,
        );
        assert_eq!(
            html.0,
            "    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        \
             <target refid=\"tgt\">\n        <paragraph ids=\"tgt\" names=\"tgt\">\n            \
             Kept para.\n        <paragraph>\n            Second para.\n        \
             <comment xml:space=\"preserve\">\n        <paragraph>\n            End.\n"
        );
        assert_eq!(
            html.1,
            [
                "<SRCDIR>/index.rst:6: WARNING: exception while evaluating only directive \
                 expression: unexpected char '&' at 5\n  line 1",
                "<SRCDIR>/index.rst:14: WARNING: exception while evaluating only directive \
                 expression: unexpected token 'end of template'\n  line 1",
            ]
        );

        let dummy = resolved(
            ".. _tgt:\n\n.. only:: html\n\n   Dropped under dummy.\n\n.. only:: dummy\n\n\
             .. _two:\n\n.. only:: dummy and not html\n\n   .. only:: builder_dummy\n\n      \
             Inner kept.\n\n   Second.\n\nText.\n",
            BuilderKind::Dummy,
        );
        assert_eq!(
            dummy,
            (
                "    <target refid=\"tgt\">\n    <comment ids=\"tgt\" names=\"tgt\" \
                 xml:space=\"preserve\">\n    <comment xml:space=\"preserve\">\n    \
                 <target refid=\"two\">\n    <paragraph ids=\"two\" names=\"two\">\n        \
                 Inner kept.\n    <paragraph>\n        Second.\n    <paragraph>\n        Text.\n"
                    .to_string(),
                Vec::new()
            )
        );
    }

    /// A condition too deep for Python's recursion limit is an exception
    /// like any other: the only directive warns with its text and keeps
    /// the content ([`crate::transforms::tags`]). Probed on Sphinx 9.1.0
    /// (`-b html`): 300 parentheses and 20000 `not`s.
    #[test]
    fn an_only_condition_too_deep_for_python_warns_and_keeps_its_content() {
        for expr in [
            format!("{}html{}", "(".repeat(300), ")".repeat(300)),
            format!("{}html", "not ".repeat(20_000)),
        ] {
            let (body, records) = resolved(
                &format!("Title\n=====\n\n.. only:: {expr}\n\n   Body.\n"),
                BuilderKind::Html,
            );
            assert_eq!(
                body,
                "    <section ids=\"title\" names=\"title\">\n        <title>\n            \
                 Title\n        <paragraph>\n            Body.\n"
            );
            assert_eq!(
                records,
                [
                    "<SRCDIR>/index.rst:4: WARNING: exception while evaluating only directive \
                  expression: maximum recursion depth exceeded"
                ]
            );
        }
    }

    /// `HighlightLanguageTransform` (400) stamps every `literal_block`
    /// without a `language` with the setting in force, in document order
    /// whatever the nesting: `highlight_language` until a `.. highlight::`
    /// replaces it; `force` only together with `language`, so a
    /// `code-block` keeps its own; `linenos` where it is missing, as
    /// `astext().count('\n') >= linenothreshold - 1`. A `doctest_block`
    /// is not stamped, and every `highlightlang` node leaves the tree
    /// ([PYG §3.3]). Probed on Sphinx 9.1.0 under the dummy builder.
    #[test]
    fn highlight_language_stamps_literal_blocks() {
        let (body, records) = resolved(
            "Intro::\n\n   default block\n\n.. code-block:: c\n\n   int x;\n\n.. note::\n\n   \
             .. highlight:: python\n      :linenothreshold: 3\n\n   One::\n\n      a\n      b\n\n\
             Two::\n\n   a\n   b\n   c\n\n.. highlight:: c\n   :force:\n\n.. code-block::\n\n   \
             int y;\n\n.. code-block:: python\n   :force:\n\n   z\n\n.. parsed-literal::\n\n   \
             *parsed* text\n\n>>> doctest()\nout\n",
            BuilderKind::Dummy,
        );
        assert_eq!(
            body,
            "    <paragraph>\n        Intro:\n    \
             <literal_block force=\"0\" language=\"default\" linenos=\"0\" xml:space=\"preserve\">\n        \
             default block\n    \
             <literal_block force=\"0\" highlight_args=\"{}\" language=\"c\" linenos=\"0\" \
             xml:space=\"preserve\">\n        int x;\n    <note>\n        <paragraph>\n            One:\n        \
             <literal_block force=\"0\" language=\"python\" linenos=\"0\" xml:space=\"preserve\">\n            \
             a\n            b\n    <paragraph>\n        Two:\n    \
             <literal_block force=\"0\" language=\"python\" linenos=\"1\" xml:space=\"preserve\">\n        \
             a\n        b\n        c\n    \
             <literal_block force=\"0\" highlight_args=\"{}\" language=\"c\" linenos=\"0\" \
             xml:space=\"preserve\">\n        int y;\n    \
             <literal_block force=\"1\" highlight_args=\"{}\" language=\"python\" linenos=\"0\" \
             xml:space=\"preserve\">\n        z\n    \
             <literal_block force=\"1\" language=\"c\" linenos=\"0\" xml:space=\"preserve\">\n        \
             <emphasis>\n            parsed\n         text\n    \
             <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n        >>> doctest()\n        out\n"
        );
        assert!(records.is_empty(), "{records:?}");
    }

    /// A `highlight` whose `:linenothreshold:` is not positive is the
    /// option ERROR in place of the directive (`positive_int`,
    /// `SP/sphinx/directives/code.py:39-42`): no `highlightlang` node, so
    /// the literal block takes the configuration's language, unnumbered.
    /// Probed on Sphinx 9.1.0 (dummy builder).
    #[test]
    fn a_rejected_highlight_sets_nothing() {
        for threshold in ["0", "-3"] {
            let rst = format!(
                ".. highlight:: python\n   :linenothreshold: {threshold}\n\nText::\n\n   a\n"
            );
            let read = read_standalone(&rst, &index_rst(), &settings(true));
            let (doctree, _) = resolve_standalone(&read, &settings(true), BuilderKind::Dummy);
            let pformat = doctree.root.pformat();
            assert_eq!(
                pformat.split_once('\n').map(|(_, body)| body),
                Some(
                    "    <paragraph>\n        Text:\n    \
                     <literal_block force=\"0\" language=\"default\" linenos=\"0\" \
                     xml:space=\"preserve\">\n        a\n"
                ),
                "{rst:?}"
            );
            let records: Vec<String> = read
                .diagnostics
                .iter()
                .map(|record| record.render(true))
                .collect();
            assert_eq!(
                records,
                [format!(
                    "<SRCDIR>/index.rst:1: ERROR: Error in \"highlight\" directive:\n\
                     invalid option value: (option: \"linenothreshold\"; value: '{threshold}')\n\
                     negative or zero value; must be positive.\n\n\
                     .. highlight:: python\n   :linenothreshold: {threshold} [docutils]"
                )],
                "{rst:?}"
            );
        }
    }

    /// The stamp follows the document's text across an include, a
    /// `literalinclude` without `:language:` loses its own `:force:` to the
    /// setting's, a threshold of 1 numbers every block, and
    /// `highlight_language` is the configuration's. Probed on Sphinx 9.1.0
    /// with `highlight_language = 'rst'`.
    #[test]
    fn highlight_language_follows_includes_and_the_configuration() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let srcdir = crate::utils::canonicalize_simplified(tmp.path()).expect("canonical");
        std::fs::write(srcdir.join("snippet.py"), "print(1)\n").expect("write");
        std::fs::write(
            srcdir.join("part.inc"),
            ".. highlight:: c\n\nInc::\n\n   inc\n",
        )
        .expect("write");
        let mut parse = index_rst();
        parse.source_path = srcdir.join("index.rst").display().to_string();
        parse.srcdir = Some(srcdir.clone());
        parse.read.highlight_language = "rst".to_string();
        let (body, records) = resolved_with(
            ".. literalinclude:: snippet.py\n   :force:\n\n.. highlight:: python\n   \
             :linenothreshold: 1\n\nOne::\n\n   x\n\n.. include:: part.inc\n\nAfter::\n\n   y\n\n\
             .. code-block::\n   :linenos:\n\n   z\n",
            &parse,
            &settings(true),
            BuilderKind::Dummy,
        );
        let root = format!("{}{}", srcdir.display(), std::path::MAIN_SEPARATOR);
        assert_eq!(
            body.replace(&root, "<SRCDIR>/"),
            "    <literal_block force=\"0\" highlight_args=\"{'linenostart': 1}\" language=\"rst\" \
             linenos=\"0\" source=\"<SRCDIR>/snippet.py\" xml:space=\"preserve\">\n        \
             print(1)\n    <paragraph>\n        One:\n    \
             <literal_block force=\"0\" language=\"python\" linenos=\"1\" xml:space=\"preserve\">\n        \
             x\n    <paragraph>\n        Inc:\n    \
             <literal_block force=\"0\" language=\"c\" linenos=\"0\" xml:space=\"preserve\">\n        \
             inc\n    <paragraph>\n        After:\n    \
             <literal_block force=\"0\" language=\"c\" linenos=\"0\" xml:space=\"preserve\">\n        \
             y\n    \
             <literal_block force=\"0\" highlight_args=\"{}\" language=\"c\" linenos=\"1\" \
             xml:space=\"preserve\">\n        z\n"
        );
        assert!(records.is_empty(), "{records:?}");
    }

    /// `TrimDoctestFlagsTransform` (401) strips `<BLANKLINE>` lines and
    /// `# doctest:` comments from every `doctest_block` and from each
    /// literal block a Python console session is in: `pycon` or `pycon3`,
    /// a Python language (or `default`) whose source opens with `>>>`, or
    /// `guess` with a Vim modeline naming `pycon`; never from a
    /// parsed-literal whose text is not its source. With
    /// `trim_doctest_flags = False` nothing is stripped. Probed on Sphinx
    /// 9.1.0 under the dummy builder.
    #[test]
    fn trim_doctest_flags() {
        let (body, _) = resolved(
            ".. code-block:: pycon\n\n   >>> f()  # doctest: +SKIP\n   a\n\n   <BLANKLINE>\n   b\n\n\
             .. code-block:: python\n\n   >>> g() #doctest:+ELLIPSIS   \n   ...\n\nLit::\n\n   \
             >>> h()   # doctest: +NORMALIZE_WHITESPACE\n   <BLANKLINE>\n\nLit2::\n\n   \
             x = 1  # doctest: +SKIP\n\n>>> d()  # doctest: +SKIP\n    <BLANKLINE>\n1\n\n\
             .. parsed-literal::\n\n   >>> *p*()  # doctest: +SKIP\n\n.. parsed-literal::\n\n   \
             >>> q()  # doctest: +SKIP\n\n.. code-block:: guess\n\n   x  # doctest: +SKIP\n   \
             # vim: ft=pycon\n\n.. code-block:: guess\n\n   >>> y  # doctest: +SKIP\n\n\
             .. code-block:: pycon3\n\n   >>> z  # doctest: +SKIP\n\n.. code-block:: text\n\n   \
             >>> t  # doctest: +SKIP\n",
            BuilderKind::Dummy,
        );
        assert_eq!(
            body,
            "    <literal_block force=\"0\" highlight_args=\"{}\" language=\"pycon\" linenos=\"0\" \
             xml:space=\"preserve\">\n        >>> f()\n        a\n        \n        b\n    \
             <literal_block force=\"0\" highlight_args=\"{}\" language=\"python\" linenos=\"0\" \
             xml:space=\"preserve\">\n        >>> g()\n        ...\n    <paragraph>\n        Lit:\n    \
             <literal_block force=\"0\" language=\"default\" linenos=\"0\" xml:space=\"preserve\">\n        \
             >>> h()\n    <paragraph>\n        Lit2:\n    \
             <literal_block force=\"0\" language=\"default\" linenos=\"0\" xml:space=\"preserve\">\n        \
             x = 1  # doctest: +SKIP\n    \
             <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n        >>> d()\n        \n        \
             1\n    \
             <literal_block force=\"0\" language=\"default\" linenos=\"0\" xml:space=\"preserve\">\n        \
             >>> *p*()\n    \
             <literal_block force=\"0\" language=\"default\" linenos=\"0\" xml:space=\"preserve\">\n        \
             >>> q()\n    \
             <literal_block force=\"0\" highlight_args=\"{}\" language=\"guess\" linenos=\"0\" \
             xml:space=\"preserve\">\n        x\n        # vim: ft=pycon\n    \
             <literal_block force=\"0\" highlight_args=\"{}\" language=\"guess\" linenos=\"0\" \
             xml:space=\"preserve\">\n        >>> y  # doctest: +SKIP\n    \
             <literal_block force=\"0\" highlight_args=\"{}\" language=\"pycon3\" linenos=\"0\" \
             xml:space=\"preserve\">\n        >>> z\n    \
             <literal_block force=\"0\" highlight_args=\"{}\" language=\"text\" linenos=\"0\" \
             xml:space=\"preserve\">\n        >>> t  # doctest: +SKIP\n"
        );

        let rst =
            ".. parsed-literal::\n\n   >>> *p* ()  # doctest: +SKIP\n\n.. code-block:: py3\n\n   \
                   >>> a  # doctest: +SKIP\n\n.. code-block:: py\n\n   b  # doctest: +SKIP\n\n\
                   .. highlight:: pycon\n\nLit::\n\n   c  # doctest: +SKIP\n   <BLANKLINE>\n\n\
                   >>> d()  # doctest: +SKIP\n1\n";
        let parsed = "    <literal_block force=\"0\" language=\"default\" linenos=\"0\" \
                      xml:space=\"preserve\">\n        >>> \n        <emphasis>\n            p\n         \
                      ()  # doctest: +SKIP\n";
        let trimmed = |b: &str, c: &str, d: &str| {
            format!(
                "{parsed}    <literal_block force=\"0\" highlight_args=\"{{}}\" language=\"py3\" \
                 linenos=\"0\" xml:space=\"preserve\">\n        {b}\n    \
                 <literal_block force=\"0\" highlight_args=\"{{}}\" language=\"py\" linenos=\"0\" \
                 xml:space=\"preserve\">\n        b  # doctest: +SKIP\n    <paragraph>\n        \
                 Lit:\n    <literal_block force=\"0\" language=\"pycon\" linenos=\"0\" \
                 xml:space=\"preserve\">\n        {c}\n    \
                 <doctest_block classes=\"doctest\" xml:space=\"preserve\">\n        {d}\n        1\n"
            )
        };
        assert_eq!(
            resolved_with(rst, &index_rst(), &settings(true), BuilderKind::Dummy).0,
            trimmed(">>> a", "c", ">>> d()")
        );
        assert_eq!(
            resolved_with(rst, &index_rst(), &settings(false), BuilderKind::Dummy).0,
            trimmed(
                ">>> a  # doctest: +SKIP",
                "c  # doctest: +SKIP\n        <BLANKLINE>",
                ">>> d()  # doctest: +SKIP"
            )
        );
    }
}
