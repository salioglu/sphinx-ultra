//! The role dispatch against Sphinx 9.1.0 and docutils 0.22.4 ([RC
//! §2.2]): the lookup of every name a default build registers, and what a
//! parse makes of the roles the dispatch found (or did not).

use super::*;
use crate::doctree::{kinds, AttrValue, Node};
use crate::rst::{parse_rst_full, ParseLogWarning, ParseOptions, ParseOutput, ReadConfig};
use crate::transforms::reporter::LogRecord;

use RoleFn::*;
use WithoutDefault::*;

/// How a name resolves with no default domain, against with `py`.
enum WithoutDefault {
    Same,
    /// The role found (`None`: none) and whether docutils' lookup created
    /// its INFO.
    NoDefault(Option<RoleFn>, bool),
}

/// Sphinx 9.1.0's lookup (`sphinx_domains.role` over docutils'
/// `roles.role`) of every role name a default html build registers — the
/// local registry, the canonical registry, the English aliases, and every
/// domain role with and without its prefix ([RC §2.1]) — plus the prefix
/// forms and spellings [RC §2.2] names, with `py` as the default domain:
/// the role found (`None`: none), the same with no default domain, whether
/// docutils' lookup created its INFO, and whether `unknown role name` was
/// logged. Probed: `roles.role(name, en, 0, reporter)` inside
/// `sphinx_domains(env)` of a default `dummy` app, with
/// `env.current_document.default_domain` set to `py` and to `None`, and
/// docutils' `_roles` restored after every lookup.
#[rustfmt::skip]
const SPHINX_LOOKUPS: &[(&str, Option<RoleFn>, WithoutDefault, bool, bool)] = &[
    ("abbr", Some(Unported("abbr")), Same, false, false),
    ("any", Some(Any), Same, false, false),
    ("command", Some(Unported("command")), Same, false, false),
    ("cve", Some(Cve), Same, false, false),
    ("cwe", Some(Cwe), Same, false, false),
    ("dfn", Some(Unported("dfn")), Same, false, false),
    ("download", Some(Unported("download")), Same, false, false),
    ("eq", Some(Eq), Same, false, false),
    ("file", Some(Unported("file")), Same, false, false),
    ("guilabel", Some(Unported("guilabel")), Same, false, false),
    ("index", Some(Index), Same, false, false),
    ("kbd", Some(Unported("kbd")), Same, false, false),
    ("mailheader", Some(Unported("mailheader")), Same, false, false),
    ("makevar", Some(Unported("makevar")), Same, false, false),
    ("manpage", Some(Unported("manpage")), Same, false, false),
    ("menuselection", Some(Unported("menuselection")), Same, false, false),
    ("mimetype", Some(Unported("mimetype")), Same, false, false),
    ("newsgroup", Some(Unported("newsgroup")), Same, false, false),
    ("pep", Some(Pep), Same, false, false),
    ("program", Some(Unported("program")), Same, false, false),
    ("regexp", Some(Unported("regexp")), Same, false, false),
    ("rfc", Some(Rfc), Same, false, false),
    ("samp", Some(Unported("samp")), Same, false, false),
    ("abbreviation", Some(Generic("abbreviation")), Same, false, false),
    ("acronym", Some(Generic("acronym")), Same, false, false),
    ("anonymous-reference", Some(Unimplemented), Same, false, false),
    ("citation-reference", Some(Unimplemented), Same, false, false),
    ("code", Some(SphinxCode), Same, false, false),
    ("emphasis", Some(Generic("emphasis")), Same, false, false),
    ("footnote-reference", Some(Unimplemented), Same, false, false),
    ("literal", Some(Generic("literal")), Same, false, false),
    ("math", Some(Math), Same, false, false),
    ("named-reference", Some(Unimplemented), Same, false, false),
    ("pep-reference", Some(PepReference), Same, false, false),
    ("raw", Some(Raw), Same, false, false),
    ("restructuredtext-unimplemented-role", Some(Unimplemented), Same, true, false),
    ("rfc-reference", Some(RfcReference), Same, false, false),
    ("strong", Some(Generic("strong")), Same, false, false),
    ("subscript", Some(Generic("subscript")), Same, false, false),
    ("substitution-reference", Some(Unimplemented), Same, false, false),
    ("superscript", Some(Generic("superscript")), Same, false, false),
    ("target", Some(Unimplemented), Same, false, false),
    ("title-reference", Some(Generic("title_reference")), Same, false, false),
    ("uri-reference", Some(Unimplemented), Same, false, false),
    ("ab", Some(Generic("abbreviation")), Same, false, false),
    ("ac", Some(Generic("acronym")), Same, false, false),
    ("i", Some(Unimplemented), Same, false, false),
    ("sub", Some(Generic("subscript")), Same, false, false),
    ("sup", Some(Generic("superscript")), Same, false, false),
    ("t", Some(Generic("title_reference")), Same, false, false),
    ("title", Some(Generic("title_reference")), Same, false, false),
    ("uri", Some(Unimplemented), Same, false, false),
    ("url", Some(Unimplemented), Same, false, false),
    ("c:data", Some(Domain(DomainId::C, "data")), Same, false, false),
    ("c:enum", Some(Domain(DomainId::C, "enum")), Same, false, false),
    ("c:enumerator", Some(Domain(DomainId::C, "enumerator")), Same, false, false),
    ("c:expr", Some(Domain(DomainId::C, "expr")), Same, false, false),
    ("c:func", Some(Domain(DomainId::C, "func")), Same, false, false),
    ("c:macro", Some(Domain(DomainId::C, "macro")), Same, false, false),
    ("c:member", Some(Domain(DomainId::C, "member")), Same, false, false),
    ("c:struct", Some(Domain(DomainId::C, "struct")), Same, false, false),
    ("c:texpr", Some(Domain(DomainId::C, "texpr")), Same, false, false),
    ("c:type", Some(Domain(DomainId::C, "type")), Same, false, false),
    ("c:union", Some(Domain(DomainId::C, "union")), Same, false, false),
    ("c:var", Some(Domain(DomainId::C, "var")), Same, false, false),
    ("cpp:any", Some(Domain(DomainId::Cpp, "any")), Same, false, false),
    ("cpp:class", Some(Domain(DomainId::Cpp, "class")), Same, false, false),
    ("cpp:concept", Some(Domain(DomainId::Cpp, "concept")), Same, false, false),
    ("cpp:enum", Some(Domain(DomainId::Cpp, "enum")), Same, false, false),
    ("cpp:enumerator", Some(Domain(DomainId::Cpp, "enumerator")), Same, false, false),
    ("cpp:expr", Some(Domain(DomainId::Cpp, "expr")), Same, false, false),
    ("cpp:func", Some(Domain(DomainId::Cpp, "func")), Same, false, false),
    ("cpp:member", Some(Domain(DomainId::Cpp, "member")), Same, false, false),
    ("cpp:struct", Some(Domain(DomainId::Cpp, "struct")), Same, false, false),
    ("cpp:texpr", Some(Domain(DomainId::Cpp, "texpr")), Same, false, false),
    ("cpp:type", Some(Domain(DomainId::Cpp, "type")), Same, false, false),
    ("cpp:union", Some(Domain(DomainId::Cpp, "union")), Same, false, false),
    ("cpp:var", Some(Domain(DomainId::Cpp, "var")), Same, false, false),
    ("js:attr", Some(Domain(DomainId::Js, "attr")), Same, false, false),
    ("js:class", Some(Domain(DomainId::Js, "class")), Same, false, false),
    ("js:data", Some(Domain(DomainId::Js, "data")), Same, false, false),
    ("js:func", Some(Domain(DomainId::Js, "func")), Same, false, false),
    ("js:meth", Some(Domain(DomainId::Js, "meth")), Same, false, false),
    ("js:mod", Some(Domain(DomainId::Js, "mod")), Same, false, false),
    ("math:numref", Some(Domain(DomainId::Math, "numref")), Same, false, false),
    ("py:attr", Some(Domain(DomainId::Py, "attr")), Same, false, false),
    ("py:class", Some(Domain(DomainId::Py, "class")), Same, false, false),
    ("py:const", Some(Domain(DomainId::Py, "const")), Same, false, false),
    ("py:data", Some(Domain(DomainId::Py, "data")), Same, false, false),
    ("py:deco", Some(Domain(DomainId::Py, "deco")), Same, false, false),
    ("py:exc", Some(Domain(DomainId::Py, "exc")), Same, false, false),
    ("py:func", Some(Domain(DomainId::Py, "func")), Same, false, false),
    ("py:meth", Some(Domain(DomainId::Py, "meth")), Same, false, false),
    ("py:mod", Some(Domain(DomainId::Py, "mod")), Same, false, false),
    ("py:obj", Some(Domain(DomainId::Py, "obj")), Same, false, false),
    ("py:type", Some(Domain(DomainId::Py, "type")), Same, false, false),
    ("rst:dir", Some(Domain(DomainId::Rst, "dir")), Same, false, false),
    ("rst:role", Some(Domain(DomainId::Rst, "role")), Same, false, false),
    ("std:confval", Some(Domain(DomainId::Std, "confval")), Same, false, false),
    ("std:doc", Some(Domain(DomainId::Std, "doc")), Same, false, false),
    ("std:envvar", Some(Domain(DomainId::Std, "envvar")), Same, false, false),
    ("std:keyword", Some(Domain(DomainId::Std, "keyword")), Same, false, false),
    ("std:numref", Some(Domain(DomainId::Std, "numref")), Same, false, false),
    ("std:option", Some(Domain(DomainId::Std, "option")), Same, false, false),
    ("std:ref", Some(Domain(DomainId::Std, "ref")), Same, false, false),
    ("std:term", Some(Domain(DomainId::Std, "term")), Same, false, false),
    ("std:token", Some(Domain(DomainId::Std, "token")), Same, false, false),
    ("attr", Some(Domain(DomainId::Py, "attr")), NoDefault(None, true), false, false),
    ("class", Some(Domain(DomainId::Py, "class")), NoDefault(None, true), false, false),
    ("concept", None, Same, true, false),
    ("confval", Some(Domain(DomainId::Std, "confval")), Same, false, false),
    ("const", Some(Domain(DomainId::Py, "const")), NoDefault(None, true), false, false),
    ("data", Some(Domain(DomainId::Py, "data")), NoDefault(None, true), false, false),
    ("deco", Some(Domain(DomainId::Py, "deco")), NoDefault(None, true), false, false),
    ("dir", None, Same, true, false),
    ("doc", Some(Domain(DomainId::Std, "doc")), Same, false, false),
    ("enum", None, Same, true, false),
    ("enumerator", None, Same, true, false),
    ("envvar", Some(Domain(DomainId::Std, "envvar")), Same, false, false),
    ("exc", Some(Domain(DomainId::Py, "exc")), NoDefault(None, true), false, false),
    ("expr", None, Same, true, false),
    ("func", Some(Domain(DomainId::Py, "func")), NoDefault(None, true), false, false),
    ("keyword", Some(Domain(DomainId::Std, "keyword")), Same, false, false),
    ("macro", None, Same, true, false),
    ("member", None, Same, true, false),
    ("meth", Some(Domain(DomainId::Py, "meth")), NoDefault(None, true), false, false),
    ("mod", Some(Domain(DomainId::Py, "mod")), NoDefault(None, true), false, false),
    ("numref", Some(Domain(DomainId::Std, "numref")), Same, false, false),
    ("obj", Some(Domain(DomainId::Py, "obj")), NoDefault(None, true), false, false),
    ("option", Some(Domain(DomainId::Std, "option")), Same, false, false),
    ("ref", Some(Domain(DomainId::Std, "ref")), Same, false, false),
    ("role", None, Same, true, false),
    ("struct", None, Same, true, false),
    ("term", Some(Domain(DomainId::Std, "term")), Same, false, false),
    ("texpr", None, Same, true, false),
    ("token", Some(Domain(DomainId::Std, "token")), Same, false, false),
    ("type", Some(Domain(DomainId::Py, "type")), NoDefault(None, true), false, false),
    ("union", None, Same, true, false),
    ("var", None, Same, true, false),
    ("foo:ref", Some(Domain(DomainId::Std, "ref")), Same, false, true),
    ("py:ref", Some(Domain(DomainId::Std, "ref")), Same, false, false),
    ("std:kbd", None, Same, true, false),
    ("KBD", Some(Unported("kbd")), Same, false, false),
    ("Py:Func", Some(Domain(DomainId::Py, "func")), Same, false, false),
    ("bogus", None, Same, true, false),
    ("a:b:c", None, Same, true, true),
    ("math:sub", None, Same, true, false),
    ("external:std:ref", None, Same, true, true),
    ("external+inv:py:func", None, Same, true, true),
    ("std:program", None, Same, true, false),
    ("I", Some(Unimplemented), Same, false, false),
    ("Pep-Reference", Some(PepReference), Same, false, false),
    ("c:option", Some(Domain(DomainId::Std, "option")), Same, false, false),
    ("py:kbd", None, Same, true, false),
    ("", Some(Generic("title_reference")), Same, false, false),
];

