//! The Sphinx role functions against Sphinx 9.1.0 ([RC §3, §4.2]): one row
//! per probe line of the research report, each read as
//! `<SRCDIR>/index.rst` the way the snippet fixture reads one
//! (`keep_warnings` on, smart quotes off), its expected value the body of
//! the post-read doctree Sphinx's read gives the same line
//! (`tools/gen_sphinx_fixture.py`'s harness; the report's pformat blocks).
//! The `download` rows are the role's half: no file they name exists, so
//! the download collector (T17c) adds nothing to Sphinx's tree either.

use crate::rst::{parse_rst_full, LogLocation, ParseLogWarning, ParseOptions, ReadConfig};
use crate::transforms::test_support::{harness_settings, index_rst, DOC};

/// The body of our post-read doctree of `rst` under `read` (the lines
/// after the document line), and the warning stream the read printed.
fn read(rst: &str, read: ReadConfig) -> (String, Vec<String>) {
    let parse = ParseOptions {
        read,
        ..index_rst()
    };
    let read = crate::read::read_standalone(rst, &parse, &harness_settings(true));
    let pformat = read.outcome.doctree().root.pformat();
    let body = pformat
        .strip_prefix(DOC)
        .unwrap_or_else(|| panic!("{rst:?}: unexpected document line:\n{pformat}"))
        .to_string();
    let warnings = read
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.render(true))
        .collect();
    (body, warnings)
}

/// Every row of `rows` whose body differs from ours under `config`, as a
/// report naming the input.
fn mismatches(rows: &[(&str, &str)], config: &ReadConfig) -> Vec<String> {
    rows.iter()
        .filter_map(|(rst, expected)| {
            let (body, _) = read(rst, config.clone());
            (body != *expected)
                .then(|| format!("--- {rst:?}\n--- sphinx 9.1.0\n{expected}--- ours\n{body}"))
        })
        .collect()
}

/// Asserts every row of `rows` reads as Sphinx's under `config`.
fn check(rows: &[(&str, &str)], config: ReadConfig) {
    let mismatches = mismatches(rows, &config);
    assert!(
        mismatches.is_empty(),
        "{} of {} rows differ from Sphinx 9.1.0:\n{}",
        mismatches.len(),
        rows.len(),
        mismatches.join("\n")
    );
}

/// The crash a parse of `rst` under `read` keeps, if any (decision 13).
fn crash(rst: &str, read: ReadConfig) -> Option<(String, String)> {
    let parse = ParseOptions {
        read,
        ..index_rst()
    };
    parse_rst_full(rst, &parse)
        .crash
        .map(|crash| (crash.exception.to_string(), crash.message))
}

#[rustfmt::skip]
const GENERIC: &[(&str, &str)] = &[
    (":command:`ls -l`\n",
     "    <paragraph>\n        <literal_strong classes=\"command\">\n            ls -l\n"),
    (":dfn:`term`\n",
     "    <paragraph>\n        <emphasis classes=\"dfn\">\n            term\n"),
    (":mailheader:`Content-Type`\n",
     "    <paragraph>\n        <literal_emphasis classes=\"mailheader\">\n            Content-Type\n"),
    (":makevar:`CFLAGS`\n",
     "    <paragraph>\n        <literal_strong classes=\"makevar\">\n            CFLAGS\n"),
    (":mimetype:`text/plain`\n",
     "    <paragraph>\n        <literal_emphasis classes=\"mimetype\">\n            text/plain\n"),
    (":newsgroup:`comp.lang.python`\n",
     "    <paragraph>\n        <literal_emphasis classes=\"newsgroup\">\n            comp.lang.python\n"),
    (":program:`sphinx-build`\n",
     "    <paragraph>\n        <literal_strong classes=\"program\">\n            sphinx-build\n"),
    (":regexp:`^a.*b$`\n",
     "    <paragraph>\n        <literal classes=\"regexp\">\n            ^a.*b$\n"),
    (":command:`a\\*b \\`x\\``\n",
     "    <paragraph>\n        <literal_strong classes=\"command\">\n            a*b `x`\n"),
    (":program:`!prog` :dfn:`a <b>`\n",
     "    <paragraph>\n        <literal_strong classes=\"program\">\n            !prog\n         \n        <emphasis classes=\"dfn\">\n            a <b>\n"),
    (":COMMAND:`upper` `suffix`:makevar:\n",
     "    <paragraph>\n        <literal_strong classes=\"command\">\n            upper\n         \n        <literal_strong classes=\"makevar\">\n            suffix\n"),
];

#[rustfmt::skip]
const KBD: &[(&str, &str)] = &[
    (":kbd:`Ctrl`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Ctrl\n"),
    (":kbd:`Ctrl+C`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Ctrl\n        +\n        <literal classes=\"kbd\">\n            C\n"),
    (":kbd:`Ctrl+Alt+Del`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Ctrl\n        +\n        <literal classes=\"kbd\">\n            Alt\n        +\n        <literal classes=\"kbd\">\n            Del\n"),
    (":kbd:`Ctrl-x Ctrl-f`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Ctrl\n        -\n        <literal classes=\"kbd\">\n            x\n         \n        <literal classes=\"kbd\">\n            Ctrl\n        -\n        <literal classes=\"kbd\">\n            f\n"),
    (":kbd:`Shift ^ A`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Shift\n         \n        <literal classes=\"kbd\">\n        ^\n        <literal classes=\"kbd\">\n         \n        <literal classes=\"kbd\">\n            A\n"),
    (":kbd:`Caps Lock`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Caps Lock\n"),
    (":kbd:`Ctrl+Page Up`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Ctrl\n        +\n        <literal classes=\"kbd\">\n            Page Up\n"),
    (":kbd:`Page Up+Ctrl`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Page Up+Ctrl\n"),
    (":kbd:`-`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            -\n"),
    (":kbd:`Ctrl+-`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Ctrl\n        +\n        <literal classes=\"kbd\">\n            -\n"),
    (":kbd:`Ctrl++`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Ctrl\n        +\n        <literal classes=\"kbd\">\n            +\n"),
    (":kbd:`a  b`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            a\n          \n        <literal classes=\"kbd\">\n            b\n"),
    (":kbd:`M-x  M-y`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            M\n        -\n        <literal classes=\"kbd\">\n            x\n          \n        <literal classes=\"kbd\">\n            M\n        -\n        <literal classes=\"kbd\">\n            y\n"),
    (":kbd:`back  space`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            back  space\n"),
    (":kbd:`Num Lock+x`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Num Lock+x\n"),
    (":kbd:`Ctrl\n+ Q`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            Ctrl\n            +\n         \n        <literal classes=\"kbd\">\n            Q\n"),
    (":kbd:`a\\+b`\n",
     "    <paragraph>\n        <literal classes=\"kbd\">\n            a\n        +\n        <literal classes=\"kbd\">\n            b\n"),
];

