//! Unit tests for the py-domain parse-time configuration bundle.
//!
//! Every expected value here is probe-verified against sphinx 9.1.0 /
//! docutils 0.22.4 — the defaults by probe D and the `max_len` truthiness
//! rule by probes C1-C5 of the research spec
//! (`docs/plans/2026-09-01-m2-wave4.5-research-spec-signature-config.md`
//! §1.2), never from memory.

use super::PySigConfig;
use crate::config::BuildConfig;

#[test]
fn defaults_match_sphinx() {
    let py = PySigConfig::default();
    assert_eq!(py.maximum_signature_line_length, None);
    assert_eq!(py.python_maximum_signature_line_length, None);
    assert!(py.python_trailing_comma_in_multi_line_signatures);
    assert!(!py.python_display_short_literal_types);
    assert!(!py.python_use_unqualified_type_names);
    assert!(py.toc_object_entries);
    assert_eq!(py.toc_object_entries_show_parents, "domain");
    assert!(py.add_function_parentheses);
    assert!(py.add_module_names);
    assert!(!py.strip_signature_backslash);
}

/// The bundle is a projection of the build configuration; a default
/// `BuildConfig` must project onto a default `PySigConfig`, or the parse
/// layer would silently disagree with `conf.py`.
#[test]
fn a_default_build_config_projects_onto_the_default_bundle() {
    assert_eq!(
        PySigConfig::from(&BuildConfig::default()),
        PySigConfig::default()
    );
}

#[test]
fn every_field_is_carried_over_from_the_build_config() {
    let config = BuildConfig {
        maximum_signature_line_length: Some(7),
        python_maximum_signature_line_length: Some(3),
        python_trailing_comma_in_multi_line_signatures: false,
        python_display_short_literal_types: true,
        python_use_unqualified_type_names: true,
        toc_object_entries: false,
        toc_object_entries_show_parents: "hide".to_string(),
        add_function_parentheses: false,
        add_module_names: false,
        strip_signature_backslash: true,
        ..BuildConfig::default()
    };

    assert_eq!(
        PySigConfig::from(&config),
        PySigConfig {
            maximum_signature_line_length: Some(7),
            python_maximum_signature_line_length: Some(3),
            python_trailing_comma_in_multi_line_signatures: false,
            python_display_short_literal_types: true,
            python_use_unqualified_type_names: true,
            toc_object_entries: false,
            toc_object_entries_show_parents: "hide".to_string(),
            add_function_parentheses: false,
            add_module_names: false,
            strip_signature_backslash: true,
        }
    );
}

/// `max_len = python_… or … or 0` (`domains/python/_object.py:291-295`)
/// tests TRUTHINESS: a py-specific 0 is falsy and falls through to the
/// global key (research spec §1.2, probe C4), which is the trap this
/// matrix exists to pin.
#[test]
fn max_len_follows_the_python_or_global_or_zero_chain() {
    let with = |python: Option<i64>, global: Option<i64>| {
        PySigConfig {
            python_maximum_signature_line_length: python,
            maximum_signature_line_length: global,
            ..PySigConfig::default()
        }
        .max_len()
    };

    assert_eq!(with(None, None), 0, "both unset: the feature is off");
    assert_eq!(with(Some(5), None), 5, "probe C1: the py key wins");
    assert_eq!(with(None, Some(5)), 5, "probe C3: fall through to global");
    assert_eq!(
        with(Some(0), Some(1)),
        1,
        "probe C4: a falsy py 0 falls through to the global key"
    );
    assert_eq!(with(Some(2), Some(9)), 2, "probe C2: the py key wins again");
    assert_eq!(with(Some(0), None), 0, "probe C5: 0 everywhere means off");
    assert_eq!(
        with(None, Some(0)),
        0,
        "a global 0 is falsy too and resolves to the same off"
    );
    assert_eq!(
        with(Some(-1), Some(4)),
        -1,
        "a negative is truthy in python, so it wins and the > max_len > 0 \
         guard at the call site turns the feature off"
    );
}