/// One lookup against a fresh document state: the resolution and the
/// logger records it left.
fn resolve(ctx: DispatchCtx, written: &str) -> (Resolution, Vec<ParseLogWarning>) {
    let table = if ctx.sphinx {
        RoleTable::sphinx()
    } else {
        RoleTable::docutils()
    };
    let mut state = DocumentState::default();
    let resolution = resolve_role(&table, &ctx, written, &mut state);
    (resolution, state.parse.log_warnings)
}

/// docutils' INFO for a name its English table does not hold.
fn info_text(name: &str) -> String {
    format!(
        "No role entry for \"{name}\" in module \"docutils.parsers.rst.languages.en\".\n\
         Trying \"{name}\" as canonical role name."
    )
}

/// The resolution of `written` when the lookup finds `role` (`None`: no
/// role) with or without docutils' INFO, which names `looked_up`: a domain
/// role is called with its full name, any other with the name as written.
fn expected(written: &str, looked_up: &str, role: Option<RoleFn>, info: bool) -> Resolution {
    let info = info.then(|| info_text(looked_up));
    match role {
        Some(role @ Domain(domain, name)) => Resolution::Found {
            role,
            call_name: format!("{}:{name}", domain.name()),
            info,
        },
        Some(role) => Resolution::Found {
            role,
            call_name: written.to_string(),
            info,
        },
        None => Resolution::NotFound { info },
    }
}

