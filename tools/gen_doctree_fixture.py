#!/usr/bin/env python3
"""Generate tests/fixtures/doctree_differential.json from docutils.

Regenerate with:

    uv run --python 3.12 --with docutils==0.22.4 python tools/gen_doctree_fixture.py

The fixture records docutils 0.22.4 PARSE-LAYER pseudo-XML (no transforms:
no doctitle promotion, no target propagation, no transition hoisting, no
message filtering) for a corpus of RST snippets covering the M2 wave-1
construct set. tests/doctree_differential.rs asserts sphinx-ultra's
`rst::parse_rst(...).root.pformat()` matches byte-for-byte.

Settings pinned where plain docutils and Sphinx 9.1 diverge:
`auto_id_prefix='id'` (Sphinx overrides docutils' '%'), `report_level=1`
(keep INFO messages), `halt_level=5` (never halt). Later waves EXTEND the
corpus and SUPPORTED_KINDS; never remove or rename existing cases.
"""

import io
import json
import re
import sys
from pathlib import Path

import docutils
from docutils.frontend import get_default_settings
from docutils.parsers.rst import Parser
from docutils.utils import new_document

EXPECTED_DOCUTILS = "0.22.4"

# Wave-1 construct set. A snippet producing any node kind outside this set
# (inline markup sneaking in, a directive, a footnote...) is a generator
# ERROR: the corpus must stay inside what the Rust parser implements.
SUPPORTED_KINDS = {
    "#text",
    "document",
    "section",
    "title",
    "paragraph",
    "transition",
    "bullet_list",
    "enumerated_list",
    "list_item",
    "definition_list",
    "definition_list_item",
    "term",
    "classifier",
    "definition",
    "block_quote",
    "attribution",
    "literal_block",
    "doctest_block",
    "line_block",
    "line",
    "comment",
    "target",
    "system_message",
    # wave 2: inline basics
    "emphasis",
    "strong",
    "literal",
    "problematic",
    "reference",
    "title_reference",
    "footnote_reference",
    "citation_reference",
    "substitution_reference",
    "subscript",
    "superscript",
    "abbreviation",
    "acronym",
    "math",
}