#[rustfmt::skip]
const GUILABEL: &[(&str, &str)] = &[
    (":guilabel:`&Cancel`\n",
     "    <paragraph>\n        <inline classes=\"guilabel\" rawtext=\":guilabel:`&Cancel`\">\n            <inline classes=\"accelerator\">\n                C\n            ancel\n"),
    (":guilabel:`O&K`\n",
     "    <paragraph>\n        <inline classes=\"guilabel\" rawtext=\":guilabel:`O&K`\">\n            O\n            <inline classes=\"accelerator\">\n                K\n"),
    (":guilabel:`Save && Exit`\n",
     "    <paragraph>\n        <inline classes=\"guilabel\" rawtext=\":guilabel:`Save && Exit`\">\n            Save && Exit\n"),
    (":guilabel:`A & B`\n",
     "    <paragraph>\n        <inline classes=\"guilabel\" rawtext=\":guilabel:`A & B`\">\n            A & B\n"),
    (":guilabel:`\\&Esc`\n",
     "    <paragraph>\n        <inline classes=\"guilabel\" rawtext=\":guilabel:`\\&Esc`\">\n            <inline classes=\"accelerator\">\n                E\n            sc\n"),
    (":guilabel:`&&&Foo`\n",
     "    <paragraph>\n        <inline classes=\"guilabel\" rawtext=\":guilabel:`&&&Foo`\">\n            &&&Foo\n"),
    (":guilabel:`Plain`\n",
     "    <paragraph>\n        <inline classes=\"guilabel\" rawtext=\":guilabel:`Plain`\">\n            Plain\n"),
    (":menuselection:`Start --> Programs`\n",
     "    <paragraph>\n        <inline classes=\"menuselection\" rawtext=\":menuselection:`Start --> Programs`\">\n            Start ‣ Programs\n"),
    (":menuselection:`&File --> &Open`\n",
     "    <paragraph>\n        <inline classes=\"menuselection\" rawtext=\":menuselection:`&File --> &Open`\">\n            <inline classes=\"accelerator\">\n                F\n            ile ‣ \n            <inline classes=\"accelerator\">\n                O\n            pen\n"),
    (":menuselection:`A-->B`\n",
     "    <paragraph>\n        <inline classes=\"menuselection\" rawtext=\":menuselection:`A-->B`\">\n            A‣B\n"),
    (":menuselection:`A --> B --> C`\n",
     "    <paragraph>\n        <inline classes=\"menuselection\" rawtext=\":menuselection:`A --> B --> C`\">\n            A ‣ B ‣ C\n"),
    (":menuselection:`A -> B`\n",
     "    <paragraph>\n        <inline classes=\"menuselection\" rawtext=\":menuselection:`A -> B`\">\n            A -> B\n"),
];

#[rustfmt::skip]
const SAMP: &[(&str, &str)] = &[
    (":samp:`print({x})`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            print(\n            <emphasis>\n                x\n            )\n"),
    (":samp:`a{b}c{d}e`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            a\n            <emphasis>\n                b\n            c\n            <emphasis>\n                d\n            e\n"),
    (":samp:`{}`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            {}\n"),
    (":samp:`a\\\\{b\\\\}`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            a{b}\n"),
    (":samp:`a\\{b\\}`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            a\n            <emphasis>\n                b\n"),
    (":samp:`{a{b}c}`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            <emphasis>\n                a{b\n            c}\n"),
    (":samp:`a}b{c`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            a}b{c\n"),
    (":samp:`{unclosed`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            {unclosed\n"),
    (":samp:`x\\\\\\\\y`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            x\\y\n"),
    (":samp:`plain`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            plain\n"),
    (":file:`/etc/{name}.conf`\n",
     "    <paragraph>\n        <literal classes=\"file\" role=\"file\">\n            /etc/\n            <emphasis>\n                name\n            .conf\n"),
    (":file:`C:\\\\Users\\\\{user}`\n",
     "    <paragraph>\n        <literal classes=\"file\" role=\"file\">\n            C:\\Users{user}\n"),
    (":SAMP:`Upper{X}`\n",
     "    <paragraph>\n        <literal classes=\"samp\" role=\"samp\">\n            Upper\n            <emphasis>\n                X\n"),
];

#[rustfmt::skip]
const ABBR: &[(&str, &str)] = &[
    (":abbr:`LIFO (last-in, first-out)`\n",
     "    <paragraph>\n        <abbreviation explanation=\"last-in, first-out\">\n            LIFO\n"),
    (":abbr:`LIFO`\n",
     "    <paragraph>\n        <abbreviation>\n            LIFO\n"),
    (":abbr:`LIFO ()`\n",
     "    <paragraph>\n        <abbreviation explanation=\"\">\n            LIFO\n"),
    (":abbr:`(only explanation)`\n",
     "    <paragraph>\n        <abbreviation explanation=\"only explanation\">\n"),
    (":abbr:`A (b) (c)`\n",
     "    <paragraph>\n        <abbreviation explanation=\"b) (c\">\n            A\n"),
    (":abbr:`X (multi\nline)`\n",
     "    <paragraph>\n        <abbreviation explanation=\"multi\nline\">\n            X\n"),
    (":abbr:`Y(z)`\n",
     "    <paragraph>\n        <abbreviation explanation=\"z\">\n            Y\n"),
    (":abbr:`  `\n",
     "    <paragraph>\n        :abbr:`  `\n"),
];

#[rustfmt::skip]
const MANPAGE: &[(&str, &str)] = &[
    (":manpage:`ls(1)`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls(1)\" section=\"1\" xml:space=\"preserve\">\n            ls(1)\n"),
    (":manpage:`ls`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls\" section=\"\" xml:space=\"preserve\">\n            ls\n"),
    (":manpage:`ls.1`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls.1\" section=\"1\" xml:space=\"preserve\">\n            ls.1\n"),
    (":manpage:`the ls page <ls(1)>`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls(1)\" section=\"1\" xml:space=\"preserve\">\n            the ls page\n"),
    (":manpage:`!ls(1)`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"!ls\" path=\"!ls(1)\" section=\"1\" xml:space=\"preserve\">\n            ls(1)\n"),
    (":manpage:`git-commit(1posix)`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"git-commit\" path=\"git-commit(1posix)\" section=\"1posix\" xml:space=\"preserve\">\n            git-commit(1posix)\n"),
    (":manpage:`a  b(8)`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"a b\" path=\"a b(8)\" section=\"8\" xml:space=\"preserve\">\n            a  b(8)\n"),
];

#[rustfmt::skip]
const DOWNLOAD: &[(&str, &str)] = &[
    (":download:`f.txt`\n",
     "    <paragraph>\n        <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"f.txt\" reftype=\"download\" refwarn=\"0\">\n            <literal classes=\"xref download\">\n                f.txt\n"),
    (":download:`the file <f.txt>`\n",
     "    <paragraph>\n        <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"1\" reftarget=\"f.txt\" reftype=\"download\" refwarn=\"0\">\n            <literal classes=\"xref download\">\n                the file\n"),
    (":download:`sub/g.txt`\n",
     "    <paragraph>\n        <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"sub/g.txt\" reftype=\"download\" refwarn=\"0\">\n            <literal classes=\"xref download\">\n                sub/g.txt\n"),
    (":download:`/f.txt`\n",
     "    <paragraph>\n        <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"/f.txt\" reftype=\"download\" refwarn=\"0\">\n            <literal classes=\"xref download\">\n                /f.txt\n"),
    (":download:`missing.txt`\n",
     "    <paragraph>\n        <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"missing.txt\" reftype=\"download\" refwarn=\"0\">\n            <literal classes=\"xref download\">\n                missing.txt\n"),
    (":download:`!f.txt`\n",
     "    <paragraph>\n        <literal classes=\"xref download\">\n            f.txt\n"),
    (":download:`f  .txt`\n",
     "    <paragraph>\n        <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"f .txt\" reftype=\"download\" refwarn=\"0\">\n            <literal classes=\"xref download\">\n                f  .txt\n"),
];