/// The `unknown role name: X` record, with no location, as the first
/// record of a document (seq 0).
fn unknown_role_name(lowered: &str) -> Vec<ParseLogWarning> {
    vec![ParseLogWarning {
        message: format!("unknown role name: {lowered}"),
        location: LogLocation::None,
        seq: 0,
    }]
}

fn sphinx_ctx(default_domain: Option<DomainId>, intersphinx: bool) -> DispatchCtx {
    DispatchCtx {
        sphinx: true,
        default_domain,
        intersphinx,
    }
}

/// [RC §2.2]: every name of the Sphinx inventory with `py` and with no
/// default domain, the docutils lookup of a docutils parse, and the
/// `external:` names with and without `sphinx.ext.intersphinx`
/// (`html_projects/intersphinx`, [TW F12]). In a Sphinx parse the INFO
/// names the lowercased name, which `sphinx_domains.role` passes on; the
/// role is still called with the name as written (`Inliner.interpreted`).
#[test]
fn resolve_role_matches_sphinx_domains_role() {
    for (written, role, without_default, info, warns) in SPHINX_LOOKUPS {
        let lowered = written.to_lowercase();
        let logged = if *warns {
            unknown_role_name(&lowered)
        } else {
            Vec::new()
        };
        assert_eq!(
            resolve(sphinx_ctx(Some(DomainId::Py), false), written),
            (expected(written, &lowered, *role, *info), logged.clone()),
            "{written:?} with the py default domain"
        );
        let (role, info) = match without_default {
            Same => (*role, *info),
            NoDefault(role, info) => (*role, *info),
        };
        assert_eq!(
            resolve(sphinx_ctx(None, false), written),
            (expected(written, &lowered, role, info), logged),
            "{written:?} with no default domain"
        );
    }

    // A docutils parse: no domain dispatch and no Sphinx registration; the
    // name keeps its case in the INFO.
    let docutils = DispatchCtx {
        sphinx: false,
        default_domain: None,
        intersphinx: false,
    };
    for (written, role, info) in [
        ("code", Some(DocutilsCode), false),
        ("CODE", Some(DocutilsCode), false),
        ("pep", Some(PepReference), false),
        ("RFC", Some(RfcReference), false),
        ("i", Some(Unimplemented), false),
        (
            "restructuredtext-unimplemented-role",
            Some(Unimplemented),
            true,
        ),
        ("", Some(Generic(kinds::TITLE_REFERENCE)), false),
        ("kbd", None, true),
        ("KBD", None, true),
        ("py:func", None, true),
        ("foo:ref", None, true),
        ("external:std:ref", None, true),
    ] {
        assert_eq!(
            resolve(docutils, written),
            (expected(written, written, role, info), Vec::new()),
            "{written:?} in docutils mode"
        );
    }

    // `sphinx.ext.intersphinx` claims `external:` and `external+` names as
    // written, longer than the bare prefix; without it, and for anything
    // else, `External:` included, the name goes through the domains, where
    // `external` is unknown and the std domain gets the rest (`ref`, as
    // for `foo:ref`).
    for (written, without) in [
        ("external:std:ref", None),
        ("external+inv:py:func", None),
        ("external:ref", Some(Domain(DomainId::Std, "ref"))),
    ] {
        assert_eq!(
            resolve(sphinx_ctx(Some(DomainId::Py), true), written),
            (
                Resolution::Found {
                    role: External,
                    call_name: written.to_string(),
                    info: None,
                },
                Vec::new()
            ),
            "{written:?} with intersphinx"
        );
        assert_eq!(
            resolve(sphinx_ctx(Some(DomainId::Py), false), written),
            (
                expected(written, written, without, without.is_none()),
                unknown_role_name(written)
            ),
            "{written:?} without intersphinx"
        );
    }
    for written in ["External:std:ref", "external:"] {
        let lowered = written.to_lowercase();
        assert_eq!(
            resolve(sphinx_ctx(Some(DomainId::Py), true), written),
            (
                expected(written, &lowered, None, true),
                unknown_role_name(&lowered)
            ),
            "{written:?} with intersphinx"
        );
    }

    // What a parse makes of an unknown role: docutils' ERROR over a
    // `problematic`, never a `pending_xref` (`docutils-0264`…`-0267`,
    // `-0278`, `-0605`, `-0606`, `html_projects/intersphinx`).
    for (source, expected) in UNKNOWN_ROLE_TREES {
        assert_eq!(
            read_tree(source, ReadConfig::default()),
            *expected,
            "{source}"
        );
    }
    // ... and with `sphinx.ext.intersphinx` loaded, the `:external:` role.
    let read = ReadConfig {
        intersphinx: true,
        ..ReadConfig::default()
    };
    let out = parse(UNKNOWN_ROLE_TREES[7].0, read);
    let xref = find(&out.doctree.root, "pending_xref").expect("the external xref");
    assert_eq!(xref.get("intersphinx"), Some(&AttrValue::Int(1)));
    assert!(out.registry.log_warnings.is_empty());
}

