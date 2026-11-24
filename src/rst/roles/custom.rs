//! The custom roles of docutils' `role` directive
//! (`SP/docutils/parsers/rst/directives/misc.py:472-531`): `CustomRole`,
//! the options it passes its base role, and the directive's own pieces —
//! the argument it reads its names from, and the options the base role
//! admits ([RC §3.15]).

use super::RoleFn;

/// A role function's options as docutils' `normalize_options` hands them
/// on (`SP/docutils/parsers/rst/roles.py`): the `class` option renamed
/// `classes`. Only the options some role reads are kept: `classes`, Sphinx's
/// and docutils' `code`'s `language` and `raw`'s `format`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RoleOptions {
    /// `classes`, from a `:class:` option or the custom role's own name.
    pub(crate) classes: Option<Vec<String>>,
    /// `code`'s `:language:`.
    pub(crate) language: Option<String>,
    /// `raw`'s `:format:`.
    pub(crate) format: Option<String>,
}

impl RoleOptions {
    /// `opts = normalize_options(self.supplied_options);
    /// opts.update(options)` (`CustomRole.__call__`): these options, each
    /// one `call` sets replaced by `call`'s.
    pub(crate) fn updated_with(&self, call: &RoleOptions) -> RoleOptions {
        RoleOptions {
            classes: call.classes.clone().or_else(|| self.classes.clone()),
            language: call.language.clone().or_else(|| self.language.clone()),
            format: call.format.clone().or_else(|| self.format.clone()),
        }
    }
}

/// docutils `CustomRole` (`SP/docutils/parsers/rst/roles.py:222-243`): the
/// role `.. role:: name(base)` registers. Calling it calls the base role
/// with the name the document wrote and the custom role's options, updated
/// with any the call passes ([`RoleOptions::updated_with`]); the content
/// it would pass along is always empty, since no base role admits any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CustomRole {
    /// The new role's name, as the directive wrote it.
    pub(crate) name: String,
    /// The role it calls.
    pub(crate) base: RoleFn,
    /// `supplied_options`: the directive's options, `class` defaulting to
    /// the new role's name ([`class_option`]).
    pub(crate) options: RoleOptions,
}

/// The options a role admits as the base of `.. role::`: the
/// `option_spec` the directive parses its option block against
/// (`convert_directive_function`, `getattr(base_role, 'options', None)`),
/// with no content ever admitted (no role function has a true
/// `content`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RoleOptionSpec {
    /// No `options`: a domain role's adapter and the intersphinx role.
    /// Any option line is content, and `no content permitted`.
    None,
    /// `{'class': class_option}`, which `set_implicit_options` gives every
    /// role `register_*_role` registers.
    Class,
    /// `code` (docutils' and Sphinx's): `class` and `language`.
    ClassLanguage,
    /// `raw`: `format`, and the implicit `class`.
    FormatClass,
}

impl RoleOptionSpec {
    /// The option spec of `role` as the base of a `.. role::` directive.
    ///
    /// A Sphinx role instance's `options` attribute is the dict its last
    /// call stored (`SphinxRole.__call__` sets `self.options`), so in a
    /// running Sphinx a role already used earlier in the process admits no
    /// option. This crate takes the state of a role not used yet, the one
    /// `register_local_role` leaves (`{'class': ...}`), for every
    /// document: the spec does not depend on what earlier documents
    /// called.
    pub(crate) fn of(role: &RoleFn) -> RoleOptionSpec {
        match role {
            RoleFn::Domain(..) | RoleFn::External => RoleOptionSpec::None,
            RoleFn::DocutilsCode | RoleFn::SphinxCode => RoleOptionSpec::ClassLanguage,
            RoleFn::Raw => RoleOptionSpec::FormatClass,
            // `CustomRole.options` is the base role's dict, or a new one
            // `set_implicit_options` gives `class` when the base has none.
            RoleFn::Custom(custom) => match RoleOptionSpec::of(&custom.base) {
                RoleOptionSpec::None => RoleOptionSpec::Class,
                spec => spec,
            },
            _ => RoleOptionSpec::Class,
        }
    }
}