# (family, name, rst) — names unique, families floor-checked below.
CASES = [
    # ----- paragraphs -----
    ("paragraphs", "empty_document", ""),
    ("paragraphs", "whitespace_only", "   \n\n  \n"),
    ("paragraphs", "single", "Just some text.\n"),
    ("paragraphs", "multiline", "line one\nline two\n"),
    ("paragraphs", "blank_separated", "para one\n\n\npara two\n"),
    ("paragraphs", "trailing_whitespace", "para with trailing   \nsecond line  \n"),
    ("paragraphs", "tabs_in_text", "col\tone\n\tindented tab start\n"),
    ("paragraphs", "crlf_input", "one\r\ntwo\r\n\r\nthree\r\n"),
    ("paragraphs", "adornment_absorbed", "line1\nline2\n----\nafter\n"),
    ("paragraphs", "adornment_absorbed_blank", "line1\nline2\n----\n\nafter\n"),
    ("paragraphs", "short_adornment_alone", "before\n\n---\n\nafter\n"),
    ("paragraphs", "punctuation_text", "x -- y; z: w, (v) [u].\n"),
    # ----- sections -----
    ("sections", "simple_nested", "Title\n=====\n\nPara under title.\n\nSub\n---\n\nPara under sub.\n"),
    ("sections", "three_levels", "A\n=\n\nB\n-\n\nC\n~\n\ndeep text\n\nD\n-\n\nback at two\n"),
    ("sections", "over_under", "=====\nOver\n=====\n\nbody here\n"),
    ("sections", "over_under_vs_under", "=====\nOver\n=====\n\nUnder\n=====\n\ndeep body\n"),
    ("sections", "over_under_centered", "==========\n  Title\n==========\n\nbody\n"),
    ("sections", "underline_exact_length", "AB\n==\n\nx\n"),
    ("sections", "underline_longer", "AB\n=====\n\nx\n"),
    ("sections", "unicode_title", "Überblick\n=========\n\ntext\n"),
    ("sections", "digraph_title", "straße\n======\n\ntext\n"),
    ("sections", "digit_title", "123\n=====\n\ntext\n"),
    ("sections", "numbered_title", "1. Intro\n========\n\ntext\n"),
    ("sections", "duplicate_titles", "Duplicate\n=========\n\nx\n\nDuplicate\n=========\n\ny\n"),
    ("sections", "triple_duplicate", "Dup\n===\n\nDup\n===\n\nDup\n===\n"),
    ("sections", "same_level_siblings", "Alpha\n-----\n\none\n\nB\n----------\n\ntwo\n"),
    ("sections", "title_no_blank_after", "para\n----\nafter\n"),
    ("sections", "whitespace_collapse_title", "My  Section    Title!\n=====================\n\nx\n"),
    # ----- transitions -----
    ("transition", "basic", "Para.\n\n----\n\nMore.\n"),
    ("transition", "doc_start", "----\n\npara\n"),
    ("transition", "doc_end", "para\n\n----\n"),
    ("transition", "adjacent", "para\n\n----\n\n----\n\nend\n"),
    ("transition", "after_section_title", "Head\n====\n\n----\n\npara\n"),
    ("transition", "other_chars", "a\n\n====\n\nb\n\n~~~~\n\nc\n\n****\n\nd\n\n::::\n\ne\n\n____\n\nf\n"),
    # ----- bullet lists -----
    ("lists_bullet", "simple", "- one\n- two\n- three\n"),
    ("lists_bullet", "loose", "- one\n\n- two\n"),
    ("lists_bullet", "nested", "- outer one\n\n  * inner a\n\n  * inner b\n\n- outer two\n"),
    ("lists_bullet", "multi_paragraph_item", "- first para of item\n\n  second para of item\n"),
    ("lists_bullet", "star_and_plus", "* star one\n* star two\n\n+ plus one\n+ plus two\n"),
    ("lists_bullet", "unicode_bullets", "\u2022 uni bullet\n\n\u2023 tri bullet\n\n\u2043 hyphen bullet\n"),
    ("lists_bullet", "marker_alone", "-\n  body from next line\n"),
    ("lists_bullet", "ends_no_blank", "- item\nplain\n"),
    ("lists_bullet", "cont_then_ends", "- item\n  cont\nplain\n"),
    ("lists_bullet", "deep_nesting", "- a\n\n  - b\n\n    - c\n\n      - d\n\n        - e\n"),
    ("lists_bullet", "different_bullet_adjacent", "- a\n* b\n"),
    ("lists_bullet", "item_with_quote", "- item\n\n      quoted deeper\n"),
    # ----- enumerated lists -----
    ("lists_enum", "arabic", "1. one\n2. two\n3. three\n"),
    ("lists_enum", "loweralpha", "a. x\nb. y\n"),
    ("lists_enum", "paren_arabic", "(1) x\n(2) y\n"),
    ("lists_enum", "upper_paren", "A) x\nB) y\n"),
    ("lists_enum", "auto", "#. x\n#. y\n"),
    ("lists_enum", "auto_continue", "1. one\n#. two\n"),
    ("lists_enum", "start_three", "3. three\n4. four\n"),
    ("lists_enum", "roman_start_two", "ii. two\niii. three\n"),
    ("lists_enum", "single_i", "i. single\n"),
    ("lists_enum", "single_v", "v. five\n"),
    ("lists_enum", "single_c", "c. see\n"),
    ("lists_enum", "single_A_sentence", "A. Einstein was smart.\n"),
    ("lists_enum", "roman_narrowing", "v. five\nvi. six\n"),
    ("lists_enum", "alpha_over_roman_succession", "h. x\ni. y\nj. z\n"),
    ("lists_enum", "not_a_list", "1. one\nnot an item\n"),
    ("lists_enum", "skip_aborts", "1. one\n3. three\n"),
    ("lists_enum", "type_switch_aborts", "1. one\na. alpha\n"),
    ("lists_enum", "broken_mid_list", "1. one\n2. two\n5. five\n"),
    ("lists_enum", "continuation_lines", "1. first\n   more of first\n2. second\n"),
    ("lists_enum", "paren_roman", "(i) x\n(ii) y\n"),
    # ----- definition lists -----
    ("deflist", "simple", "term\n    definition here\n"),
    ("deflist", "classifiers", "term2 : classifier one : classifier two\n    Definition2.\n"),
    ("deflist", "colon_no_space", "term:not a classifier\n    Definition.\n"),
    ("deflist", "merge_items", "term1\n    Def1.\n\nterm2\n    Def2.\n"),
    ("deflist", "multi_para_definition", "term\n    para one\n\n    para two\n"),
    ("deflist", "nested_list_in_def", "term\n    - a\n    - b\n"),
    ("deflist", "ends_no_blank", "term\n    def\nplain\n"),
    ("deflist", "blank_between_not_deflist", "term\n\n    definition\n"),
    ("deflist", "adjacent_items", "term\n    def\nterm2\n    def2\n"),
    # ----- block quotes -----
    ("quote", "simple", "Para.\n\n    No matter where you go, there you are.\n"),
    ("quote", "attribution", "Para.\n\n    No matter where you go, there you are.\n\n    -- Buckaroo Banzai\n"),
    ("quote", "attribution_triple_dash", "Para.\n\n    Quoted here.\n\n    --- Someone\n"),
    ("quote", "attribution_em_dash", "Para.\n\n    Quoted here.\n\n    \u2014 Author\n"),
    ("quote", "two_quotes_split", "Para.\n\n    First quote.\n\n    -- First Author\n\n    Second quote.\n\n    -- Second Author\n"),
    ("quote", "multiline_attribution", "Para.\n\n    Quote.\n\n    -- Author Name,\n       Book Title, 1999\n"),
    ("quote", "nested_quote", "Para.\n\n    outer quote\n\n        inner quote\n"),
    ("quote", "partial_dedent", "Para.\n\n    quoted\n  dedented-oddly\n"),
    ("quote", "enum_trap", "Para.\n\n    Q. quoted\n"),
    ("quote", "list_inside_quote", "Para.\n\n    - a\n    - b\n"),
    ("quote", "multi_paragraph", "Para.\n\n    first quoted para\n\n    second quoted para\n"),
    # ----- literal blocks -----
    ("literal", "expanded", "Paragraph introducing::\n\n    literal line one\n    literal line two\n"),
    ("literal", "minimized", "Paragraph ends with ::\n\n    literal here\n"),
    ("literal", "triple_colon", "text:::\n\n    x\n"),
    ("literal", "colon_space_colon", "text: ::\n\n    y\n"),
    ("literal", "only_colons", "::\n\n    literal\n"),
    ("literal", "quoted", "Next is a quoted literal::\n\n> quoted line one\n> quoted line two\n"),
    ("literal", "quoted_inconsistent", "intro::\n\n> line one\n$ different\n"),
    ("literal", "missing", "Intro::\n\nNot indented.\n"),
    ("literal", "ends_no_blank", "para::\n\n    lit\nback\n"),
    ("literal", "internal_blank_lines", "code::\n\n    line one\n\n    line two\n"),
    ("literal", "deeper_relative_indent", "code::\n\n      six spaces\n        eight spaces\n"),
    ("literal", "adjacent_no_blank", "para::\n    lit right away\n"),
    # ----- doctest + line blocks -----
    ("doctest", "simple", ">>> print(\"hello\")\nhello\n>>> 1 + 1\n2\n"),
    ("doctest", "single", ">>> 2 + 2\n4\n"),
    ("doctest", "then_paragraph", ">>> x = 1\n\nAfter the doctest.\n"),
    ("doctest", "inside_list_item", "- item\n\n  >>> code()\n  result\n"),
    ("lineblock", "simple", "| Lend us a couple of bob till Thursday.\n| I am absolutely skint.\n"),
    ("lineblock", "nested", "| top one\n| top two\n|     nested one\n| back\n|\n| after empty\n"),
    ("lineblock", "continuation", "| A very long line\n  continued here\n| second\n"),
    ("lineblock", "double_nested", "| top\n|   n1\n|     n2\n| back\n"),
    ("lineblock", "after_paragraph", "Intro para.\n\n| line one\n| line two\n"),
    # ----- comments + targets -----
    ("comment_target", "comment_multiline", ".. This is a comment\n   that continues on\n   multiple lines.\n"),
    ("comment_target", "comment_empty_start", "..\n\n   Indented block attached\n   to an empty comment start.\n"),
    ("comment_target", "comment_bare", "..\n"),
    ("comment_target", "comment_weird_colons", ".. just a comment::  with weird colons\n"),
    ("comment_target", "comment_ragged", ".. first\n      deep\n   shallow\n"),
    ("comment_target", "comment_adjacent_pair", ".. one\n.. two\n"),
    ("comment_target", "target_internal", ".. _para-target:\n\nSome paragraph here.\n"),
    ("comment_target", "target_external_indirect", ".. _docutils: https://docutils.sourceforge.io/\n.. _indirect: docutils_\n"),
    ("comment_target", "target_multiline_uri", ".. _long: https://example.com/\n   path/here\n"),
    ("comment_target", "target_uri_spaces", ".. _a: B  Target_\n"),
    ("comment_target", "target_backtick_name", ".. _`name with: colon`: https://x/\n"),
    ("comment_target", "target_escaped_colon", ".. _a\\: b: https://y/\n"),
    ("comment_target", "target_anonymous_both", ".. __: https://example.com/1\n\n__ https://example.com/2\n"),
    ("comment_target", "target_chained_before_section", ".. _target1:\n.. _target2:\n\nSection Title\n=============\n"),
    ("comment_target", "target_phrase_indirect", ".. _a: `phrase ref`_\n"),
    ("comment_target", "target_dup_external", ".. _dup: https://1/\n\n.. _dup: https://2/\n"),
    ("comment_target", "target_dup_internal", ".. _t:\n\npara\n\n.. _t:\n\npara2\n"),
    ("comment_target", "target_before_paragraph", ".. _marker:\n\nFollowing paragraph.\n"),
    # ----- errors + system messages -----
    ("errors", "underline_too_short", "Long Section Title\n======\n"),
    ("errors", "underline_very_short", "Title\n===\n"),
    ("errors", "overline_mismatch", "=========\nTitle\n========\n\nbody\n"),
    ("errors", "incomplete_overline", "----\ntext with no closing\n"),
    ("errors", "short_overline", "==\nOverlong Title\n==\n\nbody\n"),
    ("errors", "skip_level", "A\n-\n\nB\n=\n\nC\n-\n\nD\n~\n\nbody\n"),
    ("errors", "unexpected_indent", "line one\nline two\n    Indented without blank line.\n"),
    ("errors", "single_line_indent_is_deflist", "line one\n    Indented without blank line.\n"),
    ("errors", "enum_start_info", "3. three\n4. four\n"),
    ("errors", "enum_empty_item_quote", "Para.\n\n    Q.\n"),
    ("errors", "nested_transition", "Para.\n\n    ----\n\n    quoted\n"),
    ("errors", "nested_title", "Para.\n\n    Fake\n    ====\n"),
    ("errors", "nested_title_in_list", "- item\n\n  ----\n\n  more\n"),
    ("errors", "quote_unindent", "Para.\n\n    quoted line\n  odd dedent\n"),
    # ----- adversarial mixtures -----
    ("mixtures", "kitchen_sink_sections", "Top\n===\n\n.. _mark:\n\npara\n\n----\n\nSub\n---\n\n- list\n- items\n\nBack\n====\n\nend\n"),
    ("mixtures", "list_quote_list", "- outer\n\n      quoted in item\n\n  - inner after quote\n"),
    ("mixtures", "literal_in_list", "- item with code::\n\n      indented code\n\n- next item\n"),
    ("mixtures", "deflist_in_quote", "Para.\n\n    term\n        def in quote\n"),
    ("mixtures", "comment_between_paragraphs", "one\n\n.. hidden note\n\ntwo\n"),
    ("mixtures", "targets_and_comments", ".. _t1: https://x/\n.. a comment\n.. _t2:\n\npara\n"),
    ("mixtures", "lineblock_then_list", "| a\n| b\n\n- item\n"),
    ("mixtures", "quote_with_doctest", "Para.\n\n    >>> quoted_code()\n    output\n"),
    ("mixtures", "tabbed_list", "- item\n\n\tcontinued via tab\n"),
    # ----- hardening round 2 (post-zero-divergence adversarial cases) -----
    ("hardening", "bullet_after_paragraph_adjacent", "para\n- item\n"),
    ("hardening", "roman_i_then_j", "i. x\nj. y\n"),
    ("hardening", "single_x_alpha", "x. ten\n"),
    ("hardening", "enum_line_as_title", "1. one\n=========\n\nbody\n"),
    ("hardening", "bullet_then_adornment", "- a\n----\n\nafter\n"),
    ("hardening", "target_camel_name", ".. _CamelCase  Name: https://x/\n"),
    ("hardening", "lone_double_colon", "::\n"),
    ("hardening", "trailing_expect_literal", "para::\n"),
    ("hardening", "target_then_adjacent_paragraph", ".. _t:\npara right after\n"),
    ("hardening", "comment_then_adjacent_text", ".. c\nnot indented\n"),
    ("hardening", "tab_in_literal", "code::\n\n    a\tb\n"),
    ("hardening", "sections_no_blank_between", "A\n=\nB\n=\n"),
    ("hardening", "body_adjacent_after_underline", "Title\n=====\nbody adjacent\n"),
    ("hardening", "quote_attribution_only", "Para.\n\n    -- Just Attribution\n"),
    ("hardening", "deep_def_indent", "term\n    def1\n        deeper block\n"),
    ("hardening", "lineblock_whole_doc", "|\n"),
    ("hardening", "adjacent_comments_with_bodies", ".. one\n   one body\n.. two\n"),
    ("hardening", "enum_paren_mismatch", "(1. x\n"),
    ("hardening", "anon_shortcut_adjacent_para", "__ https://x/\npara after\n"),
    ("hardening", "literal_inside_list_item", "- item\n\n  para::\n\n      lit in item\n"),
    ("hardening", "dashes_mid_quote_not_attribution", "Para.\n\n    a -- b stays text\n"),
    ("hardening", "attribution_then_more_attribution", "Para.\n\n    body\n\n    -- a\n\n    -- b\n"),
    ("hardening", "target_then_bullet_adjacent", ".. _t:\n- item\n"),
    ("hardening", "quoted_literal_at_eof", "q::\n\n> a\n> b\n"),
    ("hardening", "comment_tab_continuation", ".. c\n\tbody via tab\n"),
    ("hardening", "deflist_term_double_colon_blank_ok", "para::\n\n    real literal\n"),
    # ----- wave 2: inline basics (probe-verified inputs) -----
    ("inline_basics", "simple_emphasis", "before *emph* after\n"),
    ("inline_basics", "three_kinds", "*a* **b** ``c``\n"),
    ("inline_basics", "comma_after", "a *b*, c\n"),
    ("inline_basics", "word_chars_block", "a*b*c\n\n2*3*4\n"),
    ("inline_basics", "hyphen_before", "x-*emph* y\n"),
    ("inline_basics", "parens_around", "(*emph*)\n"),
    ("inline_basics", "quoted_suppression", "\"*\" and '*' and (*) stay plain\n"),
    ("inline_basics", "end_of_text_star", "word *\n"),
    ("inline_basics", "unclosed_emphasis", "*oops\n"),
    ("inline_basics", "unclosed_strong", "**oops\n"),
    ("inline_basics", "unclosed_literal", "``oops\n"),
    ("inline_basics", "unclosed_mid_text", "start *oops end\n"),
    ("inline_basics", "double_problematic", "(*emph *nope\n"),
    ("inline_basics", "no_nesting_emphasis", "*a **b** c*\n"),
    ("inline_basics", "no_nesting_strong", "**a *b* c**\n"),
    ("inline_basics", "triple_stars", "***x***\n"),
    ("inline_basics", "first_end_wins", "*word *word*\n"),
    ("inline_basics", "trailing_wordchar_problematic", "*emph*s\n"),
    ("inline_basics", "literal_protects_markup", "``*not markup*``\n"),
    ("inline_basics", "literal_keeps_backslash", "``a\\*b``\n"),
    ("inline_basics", "escaped_stars_plain", "\\*not markup\\*\n"),
    ("inline_basics", "punct_after_end", "*emph*. and *emph*-like and *emph*, done\n"),
    ("inline_basics", "escaped_space_joins", "one\\ two\n"),
    ("inline_basics", "markup_spans_lines", "*multi\nline* end\n"),
    ("inline_basics", "emphasis_in_quote", "Para.\n\n    quoted *emph* here\n"),
    ("inline_basics", "emphasis_in_list", "- item *emph* text\n"),
    ("inline_basics", "unclosed_in_list_item", "- *oops in item\n"),
    # ----- wave 2: inline in carriers (titles/terms/attributions/line blocks) -----
    ("inline_carriers", "markup_in_title", "The *Great* Title\n=================\n\nbody\n"),
    ("inline_carriers", "literal_in_title", "Using ``code`` Here\n===================\n\nbody\n"),
    ("inline_carriers", "unclosed_in_title", "Bad *title\n==========\n\nbody\n"),
    ("inline_carriers", "markup_in_term", "*term* text\n    definition\n"),
    ("inline_carriers", "markup_in_classifier", "term : *class*\n    definition\n"),
    ("inline_carriers", "unclosed_in_term", "*oops term\n    definition\n"),
    ("inline_carriers", "markup_in_attribution", "Para.\n\n    body\n\n    -- *Anon* Author\n"),
    ("inline_carriers", "markup_in_lineblock", "| plain line\n| *emph* line\n| ``lit`` line\n"),
    ("inline_carriers", "unclosed_in_lineblock", "| *oops line\n| second\n"),
    ("inline_carriers", "markup_title_dup_names", "*Same*\n======\n\nx\n\nSame\n====\n\ny\n"),
    # ----- wave 2: inline references (probe-verified inputs) -----
    ("inline_refs", "named_word_ref", "See word_ here.\n"),
    ("inline_refs", "cased_word_ref", "See Word_ now.\n"),
    ("inline_refs", "ref_with_target", "See word_ here.\n\n.. _word: https://example.com\n"),
    ("inline_refs", "phrase_ref", "See `Two Words`_ here.\n"),
    ("inline_refs", "phrase_ref_spaces", "See `two   words`_ (extra spaces) here.\n"),
    ("inline_refs", "phrase_ref_multiline", "See `two\nwords`_ here.\n"),
    ("inline_refs", "anonymous_word", "See word__ here.\n"),
    ("inline_refs", "anonymous_phrase", "See `some phrase`__ here.\n"),
    ("inline_refs", "anon_trailing_punct", "Anon word__.\n"),
    ("inline_refs", "embedded_uri", "See `text <https://x/>`_ here.\n"),
    ("inline_refs", "embedded_uri_anon", "See `text <https://x/>`__ here.\n"),
    ("inline_refs", "embedded_uri_multiline", "See `a b\n<https://long.example/\npath>`_ here.\n"),
    ("inline_refs", "embedded_alias", "See `text <alias_>`_ here.\n"),
    ("inline_refs", "embedded_alias_anon", "See `text <alias_>`__ here.\n"),
    ("inline_refs", "embedded_alias_phrase", "See `text <two words_>`_ here.\n"),
    ("inline_refs", "embedded_email", "See `mail me <foo@example.com>`_ now.\n"),
    ("inline_refs", "embedded_escaped_underscore", "See `text <alias\\_>`_ now.\n"),
    ("inline_refs", "embedded_relative_uri", "See `text <not-a-uri>`_ now.\n"),
    ("inline_refs", "embedded_omitted_text", "See `<https://x/>`_ here.\n"),
    ("inline_refs", "inline_internal_target", "Here is _`marked text` inline.\n"),
    ("inline_refs", "inline_target_cased", "Here is _`Two Words` inline.\n"),
    ("inline_refs", "underscore_word_plain", "A _word and _`target` here.\n"),
    ("inline_refs", "midword_target_plain", "midword_`no target` here.\n"),
    ("inline_refs", "bare_interpreted", "See `interpreted` here.\n"),
    ("inline_refs", "footnote_refs_all", "Refs [1]_ [#]_ [#label]_ [*]_ end.\n"),
    ("inline_refs", "footnote_ref_cased", "Ref [#Label]_ end.\n"),
    ("inline_refs", "footnote_ref_ten", "Ref [10]_ now.\n"),
    ("inline_refs", "footnote_ref_space_breaks", "Bad [1] _ and good [1]_ here.\n"),
    ("inline_refs", "citation_ref", "Cite [cite2020]_ end.\n"),
    ("inline_refs", "citation_ref_dotted", "Cite [Cite.2020-X]_ end.\n"),
    ("inline_refs", "substitution_refs", "A |sub| B |sub2|_ C |sub3|__ D.\n"),
    ("inline_refs", "substitution_cased", "A |Sub Name|_ B.\n"),
    ("inline_refs", "substitution_broken", "A |sub|_z now.\n"),
    ("inline_refs", "standalone_uris", "Go to https://x and http://example.com/path?q=1 now.\n"),
    ("inline_refs", "www_not_a_link", "Go to www.x.com now.\n"),
    ("inline_refs", "unknown_scheme", "See xyz:abc here.\n"),
    ("inline_refs", "uri_case_preserved", "See HTTPS://EXAMPLE.COM/Path here.\n"),
    ("inline_refs", "escaped_scheme", "See https\\://x.example/ here.\n"),
    ("inline_refs", "bare_email", "Write foo.bar-baz@example.com. Done.\n"),
    ("inline_refs", "mailto_email", "Write mailto:foo@example.com please.\n"),
    ("inline_refs", "uri_trailing_punct", "See https://x.example/, and (https://y.example/) or https://z.example/.\n"),
    ("inline_refs", "snake_case_ref", "See foo_bar_ here.\n"),
    ("inline_refs", "double_underscore_not_ref", "See foo__bar and __init__ here.\n"),
    ("inline_refs", "word_ref_end_sentence", "See word_. Done.\n"),
    ("inline_refs", "escaped_word_ref", "See word\\_ here.\n"),
    # ----- wave 2: inline roles (probe-verified inputs) -----
    ("inline_roles", "generic_roles", ":emphasis:`text` and :strong:`text` and :literal:`text` end.\n"),
    ("inline_roles", "sub_sup", "Water :sub:`2` and x :sup:`2` end.\n"),
    ("inline_roles", "title_aliases", ":title-reference:`Some Title` :title:`Some Title` :t:`Some Title` end.\n"),
    ("inline_roles", "abbrev_acronym", ":ab:`St. Nick` and :ac:`NATO` end.\n"),
    ("inline_roles", "suffix_syntax", "`text`:emphasis: and `text`:strong: end.\n"),
    ("inline_roles", "case_insensitive", ":EMPHASIS:`text` and `text`:SUP: end.\n"),
    ("inline_roles", "both_roles_error", ":emphasis:`text`:strong: end.\n"),
    ("inline_roles", "unknown_role", ":bogus:`x`\n\nnext para here\n"),
    ("inline_roles", "unknown_role_cased", ":BoGuS:`x` end.\n"),
    ("inline_roles", "unknown_role_repeat", ":bogus:`x` :bogus:`y`\n"),
    ("inline_roles", "unknown_role_suffix", "`x`:bogus: end.\n"),
    ("inline_roles", "pep_role", ":pep-reference:`8` and :PEP:`0` and :pep:`008` end.\n"),
    ("inline_roles", "pep_invalid", ":PEP:`99999` end.\n"),
    ("inline_roles", "rfc_role", ":rfc-reference:`2822` and :RFC:`0002822` and :RFC:`1` end.\n"),
    ("inline_roles", "rfc_fragment", ":RFC:`2822#section-3` end.\n"),
    ("inline_roles", "rfc_invalid", ":RFC:`0` end.\n"),
    ("inline_roles", "math_role", ":math:`x^2 + y_1` and :math:`a\\\\b` end.\n"),
    ("inline_roles", "code_role", ":code:`print(1)` end.\n"),
    ("inline_roles", "raw_role_bare", ":raw:`text` end.\n"),
    ("inline_roles", "unimplemented_roles", ":index:`x` end.\n"),
    ("inline_roles", "literal_role_escapes", ":literal:`a\\*b` end.\n"),
    ("inline_roles", "role_in_quote", "Para.\n\n    quoted :bogus:`x` here\n"),
    ("inline_roles", "colon_not_role", "see: `x` end.\n"),
    # ----- review round (adversarial-review confirmed findings, 2026-08-07) -----
    ("review", "attr_no_space", "Para.\n\n    body\n\n    --Author\n"),
    ("review", "attr_no_space_emdash", "Para.\n\n    body\n\n    \u2014Author\n"),
    ("review", "attr_multi_space", "Para.\n\n    body\n\n    --   Author\n"),
    ("review", "attr_ragged_continuation", "Para.\n\n    body\n\n    -- a,\n       b,\n      c\n"),
    ("review", "attr_deep_continuation", "Para.\n\n    body\n\n    -- a,\n         b\n"),
    ("review", "overline_too_short_warn", "====\nVery long title\n====\n\nbody\n"),
    ("review", "overline_too_short_leading_spaces", "=====\n   Long title text\n=====\n\nbody\n"),
    ("review", "overline_missing_underline_text", "====\nTitle\nnot underline\n\npara\n"),
    ("review", "overline_missing_underline_blank", "====\nTitle\n\npara\n"),
    ("review", "overline_diff_char_underline", "----\nTitle\n====\n\nbody\n"),
    ("review", "adornment_pair", "====\n----\n\npara\n"),
    ("review", "short_overline_pair", "--\n--\n\npara\n"),
    ("review", "nested_short_adornment", "Para.\n\n    ---\n    text\n"),
    ("review", "nested_short_adornment_alone", "Para.\n\n    ---\n\n    text\n"),
    ("review", "para_multiline_colon_adjacent_indent", "line one\nline two::\n    adjacent\n"),
    ("review", "doctest_indented_continuation", ">>> if x:\n...     y\n  indented output\nmore output\n\nafter\n"),
    ("review", "lineblock_ends_no_blank", "| a\n| b\nplain\n"),
    ("review", "lineblock_empty_inherits_depth", "| a\n|   n1\n|\n|   n2\n"),
    ("review", "lineblock_continuation_relative_indent", "| a\n  b\n    c\n\nafter\n"),
    ("review", "lineblock_continuation_after_empty", "|\n  cont\n\nafter\n"),
    ("review", "bare_double_underscore", "__\n\npara\n"),
    ("review", "anon_shortcut_continuation", "__ https://example.com/\n   path\n"),
    ("review", "malformed_target_double_colon", ".. _name::\n\npara\n"),
    ("review", "malformed_target_double_colon_uri", ".. _name:: uri\n"),
    ("review", "malformed_target_no_colon", ".. _name\n\npara\n"),
    ("review", "malformed_target_bare_anon", ".. __\n\npara\n"),
    ("review", "malformed_target_empty_name", ".. _: uri\n\npara\n"),
    ("review", "malformed_target_empty_backtick", ".. _``: https://x/\n"),
    ("review", "malformed_target_unclosed_backtick", ".. _`abc: x\n"),
    ("review", "target_multiline_backtick_name", ".. _`multi\n   line name`: https://x/\n\npara\n"),
    ("review", "target_multiline_plain_name", ".. _long\n   name: uri\n\npara\n"),
    ("review", "target_escaped_underscore_uri", ".. _a: uri\\_\n"),
    ("review", "target_space_joined_indirect", ".. _a: one\n   two_\n"),
    ("review", "enum_bare_successor", "1. one\n2.\n"),
    ("review", "enum_bare_third", "1. one\n2. two\n3.\n"),
    ("review", "enum_bare_pair", "1.\n2.\n"),
    ("review", "enum_explicit_after_auto", "#. one\n2. two\n"),
    ("review", "enum_auto_mid_explicit", "1. one\n#. two\n3. three\n"),
    ("review", "target_then_section_same_name", ".. _conflict: https://x/\n\nconflict\n========\n\nbody\n"),
    ("review", "section_then_target_same_name", "conflict\n========\n\n.. _conflict: https://x/\n\nbody\n"),
    ("review", "bullet_alone_blank_body", "-\n\n  body\n"),
    ("review", "cjk_underline_short", "\u65e5\u672c\u8a9e\n===\n\nbody\n"),
    ("review", "cjk_underline_between", "\u65e5\u672c\u8a9e\n====\n\nbody\n"),
    ("review", "cjk_underline_exact", "\u65e5\u672c\u8a9e\n======\n\nbody\n"),
    ("review", "explicit_double_space_target", "..  _t: https://x/\n"),
    ("review", "comment_triple_space", "..   comment text\n"),
    ("review", "classifier_multi_space", "term  :  classifier\n    def\n"),
    ("review", "short_overline_demote_deflist", "---\n    x\n"),
    ("review", "quoted_literal_then_indent", "intro::\n\n> line one\n  indented\n"),
    ("review", "established_styles_overunder", "=====\nA\n=====\n\nB\n-\n\n=====\nC\n=====\n\nD\n~\n\nbody\n"),
    ("mixtures", "everything_adjacent", "Head\n====\n\nterm\n    def\n\n- a\n- b\n\n1. one\n2. two\n\n::\n\n    lit\n\n.. done\n"),
]