/// A Sphinx-mode parse of `source` as `<SRC>/index.rst`, the path the
/// probes print.
pub(crate) fn parse(source: &str, read: ReadConfig) -> ParseOutput {
    parse_rst_full(
        source,
        &ParseOptions {
            source_path: "<SRC>/index.rst".to_string(),
            sphinx: true,
            read,
            ..ParseOptions::default()
        },
    )
}

/// The pformat of [`parse`]'s tree as the probes print it: they read with
/// `keep_warnings = True`, under which `FilterSystemMessages` (999, T9b)
/// removes the INFO messages.
pub(crate) fn read_tree(source: &str, read: ReadConfig) -> String {
    fn drop_info(node: &mut Node) {
        node.children.retain(|child| {
            child.kind != kinds::SYSTEM_MESSAGE
                || !matches!(child.get("level"), Some(AttrValue::Int(1)))
        });
        node.children.iter_mut().for_each(drop_info);
    }
    let mut root = parse(source, read).doctree.root;
    drop_info(&mut root);
    root.pformat()
}

fn find<'a>(node: &'a Node, kind: &str) -> Option<&'a Node> {
    if node.kind == kind {
        return Some(node);
    }
    node.children.iter().find_map(|child| find(child, kind))
}

/// Sphinx 9.1.0's post-read trees of the unknown-role inputs (probed with
/// `keep_warnings = True`).
const UNKNOWN_ROLE_TREES: [(&str, &str); 8] = [
    (
        ":bogus:`x`\n\nnext para here\n",
        "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :bogus:`x`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"bogus\".\n    <paragraph>\n        next para here\n",
    ),
    (
        ":BoGuS:`x` end.\n",
        "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :BoGuS:`x`\n         end.\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"BoGuS\".\n",
    ),
    (
        ":bogus:`x` :bogus:`y`\n",
        "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :bogus:`x`\n         \n        <problematic ids=\"id4\" refid=\"id3\">\n            :bogus:`y`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"bogus\".\n    <system_message backrefs=\"id4\" ids=\"id3\" level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"bogus\".\n",
    ),
    (
        "`x`:bogus: end.\n",
        "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            `x`:bogus:\n         end.\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"bogus\".\n",
    ),
    (
        "Para.\n\n    quoted :bogus:`x` here\n",
        "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        Para.\n    <block_quote>\n        <paragraph>\n            quoted \n            <problematic ids=\"id2\" refid=\"id1\">\n                :bogus:`x`\n             here\n        <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"3\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"bogus\".\n",
    ),
    (
        ":a:b:c:`text` end.\n",
        "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :a:b:c:`text`\n         end.\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"a:b:c\".\n",
    ),
    (
        ":math:sub:`text` end.\n",
        "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :math:sub:`text`\n         end.\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"math:sub\".\n",
    ),
    (
        "A target that exists in no inventory: :external:std:ref:`whatever`.\n",
        "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        A target that exists in no inventory: \n        <problematic ids=\"id2\" refid=\"id1\">\n            :external:std:ref:`whatever`\n        .\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"external:std:ref\".\n",
    ),
];

