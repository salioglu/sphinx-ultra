//! The Python domain (M2 wave 4.5).
//!
//! This module root carries the parse-time configuration bundle that the py
//! object directives, the signature parser and the `fix_parens` xref roles
//! read. The directives themselves land on top of this file in the next
//! task; nothing here walks a doctree.

pub mod annotations;
pub mod arglist;
pub mod expr;
pub mod pycode;

#[cfg(test)]
mod tests;

/// The object-signature / py-domain configuration the *read phase* consumes,
/// lifted out of [`crate::config::BuildConfig`] so the parser depends on ten
/// values instead of the whole build configuration.
///
/// Sphinx registers these across two files — `sphinx/config.py:248-281` for
/// the four domain-agnostic keys and `sphinx/directives/__init__.py:370-372`
/// for `strip_signature_backslash`, `sphinx/domains/python/__init__.py:
/// 1105-1122` for the `python_*` family — and every one of them is rebuild
/// category `'env'` (research spec §7), i.e. a read-phase input whose change
/// invalidates every parsed document.
///
/// `modindex_common_prefix` is deliberately *not* here: it is consumed by the
/// python module index at write time, not by the parse layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PySigConfig {
    /// `maximum_signature_line_length` (`config.py:279-281`), the global
    /// wrap threshold shared by the py/js/c/cpp domains. See [`Self::max_len`].
    pub maximum_signature_line_length: Option<i64>,
    /// `python_maximum_signature_line_length`
    /// (`domains/python/__init__.py:1108-1113`), the py-domain override.
    pub python_maximum_signature_line_length: Option<i64>,
    /// `python_trailing_comma_in_multi_line_signatures`
    /// (`domains/python/__init__.py:1114-1119`): the
    /// `multi_line_trailing_comma` attribute on a wrapped
    /// `desc_parameterlist`.
    pub python_trailing_comma_in_multi_line_signatures: bool,
    /// `python_display_short_literal_types`
    /// (`domains/python/__init__.py:1120-1122`): render `Literal['a', 'b']`
    /// as `'a' | 'b'`.
    pub python_display_short_literal_types: bool,
    /// `python_use_unqualified_type_names`
    /// (`domains/python/__init__.py:1105-1107`): emit a
    /// `pending_xref_condition` pair so the resolver can show the last
    /// dotted segment of a resolved annotation.
    pub python_use_unqualified_type_names: bool,
    /// `toc_object_entries` (`config.py:250`): whether object descriptions
    /// get `_toc_name`/`_toc_parts` and therefore TOC entries at all.
    pub toc_object_entries: bool,
    /// `toc_object_entries_show_parents` (`config.py:251-253`), an
    /// `ENUM('domain', 'all', 'hide')`. Kept as the raw string because
    /// Sphinx only *warns* about an unrecognised value and carries it
    /// through unchanged — see [`crate::config::BuildConfig::validate`].
    pub toc_object_entries_show_parents: String,
    /// `add_function_parentheses` (`config.py:248`), consumed by
    /// `XRefRole.update_title_and_target` for the `fix_parens` roles
    /// (`:py:func:`, `:py:meth:`) and by the `_toc_name` parens gate.
    pub add_function_parentheses: bool,
    /// `add_module_names` (`config.py:249`): whether a signature's module
    /// prefix is rendered in `desc_addname`.
    pub add_module_names: bool,
    /// `strip_signature_backslash` (`directives/__init__.py:370-372`):
    /// strip backslashes from a signature before it is measured and parsed.
    pub strip_signature_backslash: bool,
}

impl Default for PySigConfig {
    /// Sphinx 9.1.0's own defaults, probe-verified (task-2 brief, "Probe
    /// outcomes"), so a parse with no project behind it behaves like a
    /// default Sphinx project.
    fn default() -> Self {
        Self {
            maximum_signature_line_length: None,
            python_maximum_signature_line_length: None,
            python_trailing_comma_in_multi_line_signatures: true,
            python_display_short_literal_types: false,
            python_use_unqualified_type_names: false,
            toc_object_entries: true,
            toc_object_entries_show_parents: "domain".to_string(),
            add_function_parentheses: true,
            add_module_names: true,
            strip_signature_backslash: false,
        }
    }
}

