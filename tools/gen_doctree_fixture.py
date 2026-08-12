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
    "footnote",
    "citation",
    "label",
    "field_list",
    "field",
    "field_name",
    "field_body",
    "option_list",
    "option_list_item",
    "option_group",
    "option",
    "option_string",
    "option_argument",
    "description",
    "table",
    "tgroup",
    "colspec",
    "thead",
    "tbody",
    "row",
    "entry",
    # wave 3: admonitions
    "note",
    "warning",
    "tip",
    "hint",
    "important",
    "caution",
    "danger",
    "error",
    "attention",
    "admonition",
    "image",
    "topic",
    "sidebar",
    "subtitle",
    "rubric",
    "compound",
    "container",
}

# Families whose snippets intentionally exercise the directive machinery.
DIRECTIVE_FAMILIES = ("dir_core", "dir_admonitions", "dir_options", "dir_image", "dir_body")

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
    # ----- wave 2: footnote/citation definitions (probe-verified) -----
    ("footnotes", "manual_numbered", ".. [1] A numbered footnote.\n"),
    ("footnotes", "auto_numbered", ".. [#] An auto-numbered footnote.\n"),
    ("footnotes", "auto_labeled", ".. [#note] Labeled auto footnote.\n"),
    ("footnotes", "symbol", ".. [*] Symbol footnote.\n"),
    ("footnotes", "citation_simple", ".. [CIT2020] A citation body.\n"),
    ("footnotes", "empty_footnote", ".. [1]\n"),
    ("footnotes", "empty_citation", ".. [CIT]\n"),
    ("footnotes", "blank_then_body", ".. [1]\n\n   Body after blank.\n"),
    ("footnotes", "multiline_body", ".. [CIT] line one\n   line two at 3\n     line three deeper\n"),
    ("footnotes", "multi_paragraph", ".. [2] para one\n\n   para two\n"),
    ("footnotes", "list_in_footnote", ".. [3] intro\n\n   - a\n   - b\n"),
    ("footnotes", "duplicate_manual", ".. [1] first\n.. [1] second\n"),
    ("footnotes", "auto_vs_citation_dup", ".. [#x] auto footnote\n.. [X] citation\n"),
    ("footnotes", "symbol_pair_no_warning", ".. [*] one\n.. [*] two\n"),
    ("footnotes", "target_vs_footnote_dup", ".. _1: https://x/\n\n.. [1] footnote\n"),
    ("footnotes", "refs_and_defs", "See [1]_ and [#]_ here.\n\n.. [1] first\n.. [#] auto\n"),
    # ----- wave 2: field + option lists (probe-verified) -----
    ("fields", "basic_mid_document", "A paragraph first.\n\n:name: value\n:other: thing\n"),
    ("fields", "doc_start_biblio", ":Author: Jane\n:Date: 2026\n\nBody.\n"),
    ("fields", "no_space_not_field", ":field:value-no-space\n"),
    ("fields", "double_colon_not_field", ":: value\n"),
    ("fields", "interior_colon", ":fie:ld: value\n"),
    ("fields", "escaped_colon_name", ":field\\: colon: value\n"),
    ("fields", "role_not_field", ":code:`x`: description\n"),
    ("fields", "markup_in_name", ":*emph name*: value\n"),
    ("fields", "unclosed_markup_in_name", ":*oops: value\n"),
    ("fields", "continuation_indent", ":field: first line\n   continuation at indent 3\n"),
    ("fields", "body_next_line", ":field:\n    body starts on the next line\n"),
    ("fields", "empty_body", ":empty:\n\nnext para\n"),
    ("fields", "deeper_indent_error", ":f: first\n      more\n        deeper\n"),
    ("fields", "nested_field_in_body", ":field: :not-a-field: inside body\n"),
    ("fields", "ends_no_blank", ":field: body\nnot part of it\n"),
    ("fields", "blank_separated_merge", ":a: one\n\n:b: two\n"),
    ("fields", "paragraph_not_interrupted", "text line\n:field: value\n"),
    ("options", "angle_arg", "-f <file>  Use this file.\n"),
    ("options", "synonyms", "-f FILE, --file=FILE  Specify the file.\n"),
    ("options", "attached_arg", "-oVALUE  Attached argument.\n"),
    ("options", "dos_options", "/INPUT=FILE  Dos style.\n\n/IN FILE  Spaced.\n"),
    ("options", "one_space_not_option", "-a description-one-space\n"),
    ("options", "comma_in_arg_rejected", "-f x,y  Comma inside arg.\n"),
    ("options", "two_word_arg_rejected", "--file A B  Two-word arg without angles.\n"),
    ("options", "digit_start_rejected", "-o2x  Digit-start attached.\n"),
    ("options", "bare_markers_paragraph", "-a\n-b\n"),
    ("options", "desc_next_line", "-x\n   Description on the next line.\n"),
    ("options", "angle_with_spaces", "-p <port number>  Port to use.\n"),
    ("options", "plus_option", "+x  Enable x.\n"),
    # ----- wave 2: grid tables (probe-verified) -----
    ("tables_grid", "minimal_2x2", "+----+----+\n| A  | B  |\n+----+----+\n| C  | D  |\n+----+----+\n"),
    ("tables_grid", "colwidths", "+---+---------+--+\n| a | bbbbbbb | c|\n+---+---------+--+\n"),
    ("tables_grid", "header_sep", "+----+----+\n| H1 | H2 |\n+====+====+\n| C  | D  |\n+----+----+\n"),
    ("tables_grid", "two_header_rows", "+----+----+\n| H1 | H2 |\n+----+----+\n| H3 | H4 |\n+====+====+\n| C  | D  |\n+----+----+\n"),
    ("tables_grid", "empty_header", "+----+----+\n+====+====+\n| C  | D  |\n+----+----+\n"),
    ("tables_grid", "column_span", "+----+----+\n| A  | B  |\n+----+----+\n| merged  |\n+----+----+\n"),
    ("tables_grid", "row_span", "+------+----+\n| span | B  |\n|      +----+\n|      | D  |\n+------+----+\n"),
    ("tables_grid", "multiline_cell", "+----------+----+\n| Cells may| B  |\n| span.    |    |\n+----------+----+\n"),
    ("tables_grid", "multi_para_cell", "+-------------+----+\n| para one    | B  |\n|             |    |\n| para two    |    |\n+-------------+----+\n"),
    ("tables_grid", "list_in_cell", "+----------+----+\n| - item   | B  |\n| - two    |    |\n+----------+----+\n"),
    ("tables_grid", "empty_cells", "+----+----+\n|    |    |\n+----+----+\n"),
    ("tables_grid", "borders_only", "+----+----+\n+----+----+\n"),
    ("tables_grid", "lone_border", "+----+----+\n"),
    ("tables_grid", "right_border_misaligned", "+----+----+\n| A  | B   |\n+----+----+\n"),
    ("tables_grid", "short_bottom_border", "+----+----+\n| A  | B  |\n+----+---+\n"),
    ("tables_grid", "unclosed_table", "+----+----+\n| A  | B  |\n"),
    ("tables_grid", "eq_border_cannot_close", "+----+\n| A  |\n+====+\n"),
    ("tables_grid", "nested_indent_in_cell", "+------------+\n|   deep     |\n| shallow    |\n+------------+\n"),
    ("tables_grid", "table_in_list_item", "- item\n\n  +----+----+\n  | A  | B  |\n  +----+----+\n"),
    ("tables_grid", "text_after_table", "+----+----+\n| A  | B  |\n+----+----+\n\nafter para\n"),
    # ----- wave 2: simple tables (probe-verified) -----
    ("tables_simple", "basic", "=====  =====\nA      B\nC      D\n=====  =====\n"),
    ("tables_simple", "header", "=====  =====\nH1     H2\n=====  =====\nA      B\n=====  =====\n"),
    ("tables_simple", "multiline_row", "=====  =====\nfirst  cell\nmore   text\n-----  -----\nnext   row\n=====  =====\n"),
    ("tables_simple", "column_span_rule", "=====  =====\nmerged cells\n------------\nA      B\n=====  =====\n"),
    ("tables_simple", "right_edge_overflow", "=====  =====\nA      B and this extends beyond\n=====  =====\n"),
    ("tables_simple", "single_run_not_table", "=====\nA\n=====\n"),
    ("tables_simple", "borders_only", "=====  =====\n=====  =====\n"),
    ("tables_simple", "no_bottom_border", "=====  =====\nA      B\n"),
    ("tables_simple", "border_mismatch", "=====  =====\nA      B\n===  ===\n"),
    ("tables_simple", "text_after_table_no_blank", "=====  =====\nA      B\n=====  =====\ntrailing text\n"),
    ("tables_simple", "margin_text", "=====  =====\nA     xB\n=====  =====\n"),
    ("tables_simple", "three_columns", "===  ===  ===\na    b    c\nd    e    f\n===  ===  ===\n"),
    ("tables_simple", "in_paragraph_absorbed", "para line\n=====  =====\nA      B\n=====  =====\n"),
    # ----- wave 2 hardening: construct interactions -----
    ("w2_hardening", "table_in_quote", "Para.\n\n    +----+----+\n    | A  | B  |\n    +----+----+\n"),
    ("w2_hardening", "footnote_in_quote", "Para.\n\n    .. [1] quoted footnote\n"),
    ("w2_hardening", "fields_in_footnote", ".. [1] intro\n\n   :key: value\n"),
    ("w2_hardening", "refs_in_table_cells", "+----------------+\n| See word_ here |\n+----------------+\n"),
    ("w2_hardening", "markup_in_field_body", ":field: has *emph* and ``lit``\n"),
    ("w2_hardening", "role_in_term", ":sub:`x` term\n    definition\n"),
    ("w2_hardening", "footnote_ref_in_title", "Title [1]_ Here\n===============\n\nbody\n"),
    ("w2_hardening", "uri_in_lineblock", "| See https://example.com/ here\n| second\n"),
    ("w2_hardening", "emphasis_across_lines_in_cell", "+--------------+\n| *multi       |\n| line* cell   |\n+--------------+\n"),
    ("w2_hardening", "substitution_in_option_desc", "-a  Uses |sub| here.\n"),
    ("w2_hardening", "target_after_footnote", ".. [1] note\n.. _t: https://x/\n"),
    ("w2_hardening", "anon_ref_then_anon_target", "See thing__ here.\n\n__ https://x/\n"),
    ("w2_hardening", "inline_target_dup_section", "A _`dup` inline.\n\ndup\n===\n\nbody\n"),
    ("w2_hardening", "field_then_option", ":field: value\n\n-a  desc\n"),
    ("w2_hardening", "table_then_footnote_adjacent", "+----+\n| A  |\n+----+\n.. [1] adjacent note\n"),
    ("w2_hardening", "problematic_in_deep_nesting", "- item\n\n  - inner *oops\n"),
    ("w2_hardening", "literal_role_vs_literal_block", ":literal:`x`::\n\n    block\n"),
    ("w2_hardening", "pep_in_footnote", ".. [1] See :pep:`8` for style.\n"),
    # ----- wave 3: directive core + admonitions (probe-verified) -----
    ("dir_core", "unknown_directive", ".. bogusdirective::\n\n   content\n"),
    ("dir_core", "unknown_dotted_name", ".. foo.bar::\n\n   content\n"),
    ("dir_core", "unknown_domain_name", ".. py:function:: foo(x)\n\n   content\n"),
    ("dir_core", "dangling_separator_comment", ".. note-::\n\n   content\n"),
    ("dir_core", "no_space_paragraph", "..note::\n\n   Body text.\n"),
    ("dir_core", "single_colon_comment", ".. note:\n\n   Body text.\n"),
    ("dir_core", "two_spaces_comment", ".. note  ::\n\n   Body text.\n"),
    ("dir_core", "one_space_before_colons_ok", ".. note ::\n\n   Body text.\n"),
    ("dir_core", "case_insensitive", ".. NOTE::\n\n   Body text.\n"),
    ("dir_core", "leading_underscore_target", ".. _note::\n\n   content\n"),
    ("dir_admonitions", "note_indented_body", ".. note::\n\n   Body text.\n"),
    ("dir_admonitions", "note_inline_content", ".. note:: inline text\n"),
    ("dir_admonitions", "note_inline_plus_body", ".. note:: inline text\n\n   Body.\n"),
    ("dir_admonitions", "note_class_option", ".. note:: inline text\n   :class: foo\n\n   Body.\n"),
    ("dir_admonitions", "note_invalid_option_block", ".. note:: inline text\n   :class: foo\n   more inline text same para as option???\n\n   Body.\n"),
    ("dir_admonitions", "note_unknown_option", ".. note::\n   :bogus: x\n\n   Body.\n"),
    ("dir_admonitions", "empty_note_error", ".. note::\n"),
    ("dir_admonitions", "all_admonition_kinds", ".. warning:: w\n\n.. tip:: t\n\n.. danger:: d\n\n.. attention:: a\n"),
    ("dir_admonitions", "generic_admonition", ".. admonition:: Custom Title\n\n   Body text.\n"),
    ("dir_admonitions", "generic_admonition_class", ".. admonition:: T\n   :class: special\n\n   Body.\n"),
    ("dir_admonitions", "generic_missing_arg", ".. admonition::\n\n   Body.\n"),
    ("dir_admonitions", "note_nested_list", ".. note::\n\n   - a\n   - b\n"),
    ("dir_admonitions", "note_named", ".. note::\n   :name: my-note\n\n   Body.\n"),
    ("dir_admonitions", "nested_admonition", ".. note::\n\n   .. warning::\n\n      inner\n"),
    ("dir_admonitions", "directive_no_blank_after", ".. note:: content\nadjacent para\n"),
    # ----- wave 3: option/argument/content machinery (probe families O/A/C/X) -----
    ("dir_options", "unknown_option_uppercase_name", ".. NOTE::\n   :bogus: x\n\n   Body.\n"),
    ("dir_options", "duplicate_option", ".. note::\n   :class: a\n   :class: b\n\n   Body.\n"),
    ("dir_options", "duplicate_option_mixed_case", ".. note::\n   :Class: a\n   :class: b\n\n   Body.\n"),
    ("dir_options", "multiword_field_name", ".. note::\n   :class extra: v\n\n   Body.\n"),
    ("dir_options", "multiword_beats_unknown", ".. note::\n   :bogus: x\n   :class extra: v\n\n   Body.\n"),
    ("dir_options", "class_empty_value", ".. note::\n   :class:\n\n   Body.\n"),
    ("dir_options", "name_empty_value", ".. note::\n   :name:\n\n   Body text.\n"),
    ("dir_options", "name_and_class", ".. note::\n   :class: foo bar\n   :name: target one\n\n   Body.\n"),
    ("dir_options", "option_value_continuation", ".. note::\n   :class: foo\n      bar continued\n\n   Body text.\n"),
    ("dir_options", "options_after_blank_are_content", ".. note::\n   :class: foo\n\n   :name: bar\n\n   Body.\n"),
    ("dir_options", "two_blanks_before_content", ".. note::\n   :class: foo\n\n\n   Body after two blank lines.\n"),
    ("dir_options", "malformed_field_marker_to_content", ".. note::\n   :class value\n\n   Body.\n"),
    ("dir_options", "field_body_blockquote_promotion", ".. note::\n   :class: first para\n\n       second para\n\n   Body text.\n"),
    ("dir_options", "admonition_multiline_title", ".. admonition:: The Title\n   continues here\n\n   Body text.\n"),
    ("dir_options", "admonition_punct_title_class", ".. admonition:: !!!\n\n   Body.\n"),
    ("dir_options", "note_empty_uppercase", ".. NOTE::\n"),
    ("dir_options", "note_marker_line_content_only", ".. note:: This whole line becomes content, not an argument.\n"),
    ("dir_options", "warning_continuation_content", ".. warning:: Danger\n   ahead. This continues the paragraph.\n\n   Second paragraph of warning.\n"),
    ("dir_options", "unexpected_indentation_in_note", ".. note::\n\n   a\n     b\n"),
    ("dir_options", "note_content_unindent_warning", ".. note::\n\n   para\nafter\n"),
    ("dir_options", "trailing_blanks_rawsource", ".. frobnicate:: x\n\n   c\n\n\nafter\n"),
    ("dir_options", "trailing_blanks_eof", ".. frobnicate:: x\n\n   c\n\n\n"),
    ("dir_options", "trailing_blank_single", ".. frobnicate:: x\n\n   c\n\nafter\n"),
    ("dir_options", "consecutive_directives_no_blank", ".. note:: one\n.. note:: two\n"),
    # ----- wave 3: image directive (converters + target wrapping) -----
    ("dir_image", "minimal", ".. image:: picture.png\n"),
    ("dir_image", "multiword_uri_collapses", ".. image:: picture with spaces.png\n"),
    ("dir_image", "multiline_uri", ".. image:: pic\n   ture.png\n"),
    ("dir_image", "full_options", ".. image:: picture.png\n   :alt: alt text\n   :height: 100px\n   :width: 200 px\n   :scale: 50 %\n   :align: left\n"),
    ("dir_image", "align_vertical_error", ".. image:: pic.png\n   :align: top\n"),
    ("dir_image", "align_invalid_choice", ".. image:: pic.png\n   :align: sideways\n"),
    ("dir_image", "scale_not_number", ".. image:: pic.png\n   :scale: notanumber\n"),
    ("dir_image", "scale_negative", ".. image:: pic.png\n   :scale: -5\n"),
    ("dir_image", "width_banana", ".. image:: pic.png\n   :width: banana\n"),
    ("dir_image", "width_percentage", ".. image:: pic.png\n   :width: 50%\n"),
    ("dir_image", "width_unitless", ".. image:: pic.png\n   :width: 120\n"),
    ("dir_image", "height_bad_unit", ".. image:: pic.png\n   :height: 10banana\n"),
    ("dir_image", "height_decimal_em", ".. image:: pic.png\n   :height: 1.5em\n"),
    ("dir_image", "target_url", ".. image:: pic.png\n   :target: https://example.com/page\n"),
    ("dir_image", "target_refname_simple", ".. image:: pic.png\n   :target: sometarget_\n"),
    ("dir_image", "target_refname_phrase", ".. image:: pic.png\n   :target: `some phrase`_\n"),
    ("dir_image", "target_empty", ".. image:: pic.png\n   :target:\n"),
    ("dir_image", "loading_option", ".. image:: pic.png\n   :loading: lazy\n"),
    ("dir_image", "class_and_name", ".. image:: pic.png\n   :class: big shot\n   :name: my pic\n"),
    ("dir_image", "missing_arg", ".. image::\n"),
    ("dir_image", "content_not_permitted", ".. image:: pic.png\n\n   caption text\n"),
    ("dir_image", "second_uri_line_no_blank", ".. image:: pic.png\n   second line of uri\n"),
    # ----- wave 3: simple body directives -----
    ("dir_body", "topic_basic", ".. topic:: Topic Title\n\n   Topic body paragraph.\n"),
    ("dir_body", "topic_no_body", ".. topic:: Topic Title\n"),
    ("dir_body", "topic_in_note", ".. note::\n\n   .. topic:: Inner\n\n      body\n"),
    ("dir_body", "topic_in_list_item", "- item\n\n  .. topic:: Inner\n\n     body\n"),
    ("dir_body", "topic_class_name", ".. topic:: T\n   :class: special\n   :name: my topic\n\n   Body.\n"),
    ("dir_body", "topic_markup_title", ".. topic:: *emphasized* title\n\n   Body.\n"),
    ("dir_body", "sidebar_title_body", ".. sidebar:: Sidebar Title\n\n   Sidebar body.\n"),
    ("dir_body", "sidebar_subtitle", ".. sidebar:: Sidebar Title\n   :subtitle: Sidebar Subtitle\n\n   Sidebar body.\n"),
    ("dir_body", "sidebar_subtitle_no_title", ".. sidebar::\n   :subtitle: A Subtitle\n\n   Body text.\n"),
    ("dir_body", "sidebar_no_title", ".. sidebar::\n\n   Body only.\n"),
    ("dir_body", "sidebar_nested_error", ".. sidebar:: Outer\n\n   Outer body.\n\n   .. sidebar:: Inner\n\n      Inner body.\n"),
    ("dir_body", "topic_in_sidebar", ".. sidebar:: Outer\n\n   .. topic:: Inner Topic\n\n      body\n"),
    ("dir_body", "rubric_minimal", ".. rubric:: This is a rubric\n"),
    ("dir_body", "rubric_options", ".. rubric:: Named rubric\n   :class: myrubricclass\n   :name: rub1\n"),
    ("dir_body", "rubric_markup", ".. rubric:: A *marked up* rubric\n"),
    ("dir_body", "rubric_content_error", ".. rubric:: Title\n\n   body not allowed\n"),
    ("dir_body", "rubric_missing_arg", ".. rubric::\n"),
    ("dir_body", "epigraph_attribution", ".. epigraph::\n\n   Epigraph text.\n\n   -- Attribution\n"),
    ("dir_body", "highlights_basic", ".. highlights::\n\n   Highlighted text.\n"),
    ("dir_body", "pull_quote_basic", ".. pull-quote::\n\n   Pulled text.\n"),
    ("dir_body", "epigraph_empty", ".. epigraph::\n"),
    ("dir_body", "epigraph_marker_line", ".. epigraph:: text on the marker line\n"),
    ("dir_body", "epigraph_unknown_option", ".. epigraph::\n   :class: x\n\n   text\n"),
    ("dir_body", "compound_two_paras", ".. compound::\n\n   First paragraph of compound.\n\n   Second paragraph of compound.\n"),
    ("dir_body", "compound_empty_error", ".. compound::\n"),
    ("dir_body", "compound_class", ".. compound::\n   :class: custom\n\n   Body.\n"),
    ("dir_body", "container_no_class", ".. container::\n\n   Container body.\n"),
    ("dir_body", "container_classes", ".. container:: custom-class another-class\n\n   Container body.\n"),
    ("dir_body", "container_bad_class", ".. container:: !!!\n\n   Body.\n"),
    ("dir_body", "container_named", ".. container:: cls\n   :name: cont\n\n   Body.\n"),
    ("dir_body", "parsed_literal_inline", ".. parsed-literal::\n\n   Text with *emphasis* and **strong** and a\n   `link <http://example.com>`_.\n"),
    ("dir_body", "parsed_literal_class", ".. parsed-literal::\n   :class: code-ish\n   :name: pl1\n\n   plain \\*escaped\\* text\n"),
    # ----- wave 2 review round (Sonnet adversarial review, 2026-08-07) -----
    ("review2", "multi_segment_role", ":py:func:`target` end.\n"),
    ("review2", "multi_segment_role_three", ":a:b:c:`text` end.\n"),
    ("review2", "multi_segment_unknown", ":math:sub:`text` end.\n"),
    ("review2", "embedded_link_interior_gt", "`text <a>x>`_ end.\n"),
    ("review2", "embedded_link_escaped_gt", "`text <a\\>`_ end.\n"),
    ("review2", "simple_table_multiparagraph_cell", "=====  =====\ncol 1  col 2\n=====  =====\n3      - Second column of row 3.\n\n       - Second item in bullet\n         list (row 3, column 2).\n4      x\n=====  =====\n"),
    ("review2", "grid_left_edge_break", "+---+\n| A |\nxyz\n+---+\n"),
    ("review2", "grid_width4_border_not_table", "+--+\n|AB|\n+--+\n"),
    ("review2", "grid_nonrectangular_incomplete", "+--------------+--------------+\n| A bad table. |              |\n+--------------+              |\n| Cells must be rectangles.   |\n+-----------------------------+\n"),
    ("review2", "grid_cjk_cells", "+--------+------+\n| \u6f22\u5b57   | col2 |\n+--------+------+\n| x      | y    |\n+--------+------+\n"),
    ("review2", "simple_cjk_cells", "=====  =====\ncol 1  col 2\n=====  =====\n\u6f22\u5b57   B\n=====  =====\n"),
    ("review2", "footnote_two_spaces", ".. [1]  Two spaces after label.\n"),
    ("review2", "footnote_two_spaces_continuation", ".. [1]  First line two spaces.\n   Continuation at indent 3.\n"),
    ("review2", "citation_two_spaces", ".. [CIT]  Two spaces citation.\n"),
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
        "hardening": 20, "mixtures": 8, "review": 45, "inline_basics": 25, "inline_carriers": 10, "inline_refs": 40, "inline_roles": 20, "footnotes": 14, "fields": 15, "options": 10, "tables_grid": 18, "tables_simple": 12, "w2_hardening": 15, "review2": 12, "dir_core": 10, "dir_admonitions": 14, "dir_options": 20, "dir_image": 18, "dir_body": 30,
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
        if family not in DIRECTIVE_FAMILIES and (
            "Unknown directive type" in pseudo or "No directive entry" in pseudo
        ):
            bad.append(f"{name}: snippet reaches directive machinery")
            continue
        if family not in DIRECTIVE_FAMILIES and re.search(
            r"^\s*\.\. +(?!_)[\w][\w.+:-]* *::", rst, re.M
        ):
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
