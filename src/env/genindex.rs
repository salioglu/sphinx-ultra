//! The `index` domain and the general index it feeds: Sphinx's
//! `IndexDomain` (`domains/index.py`) collection half, and
//! `IndexEntries.create_index` (`environment/adapters/indexentries.py:54-197`)
//! — the assembly half that turns the collected 5-tuples into the grouped,
//! sorted structure `genindex.html` renders.
//!
//! See `docs/superpowers/plans/2026-08-31-m2-wave4-research-spec-sphinx-env-toctree-domains.md`
//! §6 for the attribute-by-attribute mapping this port is drawn from.
//!
//! **What feeds it.** Everything here reads `index` nodes out of a finished
//! doctree, so its fidelity is bounded by the parse layer's. Two gaps are
//! known and belong to that layer, not this one (neither is exercised by
//! the environment-oracle corpus):
//!
//! * The `:index:` role (`IndexRole`, `domains/index.py:97-117`) is not
//!   implemented, so `:index:`text`` parses as `problematic` and indexes
//!   nothing.
//! * `.. index::` with a `:name:` should anchor its entries on the id
//!   docutils' `note_explicit_target` gives the *named* target
//!   (`domains/index.py:78-83`), and consume no `index-N` serial. This
//!   crate's `:name:`-optioned targets carry a name but no id throughout
//!   the parse layer, so those entries anchor on `index-N` instead — and
//!   because the serial *is* consumed, every later `index-N` in the same
//!   document is shifted up by one as well, so the divergence is not
//!   confined to the named directive's own entries.
//!
//! **Unicode caveat.** The sort and grouping keys normalize to NFD and
//! lowercase exactly where Sphinx does, but two primitives are Rust's rather
//! than Python's:
//!
//! * `str::to_lowercase`/`char::to_uppercase` are full Unicode case
//!   mappings, like Python's `str.lower()`/`str.upper()` (both expand
//!   `U+0130` to `i` + combining dot, both special-case final sigma). No
//!   divergence is known for the corpus, and none is expected outside
//!   locale-sensitive mappings (Turkish dotless i), which neither applies.
//! * [`py_isalpha`] approximates Python's `str.isalpha()` (general category
//!   `L*`) with Rust's `char::is_alphabetic` (the `Alphabetic` derived
//!   property). The two differ for `Nl` (`Ⅷ`, `ↀ`) and for `Other_Alphabetic`
//!   combining marks — which NFD *produces* — so an entry starting with a
//!   Devanagari vowel sign, or a Roman-numeral character, would be grouped
//!   under itself here and under `Symbols` by Sphinx. Closing that needs a
//!   general-category table this crate does not carry.

use std::collections::HashMap;
use std::path::Path;

use log::warn;
use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::doctree::{AttrValue, Doctree, Node};
use crate::env::std_domain::node_line;
use crate::env::{BuildEnvironment, IndexEntryRecord};
use crate::error::{BuildWarning, WarningType};
use crate::rst::block::py_repr;

/// The `index` node kind, as the parse layer spells it.
const INDEX: &str = "index";

/// Sphinx's `\N{RIGHT-TO-LEFT MARK}`, stripped from every sort key.
const RTL_MARK: char = '\u{200f}';

/// Every warning this module raises carries `type='index'` and no subtype,
/// which `show_warning_types` renders as a bare `[index]` suffix.
const INDEX_CATEGORY: &str = "index";

// ---------------------------------------------------------------------------
// Collection: IndexDomain.process_doc
// ---------------------------------------------------------------------------

/// `IndexDomain.process_doc` (`domains/index.py:48-61`).
///
/// Harvests every `index` node's entries into `env.index_entries[docname]`,
/// in document order. A node carrying an entry `split_index_msg` rejects
/// warns *once* and is **removed from the doctree**, contributing none of
/// its entries — Sphinx wraps the whole per-node validation loop in one
/// `try`, so a single bad entry discards the node's good ones too.
///
/// The docname's (possibly empty) entry list is created unconditionally
/// (`self.entries.setdefault(...)`), which is why a document with no index
/// nodes at all still appears in `env.index_entries`.
pub fn process_doc(
    env: &mut BuildEnvironment,
    docname: &str,
    doctree: &mut Doctree,
    path: &Path,
    text: &str,
    warnings: &mut Vec<BuildWarning>,
) {
    let mut collected: Vec<IndexEntryRecord> = Vec::new();
    visit(
        &mut doctree.root,
        docname,
        &mut collected,
        &mut |message, node| {
            warnings.push(
                BuildWarning::new(
                    path.to_path_buf(),
                    Some(node_line(node, text)),
                    message,
                    WarningType::Other,
                )
                .with_category(Some(INDEX_CATEGORY.to_string())),
            );
        },
    );
    env.index_entries
        .entry(docname.to_string())
        .or_default()
        .extend(collected);
}