/// `Role.argument_pattern` (`misc.py:476-477`),
/// `(simplename)\s*(\(\s*(simplename)\s*\)\s*)?$`, matched at the start of
/// the directive's first content line: the new role's name and its base
/// role's, if any.
pub(crate) fn role_arguments(line: &str) -> Option<(String, Option<String>)> {
    let chars: Vec<char> = line.chars().collect();
    let skip_space = |mut at: usize| {
        while at < chars.len() && crate::utils::py_isspace(chars[at]) {
            at += 1;
        }
        at
    };
    let name_len = crate::rst::inline::match_simplename(&chars, 0)?;
    let name: String = chars[..name_len].iter().collect();
    let mut at = skip_space(name_len);
    if at == chars.len() {
        return Some((name, None));
    }
    if chars[at] != '(' {
        return None;
    }
    at = skip_space(at + 1);
    let base_len = crate::rst::inline::match_simplename(&chars, at)?;
    let base: String = chars[at..at + base_len].iter().collect();
    at = skip_space(at + base_len);
    if chars.get(at) != Some(&')') {
        return None;
    }
    at = skip_space(at + 1);
    (at == chars.len()).then_some((name, Some(base)))
}

/// docutils `directives.class_option` (`directives/__init__.py:260-275`):
/// each whitespace-separated name made an id (`make_id`), or the name that
/// cannot be made one.
pub(crate) fn class_option(argument: &str) -> Result<Vec<String>, String> {
    crate::utils::py_split(argument)
        .map(|name| {
            let class = crate::doctree::ids::make_id(name);
            if class.is_empty() {
                Err(format!("cannot make \"{name}\" into a class name"))
            } else {
                Ok(class)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Role.argument_pattern`: a simplename, optionally followed by a
    /// parenthesized simplename, with whitespace allowed around the
    /// parentheses, and nothing after.
    #[test]
    fn role_arguments_match_docutils_argument_pattern() {
        let some =
            |name: &str, base: Option<&str>| Some((name.to_string(), base.map(str::to_string)));
        assert_eq!(role_arguments("red"), some("red", None));
        assert_eq!(role_arguments("k(kbd)"), some("k", Some("kbd")));
        assert_eq!(role_arguments("f ( py:func ) "), some("f", Some("py:func")));
        assert_eq!(
            role_arguments("raw-html(raw)"),
            some("raw-html", Some("raw"))
        );
        assert_eq!(role_arguments("a.b+c"), some("a.b+c", None));
        assert_eq!(role_arguments("k(kbd) x"), None);
        assert_eq!(role_arguments("k(kbd"), None);
        assert_eq!(role_arguments("(kbd)"), None);
        assert_eq!(role_arguments("_x"), None);
        assert_eq!(role_arguments("x y"), None);
    }

    /// The options a call passes win over the custom role's own; the rest
    /// stay ([`RoleOptions::updated_with`]).
    #[test]
    fn call_options_update_the_supplied_ones() {
        let supplied = RoleOptions {
            classes: Some(vec!["command".to_string()]),
            language: Some("python".to_string()),
            format: None,
        };
        let call = RoleOptions {
            classes: Some(vec!["extra".to_string()]),
            ..RoleOptions::default()
        };
        assert_eq!(
            supplied.updated_with(&call),
            RoleOptions {
                classes: Some(vec!["extra".to_string()]),
                language: Some("python".to_string()),
                format: None,
            }
        );
        assert_eq!(supplied.updated_with(&RoleOptions::default()), supplied);
    }

    #[test]
    fn class_option_makes_ids() {
        assert_eq!(
            class_option("Big  x_y"),
            Ok(vec!["big".to_string(), "x-y".to_string()])
        );
        assert_eq!(
            class_option("_"),
            Err("cannot make \"_\" into a class name".to_string())
        );
    }
}