/// `docutils-0605`: the `unknown role name` logger record of
/// `sphinx_domains.role` comes before the reporter messages docutils'
/// lookup and the inliner then create, so its seq is lower than the
/// ERROR's (`WARNING: unknown role name: a:b:c` prints first).
#[test]
fn parse_records_interleave_by_seq() {
    let out = parse(":a:b:c:`text` end.\n", ReadConfig::default());
    let [warning] = out.registry.log_warnings.as_slice() else {
        panic!("one logger record: {:?}", out.registry.log_warnings);
    };
    assert_eq!(
        (warning.message.as_str(), warning.location),
        ("unknown role name: a:b:c", LogLocation::None)
    );
    let reporter: Vec<(u32, u8, String)> = out
        .state
        .log
        .iter()
        .filter_map(|record| match record {
            LogRecord::Reporter {
                key, level, text, ..
            } => Some((key.seq, *level, text.clone())),
            LogRecord::Logger { .. } => None,
        })
        .collect();
    let error = reporter
        .iter()
        .find(|(_, level, _)| *level == crate::doctree::messages::ERROR)
        .expect("the ERROR");
    assert_eq!(error.2, "Unknown interpreted text role \"a:b:c\".");
    assert!(
        reporter.iter().all(|(seq, _, _)| warning.seq < *seq),
        "the logger record ({}) precedes every reporter record: {reporter:?}",
        warning.seq
    );
}