#[rustfmt::skip]
const INDEX: &[(&str, &str)] = &[
    ("Para :index:`term` here.\n",
     "    <paragraph>\n        Para \n        <index entries=\"('single',\\ 'term',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        term\n         here.\n"),
    (":index:`!main term`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ 'main\\ term',\\ 'index-0',\\ 'main',\\ None)\">\n        <target ids=\"index-0\">\n        main term\n"),
    (":index:`see <single: a; b>`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ 'a;\\ b',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        see\n"),
    (":index:`pair <pair: x; y>`\n",
     "    <paragraph>\n        <index entries=\"('pair',\\ 'x;\\ y',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        pair\n"),
    (":index:`bad <bogus: x>`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ 'bogus:\\ x',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        bad\n"),
    (":index:`text <x>`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ 'x',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        text\n"),
    (":I:`upper`\n",
     "    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :I:`upper`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Interpreted text role \"I\" not implemented.\n"),
    (":i:`alias`\n",
     "    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :i:`alias`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Interpreted text role \"i\" not implemented.\n"),
    (":index:`t <a, !b, , c>`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ 'a',\\ 'index-0',\\ '',\\ None) ('single',\\ 'b',\\ 'index-0',\\ 'main',\\ None) ('single',\\ 'c',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        t\n"),
    (":index:`!x`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ 'x',\\ 'index-0',\\ 'main',\\ None)\">\n        <target ids=\"index-0\">\n        x\n"),
    (":index:`! spaced`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ '\\ spaced',\\ 'index-0',\\ 'main',\\ None)\">\n        <target ids=\"index-0\">\n         spaced\n"),
    (":index:`see also <seealso: p; q>`\n",
     "    <paragraph>\n        <index entries=\"('seealso',\\ 'p;\\ q',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        see also\n"),
    (":index:`d <double: m; n>`\n",
     "    <paragraph>\n        <index entries=\"('pair',\\ 'm;\\ n',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        d\n"),
    (":index:`! <!z>`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ 'z',\\ 'index-0',\\ 'main',\\ None)\">\n        <target ids=\"index-0\">\n        !\n"),
];

#[rustfmt::skip]
const PEP_RFC: &[(&str, &str)] = &[
    (":pep-reference:`8` and :PEP:`0` and :pep:`008` end.\n",
     "    <paragraph>\n        <reference refuri=\"https://peps.python.org/pep-0008\">\n            PEP 8\n         and \n        <index entries=\"('single',\\ 'Python\\ Enhancement\\ Proposals;\\ PEP\\ 0',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        <reference classes=\"pep\" internal=\"0\" refuri=\"https://peps.python.org/pep-0000/\">\n            <strong>\n                PEP 0\n         and \n        <index entries=\"('single',\\ 'Python\\ Enhancement\\ Proposals;\\ PEP\\ 008',\\ 'index-1',\\ '',\\ None)\">\n        <target ids=\"index-1\">\n        <reference classes=\"pep\" internal=\"0\" refuri=\"https://peps.python.org/pep-0008/\">\n            <strong>\n                PEP 008\n         end.\n"),
    (":rfc-reference:`2822` and :RFC:`0002822` and :RFC:`1` end.\n",
     "    <paragraph>\n        <reference refuri=\"https://datatracker.ietf.org/doc/html/rfc2822.html\">\n            RFC 2822\n         and \n        <index entries=\"('single',\\ 'RFC;\\ RFC\\ 0002822',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        <reference classes=\"rfc\" internal=\"0\" refuri=\"https://datatracker.ietf.org/doc/html/rfc2822.html\">\n            <strong>\n                RFC 0002822\n         and \n        <index entries=\"('single',\\ 'RFC;\\ RFC\\ 1',\\ 'index-1',\\ '',\\ None)\">\n        <target ids=\"index-1\">\n        <reference classes=\"rfc\" internal=\"0\" refuri=\"https://datatracker.ietf.org/doc/html/rfc1.html\">\n            <strong>\n                RFC 1\n         end.\n"),
    (":pep:`abc`\n",
     "    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :pep:`abc`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            invalid PEP number abc\n"),
    (":rfc:`abc`\n",
     "    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :rfc:`abc`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            invalid RFC number abc\n"),
    (":pep:`10000`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ 'Python\\ Enhancement\\ Proposals;\\ PEP\\ 10000',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        <reference classes=\"pep\" internal=\"0\" refuri=\"https://peps.python.org/pep-10000/\">\n            <strong>\n                PEP 10000\n"),
    (":pep:`8#anchor`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ 'Python\\ Enhancement\\ Proposals;\\ PEP\\ 8#anchor',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        <reference classes=\"pep\" internal=\"0\" refuri=\"https://peps.python.org/pep-0008/#anchor\">\n            <strong>\n                PEP 8#anchor\n"),
    (":pep-reference:`10000`\n",
     "    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :pep-reference:`10000`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            PEP number must be a number from 0 to 9999; \"10000\" is invalid.\n"),
    (":rfc:`1#section-` :rfc:`1#Section-2` :pep:`x < 8 >` :rfc:`2 #page-3`\n",
     "    <paragraph>\n        <index entries=\"('single',\\ 'RFC;\\ RFC\\ 1\\ Section',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        <reference classes=\"rfc\" internal=\"0\" refuri=\"https://datatracker.ietf.org/doc/html/rfc1.html#section-\">\n            <strong>\n                RFC 1 Section\n         \n        <index entries=\"('single',\\ 'RFC;\\ RFC\\ 1#Section-2',\\ 'index-1',\\ '',\\ None)\">\n        <target ids=\"index-1\">\n        <reference classes=\"rfc\" internal=\"0\" refuri=\"https://datatracker.ietf.org/doc/html/rfc1.html#Section-2\">\n            <strong>\n                RFC 1#Section-2\n         \n        <index entries=\"('single',\\ 'Python\\ Enhancement\\ Proposals;\\ PEP\\ \\ 8\\ ',\\ 'index-2',\\ '',\\ None)\">\n        <target ids=\"index-2\">\n        <reference classes=\"pep\" internal=\"0\" refuri=\"https://peps.python.org/pep-0008/\">\n            <strong>\n                x\n         \n        <index entries=\"('single',\\ 'RFC;\\ RFC\\ 2\\ \\ Page\\ 3',\\ 'index-3',\\ '',\\ None)\">\n        <target ids=\"index-3\">\n        <reference classes=\"rfc\" internal=\"0\" refuri=\"https://datatracker.ietf.org/doc/html/rfc2.html#page-3\">\n            <strong>\n                RFC 2  Page 3\n"),
];

#[rustfmt::skip]
const CODE: &[(&str, &str)] = &[
    (":code:`x = 1`\n",
     "    <paragraph>\n        <literal classes=\"code\" language=\"\">\n            x = 1\n"),
    (":code:`a\\*b`\n",
     "    <paragraph>\n        <literal classes=\"code\" language=\"\">\n            a*b\n"),
    (":code:`a\\\\b`\n",
     "    <paragraph>\n        <literal classes=\"code\" language=\"\">\n            a\\b\n"),
];