/// Pre-order walk mirroring `document.findall(addnodes.index)`, with the
/// `node.parent.remove(node)` an invalid entry triggers applied in place.
fn visit<F: FnMut(String, &Node)>(
    node: &mut Node,
    docname: &str,
    out: &mut Vec<IndexEntryRecord>,
    warn: &mut F,
) {
    let mut i = 0;
    while i < node.children.len() {
        if node.children[i].kind == INDEX {
            let entries = index_node_entries(&node.children[i], docname);
            match entries
                .iter()
                .try_for_each(|entry| split_index_msg(&entry.entry_type, &entry.value).map(|_| ()))
            {
                Ok(()) => out.extend(entries),
                Err(message) => {
                    warn(message, &node.children[i]);
                    node.children.remove(i);
                    continue;
                }
            }
        } else {
            visit(&mut node.children[i], docname, out, warn);
        }
        i += 1;
    }
}

/// One `index` node's `entries` attribute, parsed back into records.
///
/// The three ways this can yield nothing are *not* the same, so they do not
/// share an arm: an empty list is ordinary (an object description that
/// indexes nothing writes one), an absent attribute loses no data because
/// there was none, and a non-list attribute means entries that exist are
/// being thrown away — the one case worth a log line.
fn index_node_entries(node: &Node, docname: &str) -> Vec<IndexEntryRecord> {
    match node.get("entries") {
        Some(AttrValue::List(items)) => parse_index_entries(items, docname),
        // Every producer writes a list (see
        // [`crate::rst::block::index_entry_tuple`]), so a scalar here is a
        // doctree cached before that was true. The harvest reads it as "no
        // entries" and this document silently drops out of the general
        // index; say so once rather than leaving it undiagnosable.
        Some(_) => {
            warn!(
                "{docname}: an `index` node's `entries` is not a list attribute, so its \
                 index entries were dropped. This is a doctree cached before the \
                 list-attribute format; delete the cache directory to re-read it."
            );
            Vec::new()
        }
        None => Vec::new(),
    }
}

/// The exact inverse of [`crate::rst::block::index_entry_tuple`]: each item
/// of an `index` node's `entries` list attribute is the Python `str()` of a
/// `(entry_type, value, target_id, main, category_key)` tuple.
///
/// An item that does not parse is dropped rather than panicking: only this
/// crate writes these strings, so a malformed one is a bug here, not
/// attacker-controlled input, and losing one index entry beats failing the
/// build. It is logged for the same reason the non-list attribute above is:
/// a dropped entry is invisible in the finished index.
pub(crate) fn parse_index_entries(items: &[String], docname: &str) -> Vec<IndexEntryRecord> {
    items
        .iter()
        .filter_map(|item| {
            let parsed = parse_tuple(item);
            if parsed.is_none() {
                warn!("{docname}: unparsable `index` entry {item:?} was dropped");
            }
            parsed
        })
        .collect()
}

/// `('single', 'Alpha', 'index-0', '', None)` -> one record.
fn parse_tuple(item: &str) -> Option<IndexEntryRecord> {
    let chars: Vec<char> = item.chars().collect();
    let mut at = 0usize;
    expect(&chars, &mut at, '(')?;
    let mut fields: Vec<Option<String>> = Vec::with_capacity(5);
    for field in 0..5 {
        fields.push(parse_literal(&chars, &mut at)?);
        if field < 4 {
            expect(&chars, &mut at, ',')?;
            while chars.get(at) == Some(&' ') {
                at += 1;
            }
        }
    }
    expect(&chars, &mut at, ')')?;
    if at != chars.len() {
        return None;
    }
    let main = fields[3].clone()?;
    Some(IndexEntryRecord {
        entry_type: fields[0].clone()?,
        value: fields[1].clone()?,
        target_id: fields[2].clone()?,
        // Sphinx stores the literal marker string; only `'main'` and `''`
        // are ever produced, and everything downstream tests it for truth.
        main: !main.is_empty(),
        category_key: fields[4].clone(),
    })
}

fn expect(chars: &[char], at: &mut usize, want: char) -> Option<()> {
    if chars.get(*at) == Some(&want) {
        *at += 1;
        Some(())
    } else {
        None
    }
}