impl PySigConfig {
    /// The resolved wrap threshold `PyObject.handle_signature` computes
    /// (`domains/python/_object.py:291-295`):
    ///
    /// ```python
    /// max_len = (
    ///     self.config.python_maximum_signature_line_length
    ///     or self.config.maximum_signature_line_length
    ///     or 0
    /// )
    /// ```
    ///
    /// `or` tests **truthiness**, not `None`-ness. A py-specific `0` is
    /// therefore *not* "wrap everything": it is falsy and falls through to
    /// the global key (research spec §1.2, probe C4 — python=0, global=1
    /// wraps at 1). The `> max_len > 0` guard at the call site is what makes
    /// a resolved 0 mean "feature off".
    pub fn max_len(&self) -> i64 {
        fn truthy(value: Option<i64>) -> Option<i64> {
            value.filter(|v| *v != 0)
        }
        truthy(self.python_maximum_signature_line_length)
            .or_else(|| truthy(self.maximum_signature_line_length))
            .unwrap_or(0)
    }
}

impl PySigConfig {
    /// Sets the key `key` of an oracle fixture's `conf` dict (JSON, the
    /// confoverrides its generator applied) on this configuration, when it
    /// is one of its keys: `Ok(true)` once set, `Ok(false)` for a key that
    /// is not, and an error naming the key for a value of the wrong shape
    /// — a non-integer line length included, never read as no limit.
    pub fn apply_conf_key(&mut self, key: &str, value: &serde_json::Value) -> Result<bool, String> {
        use serde_json::Value;
        let line_length = |value: &Value| match value {
            Value::Null => Ok(None),
            Value::Number(number) => number
                .as_i64()
                .map(Some)
                .ok_or_else(|| format!("conf key {key}: non-integer number {number}")),
            other => Err(format!("conf key {key}: expected integer, got {other}")),
        };
        let flag = |value: &Value| {
            value
                .as_bool()
                .ok_or_else(|| format!("conf key {key}: expected bool, got {value}"))
        };
        match key {
            "maximum_signature_line_length" => {
                self.maximum_signature_line_length = line_length(value)?;
            }
            "python_maximum_signature_line_length" => {
                self.python_maximum_signature_line_length = line_length(value)?;
            }
            "python_trailing_comma_in_multi_line_signatures" => {
                self.python_trailing_comma_in_multi_line_signatures = flag(value)?;
            }
            "python_display_short_literal_types" => {
                self.python_display_short_literal_types = flag(value)?;
            }
            "python_use_unqualified_type_names" => {
                self.python_use_unqualified_type_names = flag(value)?;
            }
            "toc_object_entries" => self.toc_object_entries = flag(value)?,
            "toc_object_entries_show_parents" => {
                self.toc_object_entries_show_parents = value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("conf key {key}: expected string, got {value}"))?;
            }
            "add_function_parentheses" => self.add_function_parentheses = flag(value)?,
            "add_module_names" => self.add_module_names = flag(value)?,
            "strip_signature_backslash" => self.strip_signature_backslash = flag(value)?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

impl From<&crate::config::BuildConfig> for PySigConfig {
    fn from(config: &crate::config::BuildConfig) -> Self {
        Self {
            maximum_signature_line_length: config.maximum_signature_line_length,
            python_maximum_signature_line_length: config.python_maximum_signature_line_length,
            python_trailing_comma_in_multi_line_signatures: config
                .python_trailing_comma_in_multi_line_signatures,
            python_display_short_literal_types: config.python_display_short_literal_types,
            python_use_unqualified_type_names: config.python_use_unqualified_type_names,
            toc_object_entries: config.toc_object_entries,
            toc_object_entries_show_parents: config.toc_object_entries_show_parents.clone(),
            add_function_parentheses: config.add_function_parentheses,
            add_module_names: config.add_module_names,
            strip_signature_backslash: config.strip_signature_backslash,
        }
    }
}