/// The long PEP and RFC role names are docutils' canonical roles, which
/// Sphinx does not replace: a plain `reference` with no `index` entry, so
/// the short names' `index-N` serials start at 0 (`docutils-0268`,
/// `-0270`, [RC §3.10]). The RFC base URL is Sphinx's
/// (`SP/sphinx/environment/__init__.py:66`). Probed on Sphinx 9.1.0 with
/// `keep_warnings = True`.
#[test]
fn pep_reference_is_docutils_plain_reference() {
    for (source, expected) in [
        (
            ":pep-reference:`8` and :PEP:`0` and :pep:`008` end.\n",
            "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <reference refuri=\"https://peps.python.org/pep-0008\">\n            PEP 8\n         and \n        <index entries=\"('single',\\ 'Python\\ Enhancement\\ Proposals;\\ PEP\\ 0',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        <reference classes=\"pep\" internal=\"0\" refuri=\"https://peps.python.org/pep-0000/\">\n            <strong>\n                PEP 0\n         and \n        <index entries=\"('single',\\ 'Python\\ Enhancement\\ Proposals;\\ PEP\\ 008',\\ 'index-1',\\ '',\\ None)\">\n        <target ids=\"index-1\">\n        <reference classes=\"pep\" internal=\"0\" refuri=\"https://peps.python.org/pep-0008/\">\n            <strong>\n                PEP 008\n         end.\n",
        ),
        (
            ":rfc-reference:`2822` and :RFC:`0002822` and :RFC:`1` end.\n",
            "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <reference refuri=\"https://datatracker.ietf.org/doc/html/rfc2822.html\">\n            RFC 2822\n         and \n        <index entries=\"('single',\\ 'RFC;\\ RFC\\ 0002822',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        <reference classes=\"rfc\" internal=\"0\" refuri=\"https://datatracker.ietf.org/doc/html/rfc2822.html\">\n            <strong>\n                RFC 0002822\n         and \n        <index entries=\"('single',\\ 'RFC;\\ RFC\\ 1',\\ 'index-1',\\ '',\\ None)\">\n        <target ids=\"index-1\">\n        <reference classes=\"rfc\" internal=\"0\" refuri=\"https://datatracker.ietf.org/doc/html/rfc1.html\">\n            <strong>\n                RFC 1\n         end.\n",
        ),
        (
            ":pep-reference:`10000`\n\n:RFC-Reference:`0`\n\n:rfc-reference:`2822#sec-1`\n",
            "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :pep-reference:`10000`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            PEP number must be a number from 0 to 9999; \"10000\" is invalid.\n    <paragraph>\n        <problematic ids=\"id4\" refid=\"id3\">\n            :RFC-Reference:`0`\n    <system_message backrefs=\"id4\" ids=\"id3\" level=\"3\" line=\"3\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            RFC number must be a number greater than or equal to 1; \"0\" is invalid.\n    <paragraph>\n        <reference refuri=\"https://datatracker.ietf.org/doc/html/rfc2822.html#sec-1\">\n            RFC 2822\n",
        ),
        (
            ":rfc-reference:`0#x`\n",
            "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :rfc-reference:`0#x`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            RFC number must be a number greater than or equal to 1; \"0#x\" is invalid.\n",
        ),
    ] {
        assert_eq!(read_tree(source, ReadConfig::default()), expected, "{source}");
    }
}