/// `None`, or a `'`/`"`-quoted string with Python's repr escapes.
fn parse_literal(chars: &[char], at: &mut usize) -> Option<Option<String>> {
    if chars[*at..].starts_with(&['N', 'o', 'n', 'e']) {
        *at += 4;
        return Some(None);
    }
    let quote = *chars.get(*at)?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    *at += 1;
    let mut out = String::new();
    loop {
        let c = *chars.get(*at)?;
        *at += 1;
        match c {
            _ if c == quote => return Some(Some(out)),
            '\\' => {
                let escaped = *chars.get(*at)?;
                *at += 1;
                out.push(match escaped {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
            }
            other => out.push(other),
        }
    }
}

/// `split_index_msg` (`util/index_entries.py:4-18`). `Err` carries the
/// `ValueError` text verbatim, which is what Sphinx logs.
fn split_index_msg(entry_type: &str, value: &str) -> Result<Vec<String>, String> {
    match entry_type {
        "single" => split_into(2, "single", value).or_else(|_| split_into(1, "single", value)),
        "pair" => split_into(2, "pair", value),
        "triple" => split_into(3, "triple", value),
        "see" | "seealso" => split_into(2, "see", value),
        other => Err(invalid_entry(other, value)),
    }
}

/// `_split_into` (`util/index_entries.py:21-27`): split at the first `n-1`
/// semicolons, strip each part, and reject unless **every** part is
/// non-empty (`len(list(filter(None, parts))) < n`).
///
/// `trim` stands in for Python's `str.strip()`. The two agree except on the
/// C0 separators `U+001C`-`U+001F`, which Python's `str.isspace()` counts
/// as whitespace and Unicode's `White_Space` property does not — so a part
/// padded with one of those would strip in sphinx and not here.
fn split_into(n: usize, entry_type: &str, value: &str) -> Result<Vec<String>, String> {
    let parts: Vec<String> = value
        .splitn(n, ';')
        .map(|part| part.trim().to_string())
        .collect();
    if parts.iter().filter(|part| !part.is_empty()).count() < n {
        return Err(invalid_entry(entry_type, value));
    }
    Ok(parts)
}

fn invalid_entry(entry_type: &str, value: &str) -> String {
    format!("invalid {entry_type} index entry {}", py_repr(Some(value)))
}

// ---------------------------------------------------------------------------
// Assembly: IndexEntries.create_index
// ---------------------------------------------------------------------------

/// One `(main, uri)` target: `main` is Sphinx's literal `"main"`/`""`.
pub type IndexTarget = (String, String);

/// One letter (or category-key) heading of the general index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexGroup {
    pub group: String,
    pub entries: Vec<IndexEntry>,
}

/// One top-level index entry with its targets and sub-entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexEntry {
    pub name: String,
    pub targets: Vec<IndexTarget>,
    pub subitems: Vec<IndexSubItem>,
    pub category_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexSubItem {
    pub name: String,
    pub targets: Vec<IndexTarget>,
}

/// A diagnostic `create_index` raises, located at a whole document
/// (`location=docname`, which renders as the source path with no line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexMessage {
    pub docname: String,
    pub message: String,
}

impl IndexMessage {
    /// Render as the `[index]`-categorized build warning Sphinx logs.
    pub fn into_warning(self, path: &Path) -> BuildWarning {
        BuildWarning::new(path.to_path_buf(), None, self.message, WarningType::Other)
            .with_category(Some(INDEX_CATEGORY.to_string()))
    }
}

/// `IndexEntries.create_index(builder, group_entries=True)`
/// (`environment/adapters/indexentries.py:54-197`).
///
/// `rel_uri(docname)` is `builder.get_relative_uri('genindex', docname)`;
/// `None` is Sphinx's `NoUri`, which keeps the entry but drops its link.
pub fn create_index(
    env: &BuildEnvironment,
    rel_uri: &dyn Fn(&str) -> Option<String>,
    messages: &mut Vec<IndexMessage>,
) -> Vec<IndexGroup> {
    let mut new = Working::default();

    // Sphinx iterates `index_domain.entries` — a dict, so in the order the
    // documents were *read*. For a full build that is `sorted(docnames)`
    // (`Builder.read`), which is the order this `BTreeMap` gives.
    //
    // Deliberate divergence on incremental rebuilds: sphinx re-reads only
    // the outdated documents, and their entries move to the end of the
    // dict, so its genindex depends on which documents happened to be
    // stale. This map stays docname-sorted, so an incremental rebuild
    // produces the *same* index a cold build would — the invariant
    // `touching_one_document_re_reads_only_it_and_the_environment_still_matches_a_cold_build`
    // (tests/env_differential.rs) enforces. The only thing the order
    // decides is sub-entry ties, which
    // `sub_entries_that_fold_alike_keep_their_insertion_order` pins.
    for (docname, entries) in &env.index_entries {
        let rel = rel_uri(docname);
        for entry in entries {
            let uri = rel.as_ref().map(|rel| format!("{rel}#{}", entry.target_id));
            if let Err(message) = add(&mut new, entry, uri.as_deref()) {
                messages.push(IndexMessage {
                    docname: docname.clone(),
                    message,
                });
            }
        }
    }

    for bucket in &mut new.buckets {
        bucket.targets.sort_by_cached_key(target_sort_key);
        for (_, targets, _) in &mut bucket.sub_items {
            targets.sort_by_cached_key(target_sort_key);
        }
    }

    let mut new_list = new.buckets;
    new_list
        .sort_by_cached_key(|bucket| entry_sort_key(&bucket.key, bucket.category_key.as_deref()));

    group_entries(&mut new_list);

    let mut grouped: Vec<IndexGroup> = Vec::new();
    for bucket in new_list {
        let group = group_of(&bucket.key, bucket.category_key.as_deref());
        let mut subitems: Vec<IndexSubItem> = bucket
            .sub_items
            .into_iter()
            .map(|(name, targets, _)| IndexSubItem { name, targets })
            .collect();
        subitems.sort_by_cached_key(|item| sub_entry_sort_key(&item.name));
        let entry = IndexEntry {
            name: bucket.key,
            targets: bucket.targets,
            subitems,
            category_key: bucket.category_key,
        };
        match grouped.last_mut() {
            Some(last) if last.group == group => last.entries.push(entry),
            _ => grouped.push(IndexGroup {
                group,
                entries: vec![entry],
            }),
        }
    }
    grouped
}