def parse_pformat(text: str) -> str:
    parser = Parser()
    settings = get_default_settings(Parser)
    settings.report_level = 1
    settings.halt_level = 5
    settings.warning_stream = io.StringIO()
    settings.auto_id_prefix = "id"
    settings.id_prefix = ""
    document = new_document("<snippet>", settings)
    parser.parse(text, document)
    return document.pformat()


def kinds_of(text: str) -> set:
    parser = Parser()
    settings = get_default_settings(Parser)
    settings.report_level = 1
    settings.halt_level = 5
    settings.warning_stream = io.StringIO()
    settings.auto_id_prefix = "id"
    settings.id_prefix = ""
    document = new_document("<snippet>", settings)
    parser.parse(text, document)
    return {node.tagname for node in document.findall()}


def main() -> int:
    assert docutils.__version__ == EXPECTED_DOCUTILS, (
        f"docutils {docutils.__version__} != {EXPECTED_DOCUTILS}; "
        "regenerate with the pinned command in the module docstring"
    )

    names = [f"{family}.{name}" for family, name, _ in CASES]
    assert len(names) == len(set(names)), "family-qualified case names must be unique"
    assert len(CASES) >= 200, f"corpus degenerated: {len(CASES)} cases"

    floors = {
        "paragraphs": 4, "sections": 8, "transition": 4, "lists_bullet": 8,
        "lists_enum": 8, "deflist": 8, "quote": 8, "literal": 8,
        "comment_target": 8, "lineblock": 4, "doctest": 4, "errors": 12,
        "hardening": 20, "mixtures": 8, "review": 45, "inline_basics": 25, "inline_carriers": 10, "inline_refs": 40, "inline_roles": 20,
    }
    counts: dict = {}
    for family, _, _ in CASES:
        counts[family] = counts.get(family, 0) + 1
    for family, floor in floors.items():
        assert counts.get(family, 0) >= floor, (
            f"family {family}: {counts.get(family, 0)} < floor {floor}"
        )

    out_cases = []
    bad = []
    for family, name, rst in CASES:
        stray = kinds_of(rst) - SUPPORTED_KINDS
        if stray:
            bad.append(f"{name}: unsupported kinds {sorted(stray)}")
            continue
        pseudo = parse_pformat(rst)
        # Directive machinery is wave 3: a snippet that reaches docutils'
        # directive parsing is out of corpus scope even when its output
        # nodes are all "supported". Two guards: output text, and directive
        # syntax in the SOURCE (catches quietly-succeeding directives like
        # `.. highlights::` whose output nodes are all supported kinds).
        if "Unknown directive type" in pseudo or "No directive entry" in pseudo:
            bad.append(f"{name}: snippet reaches directive machinery")
            continue
        if re.search(r"^\s*\.\. +(?!_)[\w][\w.+:-]* *::", rst, re.M):
            bad.append(f"{name}: directive-shaped syntax in source")
            continue
        out_cases.append({
            "name": f"{family}.{name}",
            "family": family,
            "rst": rst,
            "pseudo_xml": pseudo,
        })
    if bad:
        print("CORPUS SCOPE VIOLATIONS:", file=sys.stderr)
        for b in bad:
            print(f"  {b}", file=sys.stderr)
        return 1

    fixture = {
        "docutils_version": docutils.__version__,
        "generator": "tools/gen_doctree_fixture.py",
        "settings": {
            "report_level": 1,
            "halt_level": 5,
            "auto_id_prefix": "id",
            "id_prefix": "",
        },
        "cases": out_cases,
    }
    out_path = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "doctree_differential.json"
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(fixture, f, indent=2, sort_keys=True, ensure_ascii=False)
        f.write("\n")
    print(f"wrote {out_path}: {len(out_cases)} cases, docutils {docutils.__version__}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