#[rustfmt::skip]
const DISABLED: &[(&str, &str)] = &[
    (":ref:`!label` :doc:`!page` :term:`!x` :any:`!y` :keyword:`!k` :numref:`!n`\n",
     "    <paragraph>\n        <inline classes=\"xref std std-ref\">\n            label\n         \n        <inline classes=\"xref std std-doc\">\n            page\n         \n        <inline classes=\"xref std std-term\">\n            x\n         \n        <literal classes=\"xref any\">\n            y\n         \n        <literal classes=\"xref std std-keyword\">\n            k\n         \n        <literal classes=\"xref std std-numref\">\n            n\n"),
    (":option:`!-x` :envvar:`!HOME` :confval:`!c` :token:`!t`\n",
     "    <paragraph>\n        <literal classes=\"xref std std-option\">\n            -x\n         \n        <literal classes=\"xref std std-envvar\">\n            HOME\n         \n        <literal classes=\"xref std std-confval\">\n            c\n         \n        <literal classes=\"xref std std-token\">\n            t\n"),
    (":py:func:`!f` :py:meth:`!m()` :func:`!g` :py:class:`!~a.B` :eq:`!e`\n",
     "    <paragraph>\n        <literal classes=\"xref py py-func\">\n            f()\n         \n        <literal classes=\"xref py py-meth\">\n            m()\n         \n        <literal classes=\"xref py py-func\">\n            g()\n         \n        <literal classes=\"xref py py-class\">\n            ~a.B\n         \n        <literal classes=\"xref eq\" refdomain=\"math\">\n            e\n"),
];

#[rustfmt::skip]
const EXPLICIT_TITLES: &[(&str, &str)] = &[
    (":doc:`a <b> <c>`\n",
     "    <paragraph>\n        <pending_xref refdoc=\"index\" refdomain=\"std\" refexplicit=\"1\" reftarget=\"b> <c\" reftype=\"doc\" refwarn=\"1\">\n            <inline classes=\"xref std std-doc\">\n                a\n"),
    (":doc:`x \\<y>`\n",
     "    <paragraph>\n        <pending_xref refdoc=\"index\" refdomain=\"std\" refexplicit=\"0\" reftarget=\"x <y>\" reftype=\"doc\" refwarn=\"1\">\n            <inline classes=\"xref std std-doc\">\n                x <y>\n"),
    (":doc:`p\\ <q>`\n",
     "    <paragraph>\n        <pending_xref refdoc=\"index\" refdomain=\"std\" refexplicit=\"1\" reftarget=\"q\" reftype=\"doc\" refwarn=\"1\">\n            <inline classes=\"xref std std-doc\">\n                p\n"),
    (":manpage:`t <u(1)> <v(2)>`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"u(1)> <v\" path=\"u(1)> <v(2)\" section=\"2\" xml:space=\"preserve\">\n            t\n"),
    (":download:`n <m <o>>`\n",
     "    <paragraph>\n        <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"1\" reftarget=\"m <o>\" reftype=\"download\" refwarn=\"0\">\n            <literal classes=\"xref download\">\n                n\n"),
];

#[rustfmt::skip]
const OTHER_DOMAINS: &[(&str, &str)] = &[
    (":c:func:`f` :c:member:`s.m`\n",
     "    <paragraph>\n        <pending_xref refdoc=\"index\" refdomain=\"c\" refexplicit=\"0\" reftarget=\"f\" reftype=\"func\" refwarn=\"0\">\n            <literal classes=\"xref c c-func\">\n                f()\n         \n        <pending_xref refdoc=\"index\" refdomain=\"c\" refexplicit=\"0\" reftarget=\"s.m\" reftype=\"member\" refwarn=\"0\">\n            <literal classes=\"xref c c-member\">\n                s.m\n"),
    (":cpp:class:`C` :cpp:func:`f` :cpp:any:`x`\n",
     "    <paragraph>\n        <pending_xref refdoc=\"index\" refdomain=\"cpp\" refexplicit=\"0\" reftarget=\"C\" reftype=\"class\" refwarn=\"0\">\n            <literal classes=\"xref cpp cpp-class\">\n                C\n         \n        <pending_xref refdoc=\"index\" refdomain=\"cpp\" refexplicit=\"0\" reftarget=\"f\" reftype=\"func\" refwarn=\"0\">\n            <literal classes=\"xref cpp cpp-func\">\n                f()\n         \n        <pending_xref refdoc=\"index\" refdomain=\"cpp\" refexplicit=\"0\" reftarget=\"x\" reftype=\"any\" refwarn=\"0\">\n            <literal classes=\"xref cpp cpp-any\">\n                x\n"),
    (":js:func:`f` :js:mod:`m` :js:class:`K`\n",
     "    <paragraph>\n        <pending_xref js:module=\"True\" js:object=\"True\" refdoc=\"index\" refdomain=\"js\" refexplicit=\"0\" reftarget=\"f\" reftype=\"func\" refwarn=\"0\">\n            <literal classes=\"xref js js-func\">\n                f()\n         \n        <pending_xref js:module=\"True\" js:object=\"True\" refdoc=\"index\" refdomain=\"js\" refexplicit=\"0\" reftarget=\"m\" reftype=\"mod\" refwarn=\"0\">\n            <literal classes=\"xref js js-mod\">\n                m\n         \n        <pending_xref js:module=\"True\" js:object=\"True\" refdoc=\"index\" refdomain=\"js\" refexplicit=\"0\" reftarget=\"K\" reftype=\"class\" refwarn=\"0\">\n            <literal classes=\"xref js js-class\">\n                K\n"),
    (":js:meth:`o.m` :js:func:`~a.b` :js:func:`.c`\n",
     "    <paragraph>\n        <pending_xref js:module=\"True\" js:object=\"True\" refdoc=\"index\" refdomain=\"js\" refexplicit=\"0\" reftarget=\"o.m\" reftype=\"meth\" refwarn=\"0\">\n            <literal classes=\"xref js js-meth\">\n                o.m()\n         \n        <pending_xref js:module=\"True\" js:object=\"True\" refdoc=\"index\" refdomain=\"js\" refexplicit=\"0\" reftarget=\"a.b\" reftype=\"func\" refwarn=\"0\">\n            <literal classes=\"xref js js-func\">\n                b()\n         \n        <pending_xref js:module=\"True\" js:object=\"True\" refdoc=\"index\" refdomain=\"js\" refexplicit=\"0\" refspecific=\"1\" reftarget=\"c\" reftype=\"func\" refwarn=\"0\">\n            <literal classes=\"xref js js-func\">\n                c()\n"),
    (":rst:dir:`code-block` :rst:role:`kbd`\n",
     "    <paragraph>\n        <pending_xref refdoc=\"index\" refdomain=\"rst\" refexplicit=\"0\" reftarget=\"code-block\" reftype=\"dir\" refwarn=\"0\">\n            <literal classes=\"xref rst rst-dir\">\n                code-block\n         \n        <pending_xref refdoc=\"index\" refdomain=\"rst\" refexplicit=\"0\" reftarget=\"kbd\" reftype=\"role\" refwarn=\"0\">\n            <literal classes=\"xref rst rst-role\">\n                kbd\n"),
    (":math:numref:`e`\n",
     "    <paragraph>\n        <pending_xref refdoc=\"index\" refdomain=\"math\" refexplicit=\"0\" reftarget=\"e\" reftype=\"numref\" refwarn=\"0\">\n            <literal classes=\"xref math math-numref\">\n                e\n"),
];