/// `new`: an insertion-ordered `word -> (targets, sub_items, category_key)`
/// map. The order is load-bearing only for `sub_items`, whose sort is
/// stable and can tie (two sub-entries that lowercase to the same string).
#[derive(Default)]
struct Working {
    buckets: Vec<Bucket>,
    index: HashMap<String, usize>,
}

struct Bucket {
    key: String,
    targets: Vec<IndexTarget>,
    /// `(subword, targets, category_key)`, in insertion order.
    sub_items: Vec<(String, Vec<IndexTarget>, Option<String>)>,
    sub_index: HashMap<String, usize>,
    category_key: Option<String>,
}

impl Working {
    /// `_add_entry` (`:200-215`). Both `setdefault`s keep the category key
    /// of the *first* entry that created the word (or subword).
    fn add_entry(
        &mut self,
        word: &str,
        subword: &str,
        main: Option<&str>,
        link: Option<&str>,
        key: Option<&str>,
    ) {
        let at = match self.index.get(word) {
            Some(&at) => at,
            None => {
                let at = self.buckets.len();
                self.buckets.push(Bucket {
                    key: word.to_string(),
                    targets: Vec::new(),
                    sub_items: Vec::new(),
                    sub_index: HashMap::new(),
                    category_key: key.map(str::to_string),
                });
                self.index.insert(word.to_string(), at);
                at
            }
        };
        let bucket = &mut self.buckets[at];
        let targets = if subword.is_empty() {
            &mut bucket.targets
        } else {
            let sub_at = sub_item_slot(bucket, subword, key);
            &mut bucket.sub_items[sub_at].1
        };
        // `if link:` — an empty uri is falsy in Python, and so is `NoUri`'s
        // `False`, which is what a `see`/`seealso` sub-entry passes.
        if let Some(link) = link.filter(|link| !link.is_empty()) {
            targets.push((main.unwrap_or_default().to_string(), link.to_string()));
        }
    }
}

/// `entry[1].setdefault(subword, ([], key))`, as an index into
/// `bucket.sub_items`.
fn sub_item_slot(bucket: &mut Bucket, subword: &str, key: Option<&str>) -> usize {
    match bucket.sub_index.get(subword) {
        Some(&at) => at,
        None => {
            let at = bucket.sub_items.len();
            bucket
                .sub_items
                .push((subword.to_string(), Vec::new(), key.map(str::to_string)));
            bucket.sub_index.insert(subword.to_string(), at);
            at
        }
    }
}

/// One collected entry, split and added (`:72-146`).
fn add(new: &mut Working, entry: &IndexEntryRecord, uri: Option<&str>) -> Result<(), String> {
    let main = if entry.main { "main" } else { "" };
    let key = entry.category_key.as_deref();
    match entry.entry_type.as_str() {
        "single" => {
            let (word, subword) = match split_into(2, "single", &entry.value) {
                Ok(parts) => (parts[0].clone(), parts[1].clone()),
                Err(_) => (
                    split_into(1, "single", &entry.value)?.remove(0),
                    String::new(),
                ),
            };
            new.add_entry(&word, &subword, Some(main), uri, key);
        }
        "pair" => {
            let parts = split_into(2, "pair", &entry.value)?;
            new.add_entry(&parts[0], &parts[1], Some(main), uri, key);
            new.add_entry(&parts[1], &parts[0], Some(main), uri, key);
        }
        "triple" => {
            let parts = split_into(3, "triple", &entry.value)?;
            let (first, second, third) = (&parts[0], &parts[1], &parts[2]);
            new.add_entry(first, &format!("{second} {third}"), Some(main), uri, key);
            new.add_entry(second, &format!("{third}, {first}"), Some(main), uri, key);
            new.add_entry(third, &format!("{first} {second}"), Some(main), uri, key);
        }
        // Both `see` and `seealso` split with the type name `'see'`, and
        // both add a *linkless* sub-entry (`link=False`, `main=None`).
        "see" => {
            let parts = split_into(2, "see", &entry.value)?;
            new.add_entry(&parts[0], &format!("see {}", parts[1]), None, None, key);
        }
        "seealso" => {
            let parts = split_into(2, "see", &entry.value)?;
            new.add_entry(
                &parts[0],
                &format!("see also {}", parts[1]),
                None,
                None,
                key,
            );
        }
        other => return Err(format!("unknown index entry type {}", py_repr(Some(other)))),
    }
    Ok(())
}