/// Sphinx's `:index:` role ([RC §3.9]): an `index` node with no `inline`
/// attribute, a `target` whose `index-N` id is not registered (so the
/// later section takes `index-0` itself, P22), and the title as text; an
/// explicit target is a full entry (`process_index_entry`), a leading `!`
/// marks an implicit one main. `:i:` and `:I:` are docutils' alias of the
/// unimplemented canonical `index`, reported as written. The explicit
/// title is `ReferenceRole`'s split, on the escaped text: an escaped `<`
/// cannot open the target, and the first `<` that can does (`docutils-0276`
/// and the P02, P11 and P22 probes, Sphinx 9.1.0, `keep_warnings = True`).
#[test]
fn index_role_matches_sphinx() {
    for (source, expected) in [
        (
            ":index:`x` end.\n",
            "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <index entries=\"('single',\\ 'x',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        x\n         end.\n",
        ),
        (
            "Para :index:`term` here.\n\n:index:`!main term`\n\n:index:`see <single: a; b>`\n\n\
             :index:`pair <pair: x; y>`\n\n:index:`bad <bogus: x>`\n\n:index:`text <x>`\n\n\
             :I:`upper`\n\n:i:`alias`\n",
            "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        Para \n        <index entries=\"('single',\\ 'term',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        term\n         here.\n    <paragraph>\n        <index entries=\"('single',\\ 'main\\ term',\\ 'index-1',\\ 'main',\\ None)\">\n        <target ids=\"index-1\">\n        main term\n    <paragraph>\n        <index entries=\"('single',\\ 'a;\\ b',\\ 'index-2',\\ '',\\ None)\">\n        <target ids=\"index-2\">\n        see\n    <paragraph>\n        <index entries=\"('pair',\\ 'x;\\ y',\\ 'index-3',\\ '',\\ None)\">\n        <target ids=\"index-3\">\n        pair\n    <paragraph>\n        <index entries=\"('single',\\ 'bogus:\\ x',\\ 'index-4',\\ '',\\ None)\">\n        <target ids=\"index-4\">\n        bad\n    <paragraph>\n        <index entries=\"('single',\\ 'x',\\ 'index-5',\\ '',\\ None)\">\n        <target ids=\"index-5\">\n        text\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :I:`upper`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"13\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Interpreted text role \"I\" not implemented.\n    <paragraph>\n        <problematic ids=\"id4\" refid=\"id3\">\n            :i:`alias`\n    <system_message backrefs=\"id4\" ids=\"id3\" level=\"3\" line=\"15\" source=\"<SRC>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Interpreted text role \"i\" not implemented.\n",
        ),
        (
            ":index:`t <a, !b, , c>`\n\n:index:`!x`\n\n:index:`! spaced`\n\n\
             :index:`see also <seealso: p; q>`\n\n:index:`d <double: m; n>`\n\n:index:`! <!z>`\n",
            "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <index entries=\"('single',\\ 'a',\\ 'index-0',\\ '',\\ None) ('single',\\ 'b',\\ 'index-0',\\ 'main',\\ None) ('single',\\ 'c',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        t\n    <paragraph>\n        <index entries=\"('single',\\ 'x',\\ 'index-1',\\ 'main',\\ None)\">\n        <target ids=\"index-1\">\n        x\n    <paragraph>\n        <index entries=\"('single',\\ '\\ spaced',\\ 'index-2',\\ 'main',\\ None)\">\n        <target ids=\"index-2\">\n         spaced\n    <paragraph>\n        <index entries=\"('seealso',\\ 'p;\\ q',\\ 'index-3',\\ '',\\ None)\">\n        <target ids=\"index-3\">\n        see also\n    <paragraph>\n        <index entries=\"('pair',\\ 'm;\\ n',\\ 'index-4',\\ '',\\ None)\">\n        <target ids=\"index-4\">\n        d\n    <paragraph>\n        <index entries=\"('single',\\ 'z',\\ 'index-5',\\ 'main',\\ None)\">\n        <target ids=\"index-5\">\n        !\n",
        ),
        (
            "T\n=\n\n:index:`x`\n\nIndex 0\n-------\n\ntext\n",
            "<document source=\"<SRC>/index.rst\">\n    <section ids=\"t\" names=\"t\">\n        <title>\n            T\n        <paragraph>\n            <index entries=\"('single',\\ 'x',\\ 'index-0',\\ '',\\ None)\">\n            <target ids=\"index-0\">\n            x\n        <section ids=\"index-0\" names=\"index\\ 0\">\n            <title>\n                Index 0\n            <paragraph>\n                text\n",
        ),
        (
            ":index:`a\\<b>` :index:`x <y> <z>` :index:`\\\\!q`\n",
            "<document source=\"<SRC>/index.rst\">\n    <paragraph>\n        <index entries=\"('single',\\ 'a<b>',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        a<b>\n         \n        <index entries=\"('single',\\ 'y>\\ <z',\\ 'index-1',\\ '',\\ None)\">\n        <target ids=\"index-1\">\n        x\n         \n        <index entries=\"('single',\\ '\\\\\\\\!q',\\ 'index-2',\\ '',\\ None)\">\n        <target ids=\"index-2\">\n        \\!q\n",
        ),
    ] {
        assert_eq!(read_tree(source, ReadConfig::default()), expected, "{source}");
    }
}