#[rustfmt::skip]
const ROLE_DIRECTIVE: &[(&str, &str)] = &[
    (".. role:: python(code)\n   :language: python\n\n.. role:: plain(code)\n\n:python:`print(1)`\n\n:plain:`y`\n",
     "    <paragraph>\n        <literal classes=\"code highlight python\" language=\"python\">\n            print(1)\n    <paragraph>\n        <literal classes=\"code plain\" language=\"\">\n            y\n"),
    (".. role:: k(kbd)\n   :class: big\n\n.. role:: s(samp)\n\n.. role:: red\n\n.. role:: py3(code)\n   :language: python3\n   :class: x\n\n.. role:: g(guilabel)\n\n.. role:: m(menuselection)\n\n.. role:: d(download)\n\n.. role:: cmd(command)\n   :class: extra\n\n.. role:: ab(abbr)\n\n.. role:: mp(manpage)\n\n.. role:: idx(index)\n\n.. role:: p8(pep)\n\n:k:`Ctrl+C` :s:`a{b}` :red:`alert` :py3:`x=1` :g:`&OK` :m:`A --> B`\n\n:d:`f.txt` :cmd:`ls` :ab:`A (b)` :mp:`ls(1)` :idx:`entry` :p8:`8`\n",
     "    <paragraph>\n        <literal classes=\"kbd big\">\n            Ctrl\n        +\n        <literal classes=\"kbd big\">\n            C\n         \n        <literal classes=\"s\" role=\"s\">\n            a\n            <emphasis>\n                b\n         \n        <inline classes=\"red\">\n            alert\n         \n        <literal classes=\"code highlight x python3\" language=\"python3\">\n            x=1\n         \n        <inline classes=\"g\" rawtext=\":g:`&OK`\">\n            <inline classes=\"accelerator\">\n                O\n            K\n         \n        <inline classes=\"m\" rawtext=\":m:`A --> B`\">\n            A ‣ B\n    <paragraph>\n        <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"f.txt\" reftype=\"d\" refwarn=\"0\">\n            <literal classes=\"xref d\">\n                f.txt\n         \n        <literal_strong classes=\"extra\">\n            ls\n         \n        <abbreviation classes=\"ab\" explanation=\"b\">\n            A\n         \n        <manpage classes=\"mp\" page=\"ls\" path=\"ls(1)\" section=\"1\" xml:space=\"preserve\">\n            ls(1)\n         \n        <index entries=\"('single',\\ 'entry',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        entry\n         \n        <index entries=\"('single',\\ 'Python\\ Enhancement\\ Proposals;\\ PEP\\ 8',\\ 'index-1',\\ '',\\ None)\">\n        <target ids=\"index-1\">\n        <reference classes=\"pep\" internal=\"0\" refuri=\"https://peps.python.org/pep-0008/\">\n            <strong>\n                PEP 8\n"),
    (".. role:: f(py:func)\n\n.. role:: r(ref)\n   :class: special\n\n.. role:: a(any)\n\n.. role:: x(bogus)\n\n.. role:: em(emphasis)\n\n:f:`g` :r:`lbl` :a:`y` :x:`w` :em:`v`\n",
     "    <system_message level=\"3\" line=\"3\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Error in \"role\" directive:\n            no content permitted.\n        <literal_block xml:space=\"preserve\">\n            .. role:: r(ref)\n               :class: special\n    <system_message level=\"3\" line=\"8\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"bogus\".\n        <literal_block xml:space=\"preserve\">\n            .. role:: x(bogus)\n    <paragraph>\n        <pending_xref py:class=\"True\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"0\" reftarget=\"g\" reftype=\"func\" refwarn=\"0\">\n            <literal classes=\"xref py py-func\">\n                g()\n         \n        <problematic ids=\"id2\" refid=\"id1\">\n            :r:`lbl`\n         \n        <pending_xref refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"y\" reftype=\"a\" refwarn=\"1\">\n            <literal classes=\"xref a\">\n                y\n         \n        <problematic ids=\"id4\" refid=\"id3\">\n            :x:`w`\n         \n        <emphasis classes=\"em\">\n            v\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"12\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"r\".\n    <system_message backrefs=\"id4\" ids=\"id3\" level=\"3\" line=\"12\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"x\".\n"),
    (".. role::\n   red\n\n.. role:: a b\n\n.. role:: x(code)\n   :bogus: 1\n\n.. role:: y(code)\n   text\n\n.. role:: 1\n\n.. role:: raw-html(raw)\n   :format: html\n\n:raw-html:`<b>x</b>` :raw:`y`\n\n.. role:: K(Kbd)\n\n:K:`a+b` :k:`c`\n",
     "    <system_message level=\"3\" line=\"4\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            \"role\" directive arguments not valid role names: \"a b\".\n        <literal_block xml:space=\"preserve\">\n            .. role:: a b\n    <system_message level=\"3\" line=\"6\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Error in \"role\" directive:\n            unknown option: \"bogus\".\n        <literal_block xml:space=\"preserve\">\n            .. role:: x(code)\n               :bogus: 1\n    <system_message level=\"3\" line=\"9\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Error in \"role\" directive:\n            no content permitted.\n        <literal_block xml:space=\"preserve\">\n            .. role:: y(code)\n               text\n    <system_message level=\"3\" line=\"12\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Invalid argument for \"role\" directive:\n            cannot make \"1\" into a class name.\n        <literal_block xml:space=\"preserve\">\n            .. role:: 1\n    <paragraph>\n        <raw classes=\"raw-html\" format=\"html\" xml:space=\"preserve\">\n            <b>x</b>\n         \n        <problematic ids=\"id2\" refid=\"id1\">\n            :raw:`y`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"17\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            No format (Writer name) is associated with this role: \"raw\".\n            The \"raw\" role cannot be used directly.\n            Instead, use the \"role\" directive to create a new role with an associated format.\n    <paragraph>\n        <literal classes=\"kbd k\">\n            a\n        +\n        <literal classes=\"kbd k\">\n            b\n         \n        <literal classes=\"kbd k\">\n            c\n"),
    (".. role:: e(emphasis)\n.. role:: e2(e)\n   :class: deeper\n\n:e2:`x` :E2:`y`\n",
     "    <paragraph>\n        <emphasis classes=\"deeper\">\n            x\n         \n        <emphasis classes=\"deeper\">\n            y\n"),
    (".. role::\n\n   red\n\n:red:`x`\n",
     "    <system_message level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            \"role\" directive requires arguments on the first line.\n        <literal_block xml:space=\"preserve\">\n            .. role::\n            \n               red\n    <paragraph>\n        <problematic ids=\"id2\" refid=\"id1\">\n            :red:`x`\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"5\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"red\".\n"),
    ("T\n=\n\n.. role::\n\n   red\n\n:red:`x`\n",
     "    <section ids=\"t\" names=\"t\">\n        <title>\n            T\n        <system_message level=\"3\" line=\"4\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                \"role\" directive requires arguments on the first line.\n            <literal_block xml:space=\"preserve\">\n                .. role::\n                \n                   red\n        <paragraph>\n            <problematic ids=\"id2\" refid=\"id1\">\n                :red:`x`\n        <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"8\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"red\".\n"),
    ("T\n=\n\n.. role::\n\n\n\n\n\n   red\n\n:red:`x`\n",
     "    <section ids=\"t\" names=\"t\">\n        <title>\n            T\n        <system_message level=\"3\" line=\"4\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                \"role\" directive requires arguments on the first line.\n            <literal_block xml:space=\"preserve\">\n                .. role::\n                \n                \n                \n                \n                \n                   red\n        <paragraph>\n            <problematic ids=\"id2\" refid=\"id1\">\n                :red:`x`\n        <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"12\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"red\".\n"),
    (".. role::\n",
     "    <system_message level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            \"role\" directive requires arguments on the first line.\n        <literal_block xml:space=\"preserve\">\n            .. role::\n"),
    (".. note::\n\n   .. role::\n\n\n      red\n\n   :red:`x`\n",
     "    <note>\n        <system_message level=\"3\" line=\"3\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                \"role\" directive requires arguments on the first line.\n            <literal_block xml:space=\"preserve\">\n                .. role::\n                \n                \n                   red\n        <paragraph>\n            <problematic ids=\"id2\" refid=\"id1\">\n                :red:`x`\n        <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"8\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n            <paragraph>\n                Unknown interpreted text role \"red\".\n"),
];