/// `_key_func_0` (`:218-221`): main entries first, then by uri.
fn target_sort_key(target: &IndexTarget) -> (bool, String) {
    (target.0.is_empty(), target.1.clone())
}

/// `_key_func_1` (`:224-241`): `((group, lc_key), raw_key)`, where a
/// *truthy* category key replaces the entry name before folding.
fn entry_sort_key(key: &str, category_key: Option<&str>) -> (u8, String, String) {
    let folded = match category_key {
        Some(category) if !category.is_empty() => category,
        _ => key,
    };
    let lc_key = fold(folded);
    let first = lc_key.chars().next();
    let group = if !first.is_some_and(py_isalpha) && !lc_key.starts_with('_') {
        0 // symbols come first
    } else {
        1
    };
    (group, lc_key, key.to_string())
}

/// `_key_func_2` (`:244-250`): sub-entries sort by the same folded key,
/// with alphabetic (and `_`) keys pushed behind the symbols by a `chr(127)`
/// prefix.
fn sub_entry_sort_key(name: &str) -> String {
    let key = fold(name);
    if key.chars().next().is_some_and(py_isalpha) || key.starts_with('_') {
        format!("\u{7f}{key}")
    } else {
        key
    }
}

/// `_group_by_func` (`:253-267`). Note the asymmetry with
/// [`entry_sort_key`]: grouping honours a category key that is merely *not
/// None*, sorting only a truthy one.
fn group_of(key: &str, category_key: Option<&str>) -> String {
    if let Some(category) = category_key {
        return category.to_string();
    }
    let key = key.strip_prefix(RTL_MARK).unwrap_or(key);
    // `key[0]` — Sphinx indexes blindly; an entry name is never empty (every
    // `_split_into` part is non-empty), but a name that is nothing but the
    // RTL mark would be after the strip above.
    let Some(first) = key.chars().next() else {
        return "Symbols".to_string();
    };
    let Some(decomposed) = first.nfd().next() else {
        return "Symbols".to_string();
    };
    let letter: String = decomposed.to_uppercase().collect();
    if (!letter.is_empty() && letter.chars().all(py_isalpha)) || letter == "_" {
        letter
    } else {
        "Symbols".to_string()
    }
}

/// `unicodedata.normalize('NFD', key.lower()).removeprefix('‏')` —
/// lowercase *first*, then decompose, then drop a leading RTL mark.
fn fold(key: &str) -> String {
    let lowered: String = key.to_lowercase().nfd().collect();
    match lowered.strip_prefix(RTL_MARK) {
        Some(rest) => rest.to_string(),
        None => lowered,
    }
}

/// Python's `str.isalpha()` for one character — see the module docs for the
/// `Nl`/`Other_Alphabetic` divergence this approximation carries.
fn py_isalpha(c: char) -> bool {
    c.is_alphabetic()
}

/// The `group_entries` fixup (`:155-183`): consecutive entries that share a
/// `name (parenthesized)` prefix collapse, the later ones becoming
/// sub-entries of the first under their parenthesized part.
fn group_entries(new_list: &mut Vec<Bucket>) {
    let mut old_key = String::new();
    // The index of the bucket whose `sub_items` is Sphinx's `old_sub_items`.
    // `None` is its initial value, a scratch dict owned by nothing: writes
    // to it are discarded, exactly as they are in Sphinx.
    let mut old_index: Option<usize> = None;
    let mut i = 0;
    while i < new_list.len() {
        // "cannot move if it has sub_items; structure gets too complex"
        if new_list[i].sub_items.is_empty() {
            match fixre_match(&new_list[i].key) {
                Some((prefix, parenthesized)) => {
                    if old_key == prefix {
                        let moved = new_list.remove(i);
                        if let Some(old) = old_index {
                            let bucket = &mut new_list[old];
                            let at = sub_item_slot(
                                bucket,
                                &parenthesized,
                                moved.category_key.as_deref(),
                            );
                            bucket.sub_items[at].1.extend(moved.targets);
                        }
                        continue;
                    }
                    old_key = prefix;
                }
                None => old_key = new_list[i].key.clone(),
            }
        }
        old_index = Some(i);
        i += 1;
    }
}