#[rustfmt::skip]
const DEFAULT_ROLE_DIRECTIVE: &[(&str, &str)] = &[
    ("A `x` here.\n\n.. default-role:: math\n\nB `a^2` here.\n\n.. default-role:: kbd\n\nC `Ctrl+X` here.\n\n.. default-role::\n\nD `z` here.\n\n.. default-role:: bogus\n\nE `w` here.\n\n.. default-role:: code\n\nF `q` here.\n",
     "    <paragraph>\n        A \n        <title_reference>\n            x\n         here.\n    <paragraph>\n        B \n        <math>\n            a^2\n         here.\n    <paragraph>\n        C \n        <literal classes=\"kbd\">\n            Ctrl\n        +\n        <literal classes=\"kbd\">\n            X\n         here.\n    <paragraph>\n        D \n        <title_reference>\n            z\n         here.\n    <system_message level=\"3\" line=\"15\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            Unknown interpreted text role \"bogus\".\n        <literal_block xml:space=\"preserve\">\n            .. default-role:: bogus\n    <paragraph>\n        E \n        <title_reference>\n            w\n         here.\n    <paragraph>\n        F \n        <literal classes=\"code\" language=\"\">\n            q\n         here.\n"),
    (".. default-role:: guilabel\n\nG `&x` here.\n\n.. default-role:: py:func\n\nH `f` here.\n\n.. default-role:: Func\n\nI `g` here.\n",
     "    <paragraph>\n        G \n        <inline classes=\"guilabel\" rawtext=\"`&x`\">\n            <inline classes=\"accelerator\">\n                x\n         here.\n    <paragraph>\n        H \n        <pending_xref py:class=\"True\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"0\" reftarget=\"f\" reftype=\"func\" refwarn=\"0\">\n            <literal classes=\"xref py py-func\">\n                f()\n         here.\n    <paragraph>\n        I \n        <pending_xref py:class=\"True\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"0\" reftarget=\"g\" reftype=\"func\" refwarn=\"0\">\n            <literal classes=\"xref py py-func\">\n                g()\n         here.\n"),
];

#[rustfmt::skip]
const MANPAGES_URL: &[(&str, &str, &str)] = &[
    ("https://man.example/{page}.{section}", ":manpage:`ls(1)`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls(1)\" section=\"1\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://man.example/ls.1\">\n                ls(1)\n"),
    ("https://man.example/{page}.{section}", ":manpage:`ls`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls\" section=\"\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://man.example/ls.\">\n                ls\n"),
    ("https://man.example/{page}.{section}", ":manpage:`the ls page <ls(1)>`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls(1)\" section=\"1\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://man.example/ls.1\">\n                the ls page\n"),
    ("https://man.example/{page}.{section}", ":manpage:`!ls(1)`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"!ls\" path=\"!ls(1)\" section=\"1\" xml:space=\"preserve\">\n            ls(1)\n"),
    ("https://manpages.debian.org/{path}", ":manpage:`ls(1)`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls(1)\" section=\"1\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://manpages.debian.org/ls(1)\">\n                ls(1)\n"),
    ("https://manpages.debian.org/{path}", ":manpage:`ls`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls\" section=\"\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://manpages.debian.org/ls\">\n                ls\n"),
    ("https://manpages.debian.org/{path}", ":manpage:`the ls page <ls(1)>`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls(1)\" section=\"1\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://manpages.debian.org/ls(1)\">\n                the ls page\n"),
    ("https://manpages.debian.org/{path}", ":manpage:`!ls(1)`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"!ls\" path=\"!ls(1)\" section=\"1\" xml:space=\"preserve\">\n            ls(1)\n"),
    ("https://m/{page}/{section}/{path}", ":manpage:`ls(` :manpage:`ls()` :manpage:`ls.` :manpage:`a.b(3)` :manpage:`x(0)`\n",
     "    <paragraph>\n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls(\" section=\"True\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://m/ls/None/ls(\">\n                ls(\n         \n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls()\" section=\"True\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://m/ls/None/ls()\">\n                ls()\n         \n        <manpage classes=\"manpage\" page=\"ls\" path=\"ls.\" section=\"True\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://m/ls/None/ls.\">\n                ls.\n         \n        <manpage classes=\"manpage\" page=\"a.b\" path=\"a.b(3)\" section=\"3\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://m/a.b/3/a.b(3)\">\n                a.b(3)\n         \n        <manpage classes=\"manpage\" page=\"x(0)\" path=\"x(0)\" section=\"\" xml:space=\"preserve\">\n            <reference classes=\"manpage\" refuri=\"https://m/x(0)//x(0)\">\n                x(0)\n"),
];