/// `re.match(r'(.*) ([(][^()]*[)])', key)` — anchored at the start, with a
/// greedy first group, and with `.` not crossing a newline.
fn fixre_match(key: &str) -> Option<(String, String)> {
    // `(.*)` cannot cross a newline, so neither the prefix nor the ` (`
    // separator can live past the first one.
    let limit = key.find('\n').unwrap_or(key.len());
    let bytes = key.as_bytes();
    // Greedy: the rightmost viable split wins.
    for open in (0..limit.saturating_sub(1)).rev() {
        if bytes[open] != b' ' || bytes[open + 1] != b'(' {
            continue;
        }
        let rest = &key[open + 2..];
        let Some(close) = rest.find(['(', ')']) else {
            continue;
        };
        if rest.as_bytes()[close] != b')' {
            continue;
        }
        return Some((
            key[..open].to_string(),
            key[open + 1..open + 3 + close].to_string(),
        ));
    }
    None
}

/// The finished index as JSON, in the oracle fixture's `genindex` shape.
/// Infallible: every field of these structs is a plain string, option or
/// vector.
pub fn snapshot(groups: &[IndexGroup]) -> serde_json::Value {
    serde_json::to_value(groups).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rst::block::index_entry_tuple;

    fn record(
        entry_type: &str,
        value: &str,
        target_id: &str,
        main: bool,
        category_key: Option<&str>,
    ) -> IndexEntryRecord {
        IndexEntryRecord {
            entry_type: entry_type.to_string(),
            value: value.to_string(),
            target_id: target_id.to_string(),
            main,
            category_key: category_key.map(str::to_string),
        }
    }

    fn index_of(entries: &[(&str, Vec<IndexEntryRecord>)]) -> (Vec<IndexGroup>, Vec<IndexMessage>) {
        let mut env = BuildEnvironment::default();
        for (docname, records) in entries {
            env.index_entries
                .insert((*docname).to_string(), records.clone());
        }
        let mut messages = Vec::new();
        let groups = create_index(&env, &|_| Some(String::new()), &mut messages);
        (groups, messages)
    }

    /// The parse half must be the exact inverse of the render half, for
    /// every character class the render escapes: quotes, backslashes and
    /// the whitespace escapes.
    #[test]
    fn every_rendered_tuple_parses_back_to_the_entry_it_came_from() {
        let cases = [
            ("single", "Alpha", "index-0", "", None),
            ("pair", "bread; butter", "index-1", "main", None),
            ("single", "it's a term", "id", "", Some("K")),
            ("single", "say \"hi\"", "id", "", None),
            ("single", "both ' and \"", "id", "", None),
            ("single", "back\\slash", "id", "", None),
            ("single", "back\\ slash", "id", "", None),
            ("single", "tab\there", "id", "", None),
            ("single", "new\nline", "id", "", None),
            ("single", "carriage\rreturn", "id", "", None),
            ("single", "", "id", "", Some("")),
            ("single", "(None, 'not a tuple')", "id", "", None),
        ];
        for (entry_type, value, target_id, main, key) in cases {
            let rendered = index_entry_tuple(entry_type, value, target_id, main, key);
            let parsed = parse_index_entries(std::slice::from_ref(&rendered), "a");
            assert_eq!(
                parsed,
                vec![record(entry_type, value, target_id, !main.is_empty(), key)],
                "round trip failed for {rendered:?}"
            );
        }
    }

    /// A doctree cached before `entries` became a list attribute holds a
    /// `Str` there. The harvest must read it as "no entries" — never as one
    /// entry whose value is the whole blob — and must not manufacture a
    /// *build* warning for it: the diagnosis is a `log::warn!` line, which
    /// is deliberately outside the sphinx-fidelity warning stream the
    /// environment oracle diffs. (The log line itself is not asserted:
    /// capturing `log` output needs a process-global logger, which is racy
    /// under the parallel test harness.)
    #[test]
    fn a_stale_string_shaped_entries_attribute_harvests_nothing() {
        let mut index = Node::elem(INDEX, crate::doctree::Span::ZERO);
        index.set(
            "entries",
            AttrValue::Str("('single',\\ 'Alpha',\\ 'index-0',\\ '',\\ None)".to_string()),
        );
        let mut root = Node::elem(crate::doctree::kinds::DOCUMENT, crate::doctree::Span::ZERO);
        root.children.push(index);
        let mut doctree = Doctree {
            root,
            sources: vec!["<document>".to_string()],
        };

        let mut env = BuildEnvironment::default();
        let mut warnings = Vec::new();
        process_doc(
            &mut env,
            "a",
            &mut doctree,
            Path::new("a.rst"),
            "",
            &mut warnings,
        );

        assert_eq!(env.index_entries["a"], Vec::new());
        assert!(warnings.is_empty(), "{warnings:?}");
        // The node itself stays: nothing was *rejected*, only unreadable.
        assert_eq!(doctree.root.children.len(), 1);
    }

    #[test]
    fn a_malformed_entries_item_is_dropped_not_panicked_on() {
        assert!(parse_index_entries(&["('single', 'x'".to_string()], "a").is_empty());
        assert!(
            parse_index_entries(&["('a', 'b', 'c', 'd', 'e', 'f')".to_string()], "a").is_empty()
        );
        assert!(parse_index_entries(&[String::new()], "a").is_empty());
    }

    /// `_split_into` rejects a value with too few parts *and* one with an
    /// empty part, and the message is the one Sphinx logs.
    #[test]
    fn split_index_msg_texts_match_sphinx() {
        assert_eq!(
            split_index_msg("pair", "lonely"),
            Err("invalid pair index entry 'lonely'".to_string())
        );
        assert_eq!(
            split_index_msg("pair", "a; "),
            Err("invalid pair index entry 'a; '".to_string())
        );
        assert_eq!(
            split_index_msg("triple", "a; b"),
            Err("invalid triple index entry 'a; b'".to_string())
        );
        // `see`/`seealso` both report the type name `see`.
        assert_eq!(
            split_index_msg("seealso", "lonely"),
            Err("invalid see index entry 'lonely'".to_string())
        );
        assert_eq!(
            split_index_msg("bogus", "x"),
            Err("invalid bogus index entry 'x'".to_string())
        );
        // A `single` entry falls back to the one-part split.
        assert_eq!(
            split_index_msg("single", "Alpha"),
            Ok(vec!["Alpha".to_string()])
        );
        assert_eq!(
            split_index_msg("single", "Alpha; Beta"),
            Ok(vec!["Alpha".to_string(), "Beta".to_string()])
        );
        // Only the first `n-1` semicolons split: the rest ride the last part.
        assert_eq!(
            split_index_msg("pair", "a; b; c"),
            Ok(vec!["a".to_string(), "b; c".to_string()])
        );
    }

    /// The oracle corpus never produces an `unknown index entry type`, so
    /// its text is pinned here instead. Verified against sphinx 9.1.0's
    /// `__('unknown index entry type %r')` with `entry_type='bogus'`.
    #[test]
    fn an_unknown_entry_type_warns_with_the_sphinx_text() {
        let (groups, messages) = index_of(&[("a", vec![record("bogus", "x", "id", false, None)])]);
        assert!(groups.is_empty());
        assert_eq!(
            messages,
            vec![IndexMessage {
                docname: "a".to_string(),
                message: "unknown index entry type 'bogus'".to_string(),
            }]
        );
    }

    /// A `create_index` `ValueError` is logged with `str(err)` — the same
    /// text `split_index_msg` raises, which is what the collection half
    /// warns with too. (Reaching it needs an entry that passed collection
    /// and fails here: only `see`/`seealso` differ, and they do not — so
    /// this is exercised through a directly-seeded environment.)
    #[test]
    fn an_invalid_value_warns_with_the_value_error_text() {
        let (_, messages) = index_of(&[("a", vec![record("pair", "lonely", "id", false, None)])]);
        assert_eq!(
            messages,
            vec![IndexMessage {
                docname: "a".to_string(),
                message: "invalid pair index entry 'lonely'".to_string(),
            }]
        );
    }

    /// `NoUri` keeps the entry and drops its link.
    #[test]
    fn a_document_with_no_uri_contributes_a_linkless_entry() {
        let mut env = BuildEnvironment::default();
        env.index_entries.insert(
            "a".to_string(),
            vec![record("single", "Alpha", "id", false, None)],
        );
        let groups = create_index(&env, &|_| None, &mut Vec::new());
        assert_eq!(groups.len(), 1);
        assert!(groups[0].entries[0].targets.is_empty());
    }

    /// Main entries sort ahead of the rest, then by uri.
    #[test]
    fn main_targets_come_first() {
        let (groups, _) = index_of(&[(
            "a",
            vec![
                record("single", "Alpha", "z", false, None),
                record("single", "Alpha", "b", true, None),
                record("single", "Alpha", "a", false, None),
            ],
        )]);
        assert_eq!(
            groups[0].entries[0].targets,
            vec![
                ("main".to_string(), "#b".to_string()),
                (String::new(), "#a".to_string()),
                (String::new(), "#z".to_string()),
            ]
        );
    }

    /// The `_fixre` fixup: `func() (in module foo)` / `(in module bar)`
    /// collapse under one `func()` entry.
    #[test]
    fn consecutive_parenthesized_entries_collapse_into_subitems() {
        let (groups, _) = index_of(&[(
            "a",
            vec![
                record("single", "func() (in module foo)", "f1", false, None),
                record("single", "func() (in module bar)", "f2", false, None),
            ],
        )]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].entries.len(), 1);
        let entry = &groups[0].entries[0];
        // The *first* of the two survives as the entry; the second becomes
        // a sub-entry. Sorted, `bar` precedes `foo`, so the survivor here is
        // `(in module bar)` and `(in module foo)` moves under it.
        assert_eq!(entry.name, "func() (in module bar)");
        assert_eq!(
            entry
                .subitems
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["(in module foo)"]
        );
    }

    /// An entry that already has sub-entries is never moved, and does not
    /// become the prefix a later entry can attach to.
    #[test]
    fn an_entry_with_subitems_is_never_collapsed() {
        let (groups, _) = index_of(&[(
            "a",
            vec![
                record("pair", "func() (in module foo); detail", "f1", false, None),
                record("single", "func() (in module goo)", "f2", false, None),
            ],
        )]);
        let names: Vec<&str> = groups
            .iter()
            .flat_map(|group| group.entries.iter().map(|entry| entry.name.as_str()))
            .collect();
        assert!(
            names.contains(&"func() (in module foo)") && names.contains(&"func() (in module goo)"),
            "{names:?}"
        );
    }

    /// Sub-entries are sorted by a *folded* key with a stable sort, so two
    /// that fold alike keep the order they were added in. That is the one
    /// place [`Working`]'s insertion order is load-bearing: swapping
    /// `sub_items` for a `BTreeMap` would silently reorder this pair.
    #[test]
    fn sub_entries_that_fold_alike_keep_their_insertion_order() {
        let (groups, _) = index_of(&[(
            "a",
            vec![
                record("pair", "word; Beta", "b1", false, None),
                record("pair", "word; beta", "b2", false, None),
            ],
        )]);
        let word = groups
            .iter()
            .flat_map(|group| &group.entries)
            .find(|entry| entry.name == "word")
            .expect("the `word` entry");
        assert_eq!(
            word.subitems
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Beta", "beta"]
        );
    }

    #[test]
    fn fixre_takes_the_rightmost_parenthesized_group() {
        assert_eq!(
            fixre_match("a (b) (c)"),
            Some(("a (b)".to_string(), "(c)".to_string()))
        );
        assert_eq!(
            fixre_match("a (b) c"),
            Some(("a".to_string(), "(b)".to_string()))
        );
        assert_eq!(fixre_match("plain"), None);
        // `[^()]*` cannot span a nested paren, so the *inner* group wins.
        assert_eq!(
            fixre_match("nested (a (b))"),
            Some(("nested (a".to_string(), "(b)".to_string()))
        );
        assert_eq!(
            fixre_match("func() (in module foo)"),
            Some(("func()".to_string(), "(in module foo)".to_string()))
        );
        // `.` never crosses a newline, so a match must live on line one.
        assert_eq!(fixre_match("a\nb (c)"), None);
    }

    /// A category key overrides the group heading (and, when non-empty, the
    /// sort key) — the glossary `:sorted:`/classifier path.
    #[test]
    fn a_category_key_overrides_the_group_and_the_sort_key() {
        let (groups, _) = index_of(&[(
            "a",
            vec![
                record("single", "zebra", "z", false, Some("A")),
                record("single", "apple", "a", false, None),
            ],
        )]);
        // One heading: `groupby` merges the consecutive `A` keys, whether
        // they came from a category key or from the entry's own initial.
        assert_eq!(
            groups
                .iter()
                .map(|group| group.group.as_str())
                .collect::<Vec<_>>(),
            vec!["A"]
        );
        // `zebra` sorts under its category key `A`, ahead of `apple`.
        assert_eq!(groups[0].entries[0].name, "zebra");
        assert_eq!(groups[0].entries[1].name, "apple");
    }

    /// Symbols group first, `_` is its own group, and a letter group is the
    /// uppercased first NFD character.
    #[test]
    fn grouping_follows_the_first_decomposed_character() {
        assert_eq!(group_of("42answer", None), "Symbols");
        assert_eq!(group_of("_private", None), "_");
        assert_eq!(group_of("alpha", None), "A");
        assert_eq!(group_of("Ábc", None), "A");
        assert_eq!(group_of("--flag", None), "Symbols");
        assert_eq!(group_of("x", Some("")), "");
    }
}