#[rustfmt::skip]
const DEFAULT_ROLE_CONFIG: &[(&str, &str, &str)] = &[
    ("any", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <pending_xref refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"x\" reftype=\"any\" refwarn=\"1\">\n            <literal classes=\"xref any\">\n                x\n         and \n        <pending_xref refdoc=\"index\" refdomain=\"\" refexplicit=\"1\" reftarget=\"b\" reftype=\"any\" refwarn=\"1\">\n            <literal classes=\"xref any\">\n                a\n         and \n        <literal>\n            y\n        .\n"),
    ("obj", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <pending_xref py:class=\"True\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"0\" reftarget=\"x\" reftype=\"obj\" refwarn=\"0\">\n            <literal classes=\"xref py py-obj\">\n                x\n         and \n        <pending_xref py:class=\"True\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"1\" reftarget=\"b\" reftype=\"obj\" refwarn=\"0\">\n            <literal classes=\"xref py py-obj\">\n                a\n         and \n        <literal>\n            y\n        .\n"),
    ("py:obj", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <pending_xref py:class=\"True\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"0\" reftarget=\"x\" reftype=\"obj\" refwarn=\"0\">\n            <literal classes=\"xref py py-obj\">\n                x\n         and \n        <pending_xref py:class=\"True\" py:module=\"True\" refdoc=\"index\" refdomain=\"py\" refexplicit=\"1\" reftarget=\"b\" reftype=\"obj\" refwarn=\"0\">\n            <literal classes=\"xref py py-obj\">\n                a\n         and \n        <literal>\n            y\n        .\n"),
    ("guilabel", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <inline classes=\"guilabel\" rawtext=\"`x`\">\n            x\n         and \n        <inline classes=\"guilabel\" rawtext=\"`a <b>`\">\n            a <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("pep", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <problematic ids=\"id2\" refid=\"id1\">\n            `x`\n         and \n        <problematic ids=\"id4\" refid=\"id3\">\n            `a <b>`\n         and \n        <literal>\n            y\n        .\n    <system_message backrefs=\"id2\" ids=\"id1\" level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            invalid PEP number x\n    <system_message backrefs=\"id4\" ids=\"id3\" level=\"3\" line=\"1\" source=\"<SRCDIR>/index.rst\" type=\"ERROR\">\n        <paragraph>\n            invalid PEP number b\n"),
    ("kbd", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <literal classes=\"kbd\">\n            x\n         and \n        <literal classes=\"kbd\">\n            a\n         \n        <literal classes=\"kbd\">\n            <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("math", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <math>\n            x\n         and \n        <math>\n            a <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("code", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <literal classes=\"code\" language=\"\">\n            x\n         and \n        <literal classes=\"code\" language=\"\">\n            a <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("index", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <index entries=\"('single',\\ 'x',\\ 'index-0',\\ '',\\ None)\">\n        <target ids=\"index-0\">\n        x\n         and \n        <index entries=\"('single',\\ 'b',\\ 'index-1',\\ '',\\ None)\">\n        <target ids=\"index-1\">\n        a\n         and \n        <literal>\n            y\n        .\n"),
    ("eq", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <pending_xref refdoc=\"index\" refdomain=\"math\" refexplicit=\"0\" reftarget=\"x\" reftype=\"eq\" refwarn=\"1\">\n            <literal classes=\"xref eq\">\n                x\n         and \n        <pending_xref refdoc=\"index\" refdomain=\"math\" refexplicit=\"1\" reftarget=\"b\" reftype=\"eq\" refwarn=\"1\">\n            <literal classes=\"xref eq\">\n                a\n         and \n        <literal>\n            y\n        .\n"),
    ("nonexistent", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <title_reference>\n            x\n         and \n        <title_reference>\n            a <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("literal", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <literal>\n            x\n         and \n        <literal>\n            a <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("emphasis", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <emphasis>\n            x\n         and \n        <emphasis>\n            a <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("abbr", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <abbreviation>\n            x\n         and \n        <abbreviation>\n            a <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("samp", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <literal classes=\"samp\" role=\"samp\">\n            x\n         and \n        <literal classes=\"samp\" role=\"samp\">\n            a <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("manpage", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <manpage classes=\"manpage\" page=\"x\" path=\"x\" section=\"\" xml:space=\"preserve\">\n            x\n         and \n        <manpage classes=\"manpage\" page=\"b\" path=\"b\" section=\"\" xml:space=\"preserve\">\n            a\n         and \n        <literal>\n            y\n        .\n"),
    ("download", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"x\" reftype=\"download\" refwarn=\"0\">\n            <literal classes=\"xref download\">\n                x\n         and \n        <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"1\" reftarget=\"b\" reftype=\"download\" refwarn=\"0\">\n            <literal classes=\"xref download\">\n                a\n         and \n        <literal>\n            y\n        .\n"),
    ("term", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <pending_xref refdoc=\"index\" refdomain=\"std\" refexplicit=\"0\" reftarget=\"x\" reftype=\"term\" refwarn=\"1\">\n            <inline classes=\"xref std std-term\">\n                x\n         and \n        <pending_xref refdoc=\"index\" refdomain=\"std\" refexplicit=\"1\" reftarget=\"b\" reftype=\"term\" refwarn=\"1\">\n            <inline classes=\"xref std std-term\">\n                a\n         and \n        <literal>\n            y\n        .\n"),
    ("foo:bar", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <title_reference>\n            x\n         and \n        <title_reference>\n            a <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("menuselection", "Text `x` and `a <b>` and `y`:literal:.\n",
     "    <paragraph>\n        Text \n        <inline classes=\"menuselection\" rawtext=\"`x`\">\n            x\n         and \n        <inline classes=\"menuselection\" rawtext=\"`a <b>`\">\n            a <b>\n         and \n        <literal>\n            y\n        .\n"),
    ("any", "A `x` here.\n\n.. default-role::\n\nD `z` here.\n",
     "    <paragraph>\n        A \n        <pending_xref refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"x\" reftype=\"any\" refwarn=\"1\">\n            <literal classes=\"xref any\">\n                x\n         here.\n    <paragraph>\n        D \n        <title_reference>\n            z\n         here.\n"),
];

/// [RC §3.1]: the generic docroles are `CustomRole`s over `GenericRole`,
/// their node class Sphinx's (`literal_strong`, `emphasis`,
/// `literal_emphasis`, `literal`), classed with the registered name, the
/// text null-escaped, no title split and no `!`.
#[test]
fn generic_docroles_match_sphinx() {
    check(GENERIC, ReadConfig::default());
}

/// [RC §3.2]: `Keyboard`'s split, the multi-word keys, empty keys between
/// separators, and the newline the separator's look-arounds exclude.
#[test]
fn kbd_matches_sphinx() {
    check(KBD, ReadConfig::default());
}

/// [RC §3.3-3.4]: the accelerators of `amp_re`, `&&` only inside a split
/// span, `rawtext` an attribute, `-->` a `‣`.
#[test]
fn guilabel_and_menuselection_match_sphinx() {
    check(GUILABEL, ReadConfig::default());
}

/// [RC §3.5]: `EmphasizedLiteral`'s stack over the unescaped text.
#[test]
fn samp_and_file_match_sphinx() {
    check(SAMP, ReadConfig::default());
}

/// [RC §3.6]: the first `(` to a final `)`, an empty explanation, a
/// childless node.
#[test]
fn abbr_matches_sphinx() {
    check(ABBR, ReadConfig::default());
}

/// [RC §3.7]: the page, path and section of `_manpage_re`, a section of
/// `None` (`"True"`), and with `manpages_url` the `reference` to the URL
/// the page formats (`None` for a missing section), except for a disabled
/// reference.
#[test]
fn manpage_matches_sphinx() {
    check(MANPAGE, ReadConfig::default());
    for (url, rst, expected) in MANPAGES_URL {
        let config = ReadConfig {
            manpages_url: Some(url.to_string()),
            ..ReadConfig::default()
        };
        check(&[(rst, expected)], config);
    }
}

/// [RC §3.8], the role's half: a `download_reference` registered without
/// a domain, its target's whitespace collapsed, `!` the inner literal
/// alone.
#[test]
fn download_role_matches_sphinx() {
    check(DOWNLOAD, ReadConfig::default());
}

/// [RC §3.9]: P02 and P11's `:index:` lines.
#[test]
fn index_role_rows_match_sphinx() {
    check(INDEX, ReadConfig::default());
}

/// [RC §3.10]: the long names are docutils' roles; `:pep:` and `:rfc:`
/// take their serial and register their target before the number is
/// checked, report over a `problematic` of the raw text, parse the number
/// with Python's `int()`, keep an explicit target as written, and format
/// an RFC anchor with `partition('-')` (P02, P15).
#[test]
fn pep_and_rfc_match_sphinx() {
    check(PEP_RFC, ReadConfig::default());
}

/// [RC §3.11]: Sphinx's `code`, `language=""` and the escapes gone.
#[test]
fn sphinx_code_role_matches_sphinx() {
    check(CODE, ReadConfig::default());
}

/// [RC §3.16]: a disabled reference (`!`) is `XRefRole`'s inner node
/// alone, over the text after the `!`, with `fix_parens` as for an
/// implicit title and no `process_link` (`~` kept); `eq` still stamps its
/// domain.
#[test]
fn disabled_reference_has_no_xref() {
    check(DISABLED, ReadConfig::default());
}

/// [RC §3.17]: the explicit title split of every reference role,
/// `ReferenceRole.explicit_title_re` on the escaped text (P20).
#[test]
fn explicit_title_split_matches_sphinx() {
    check(EXPLICIT_TITLES, ReadConfig::default());
}

/// [RC §3.18]: `fix_parens` on `c:func`, `cpp:func`, `js:func` and
/// `js:meth`, `JSXRefRole`'s `js:module`/`js:object` (`None`), its `~` and
/// `.` handling; the `rst` and `math:numref` shapes (P13, without the
/// c/cpp expression roles, which are not modelled).
#[test]
fn fix_parens_and_js_attributes_match_sphinx() {
    check(OTHER_DOMAINS, ReadConfig::default());
}

/// [RC §3.15]: custom roles over Sphinx's roles, docutils' and the
/// domains', their classes, `role` and `reftype` from the new name;
/// options parsed against what the base admits; the directive's errors;
/// a custom role over a custom role (P03c, P14).
#[test]
fn role_directive_matches_sphinx() {
    check(ROLE_DIRECTIVE, ReadConfig::default());
}

/// [RC §4.2]: `default_role` looked up like any role name — a domain's
/// role through the default domain, called with its full name; a Sphinx
/// role named after the configured role — or, for a name no role answers,
/// `title_reference` and `default role X not found`, located at the
/// document (`location=docname`, [`LogLocation::Doc`]), after the
/// `unknown role name` of an unknown domain prefix. `.. default-role::`
/// switches the role, back to `title_reference` with no argument, and
/// reports a name no role answers.
#[test]
fn default_role_config_and_directive() {
    for (role, rst, expected) in DEFAULT_ROLE_CONFIG {
        let config = ReadConfig {
            default_role: Some(role.to_string()),
            ..ReadConfig::default()
        };
        check(&[(rst, expected)], config);
    }
    check(DEFAULT_ROLE_DIRECTIVE, ReadConfig::default());

    let unknown = |role: &str| ReadConfig {
        default_role: Some(role.to_string()),
        ..ReadConfig::default()
    };
    let out = parse_rst_full(
        "Text `x`.\n",
        &ParseOptions {
            read: unknown("nonexistent"),
            ..index_rst()
        },
    );
    assert_eq!(
        out.registry.log_warnings,
        vec![ParseLogWarning {
            message: "default role nonexistent not found".to_string(),
            location: LogLocation::Doc,
            seq: 0,
        }]
    );
    for (role, warnings) in [
        (
            "nonexistent",
            vec!["<SRCDIR>/index.rst: WARNING: default role nonexistent not found"],
        ),
        (
            "foo:bar",
            vec![
                "WARNING: unknown role name: foo:bar",
                "<SRCDIR>/index.rst: WARNING: default role foo:bar not found",
            ],
        ),
        ("any", vec![]),
    ] {
        assert_eq!(read("Text `x`.\n", unknown(role)).1, warnings, "{role}");
    }
}

/// Decision 13: the role inputs that crash Sphinx 9.1.0 — a trailing `&`
/// in `guilabel` and `menuselection` (`IndexError`), a removed `:index:`
/// entry type (`ValueError`), a Sphinx role called as the default role of
/// a document that has none (`SphinxError`, here through a leaked
/// registration) — keep the document's first crash; the rest of the parse
/// goes on.
#[test]
fn role_crashes_are_kept() {
    let index_error = Some((
        "IndexError".to_string(),
        "string index out of range".to_string(),
    ));
    assert_eq!(
        crash(":guilabel:`a&`\n", ReadConfig::default()),
        index_error
    );
    assert_eq!(
        crash(":menuselection:`File --> &`\n", ReadConfig::default()),
        index_error
    );
    assert_eq!(crash(":guilabel:`a&& b& c`\n", ReadConfig::default()), None);
    assert_eq!(
        crash(
            ":index:`x <module: foo>` :index:`y <builtin: len>`\n",
            ReadConfig::default()
        ),
        Some((
            "ValueError".to_string(),
            "'module' is no longer supported for index entries (from entry 'module: foo'). \
             Use 'pair: module; foo' instead."
                .to_string()
        ))
    );
    assert_eq!(
        crash(":index:`y <!builtin: len>`\n", ReadConfig::default()),
        Some((
            "ValueError".to_string(),
            "'builtin' is no longer supported for index entries (from entry 'builtin: len'). \
             Use 'pair: built-in function; len' instead."
                .to_string()
        ))
    );
    // The implicit form never reaches `process_index_entry`.
    assert_eq!(crash(":index:`module: foo`\n", ReadConfig::default()), None);
    // `manpages_url.format_map(info)` raises for a field the page has no
    // key for, and for a positional one.
    let url = |url: &str| ReadConfig {
        manpages_url: Some(url.to_string()),
        ..ReadConfig::default()
    };
    assert_eq!(
        crash(":manpage:`ls(1)`\n", url("https://m/{foo}")),
        Some(("KeyError".to_string(), "'foo'".to_string()))
    );
    assert_eq!(
        crash(":manpage:`ls(1)`\n", url("https://m/{}")),
        Some((
            "ValueError".to_string(),
            "Format string contains positional fields".to_string()
        ))
    );
    assert_eq!(crash(":manpage:`!ls(1)`\n", url("https://m/{foo}")), None);

    // A leaked default role is a registration the document's table holds
    // with no default role name of its own.
    let mut table = crate::rst::roles::RoleTable::sphinx();
    table.set_default(Some(crate::rst::roles::RoleFn::Kbd));
    let parse = ParseOptions {
        roles: Some(std::sync::Arc::new(table)),
        ..index_rst()
    };
    let out = parse_rst_full("`y` here.\n", &parse);
    assert_eq!(
        out.crash
            .map(|crash| (crash.exception, crash.category, crash.message)),
        Some((
            "sphinx.errors.SphinxError",
            Some("Sphinx error"),
            "cannot determine default role!".to_string()
        ))
    );
    // With a configured default role, the document has a name for it.
    let parse = ParseOptions {
        read: ReadConfig {
            default_role: Some("kbd".to_string()),
            ..ReadConfig::default()
        },
        ..parse
    };
    assert_eq!(parse_rst_full("`y` here.\n", &parse).crash, None);
}
