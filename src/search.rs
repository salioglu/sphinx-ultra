use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::LazyLock;

use crate::doctree::{kinds, AttrValue, Doctree, Node};
use crate::env::objects::{object_type_name, std_search_priority};
use crate::env::BuildEnvironment;

/// The environment-version map emitted by Sphinx 9.1.0's built-in domains.
/// Ultra currently implements the `std` and `py` data used below, but keeping
/// the complete built-in map makes the JavaScript contract match Sphinx's
/// searchtools loader and gives consumers the same compatibility signal.
pub fn sphinx_91_envversion() -> BTreeMap<String, u32> {
    BTreeMap::from([
        ("sphinx".to_string(), 66),
        ("sphinx.domains.c".to_string(), 3),
        ("sphinx.domains.changeset".to_string(), 1),
        ("sphinx.domains.citation".to_string(), 1),
        ("sphinx.domains.cpp".to_string(), 9),
        ("sphinx.domains.index".to_string(), 1),
        ("sphinx.domains.javascript".to_string(), 3),
        ("sphinx.domains.math".to_string(), 2),
        ("sphinx.domains.python".to_string(), 4),
        ("sphinx.domains.rst".to_string(), 2),
        ("sphinx.domains.std".to_string(), 2),
    ])
}

/// Search index that mirrors Sphinx's search functionality
#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    pub docnames: Vec<String>,
    pub filenames: Vec<String>,
    pub titles: Vec<String>,
    pub terms: HashMap<String, Vec<DocumentMatch>>,
    pub objects: HashMap<String, ObjectReference>,
    pub objnames: HashMap<String, String>,
    pub objtypes: HashMap<String, String>,
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMatch {
    pub docname_idx: usize,
    pub title_score: f32,
    pub content_score: f32,
    pub positions: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectReference {
    pub docname_idx: usize,
    pub anchor: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub obj_type: String,
}

impl SearchIndex {
    pub fn new(language: String) -> Self {
        Self {
            language,
            ..Default::default()
        }
    }

    /// Add a document to the search index
    pub fn add_document(
        &mut self,
        docname: String,
        filename: String,
        title: String,
        content: &str,
    ) -> Result<()> {
        let docname_idx = self.docnames.len();
        self.docnames.push(docname);
        self.filenames.push(filename);
        self.titles.push(title);

        // Extract and index terms from content
        self.index_content(docname_idx, content)?;

        Ok(())
    }

    /// Add an object to the search index
    pub fn add_object(
        &mut self,
        name: String,
        docname: &str,
        anchor: Option<String>,
        obj_type: &str,
        description: Option<String>,
    ) -> Result<()> {
        let docname_idx = self
            .docnames
            .iter()
            .position(|d| d == docname)
            .unwrap_or_else(|| {
                self.docnames.push(docname.to_string());
                self.docnames.len() - 1
            });

        let object_ref = ObjectReference {
            docname_idx,
            anchor,
            name: name.clone(),
            description,
            obj_type: obj_type.to_string(),
        };

        self.objects.insert(name, object_ref);
        self.objtypes
            .insert(obj_type.to_string(), obj_type.to_string());

        Ok(())
    }

    /// Index content for full-text search
    fn index_content(&mut self, docname_idx: usize, content: &str) -> Result<()> {
        let words = self.extract_words(content);

        for (normalized_word, positions) in words {
            let doc_match = DocumentMatch {
                docname_idx,
                title_score: 0.0,
                content_score: positions.len() as f32,
                positions,
            };

            self.terms
                .entry(normalized_word)
                .or_default()
                .push(doc_match);
        }

        Ok(())
    }

    /// Extract Sphinx search words and their normalized terms from content.
    fn extract_words(&self, content: &str) -> HashMap<String, Vec<usize>> {
        let mut words = HashMap::new();

        for (position, word) in search_words(content).into_iter().enumerate() {
            if let Some(term) = self.term_for_word(&word) {
                words.entry(term).or_insert_with(Vec::new).push(position);
            }
        }

        words
    }

    /// `IndexBuilder.feed`'s term for one word under this index's language.
    fn term_for_word(&self, word: &str) -> Option<String> {
        match self.language.as_str() {
            "en" => search_term(word),
            // Every other language Ultra builds is Sphinx's base
            // `SearchLanguage`: no stemming and no stopwords, lowercased by
            // `feed`, digit-only words dropped by `word_filter`.
            _ => index_term(word, word.to_lowercase(), |term| !is_digit_word(term)),
        }
    }

    /// Search for documents matching a query
    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        let query_terms: Vec<String> = search_words(query)
            .into_iter()
            .filter_map(|word| self.term_for_word(&word))
            .collect();

        if query_terms.is_empty() {
            return Vec::new();
        }

        let mut doc_scores: HashMap<usize, f32> = HashMap::new();

        // Calculate scores for each document
        for term in &query_terms {
            if let Some(matches) = self.terms.get(term) {
                for doc_match in matches {
                    let score = doc_match.title_score * 5.0 + doc_match.content_score;
                    *doc_scores.entry(doc_match.docname_idx).or_insert(0.0) += score;
                }
            }
        }

        // Convert to search results and sort by score
        let mut results: Vec<SearchResult> = doc_scores
            .into_iter()
            .map(|(docname_idx, score)| SearchResult {
                docname: self.docnames[docname_idx].clone(),
                filename: self.filenames.get(docname_idx).cloned().unwrap_or_default(),
                title: self.titles.get(docname_idx).cloned().unwrap_or_default(),
                score,
                excerpt: self.generate_excerpt(docname_idx, &query_terms),
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(50); // Limit results

        results
    }

    /// Generate an excerpt for search results
    fn generate_excerpt(&self, _docname_idx: usize, _query_terms: &[String]) -> String {
        // TODO: Implement excerpt generation
        String::new()
    }

    /// Prune the search index by removing documents not in the given set
    pub fn prune(&mut self, valid_docs: &std::collections::HashSet<String>) {
        let mut new_docnames = Vec::new();
        let mut new_filenames = Vec::new();
        let mut new_titles = Vec::new();
        let mut doc_mapping = HashMap::new();

        // Build new document lists and mapping
        for (old_idx, docname) in self.docnames.iter().enumerate() {
            if valid_docs.contains(docname) {
                let new_idx = new_docnames.len();
                doc_mapping.insert(old_idx, new_idx);
                new_docnames.push(docname.clone());
                new_filenames.push(self.filenames.get(old_idx).cloned().unwrap_or_default());
                new_titles.push(self.titles.get(old_idx).cloned().unwrap_or_default());
            }
        }

        // Update document lists
        self.docnames = new_docnames;
        self.filenames = new_filenames;
        self.titles = new_titles;

        // Update terms with new document indices
        for matches in self.terms.values_mut() {
            matches.retain_mut(|doc_match| {
                if let Some(&new_idx) = doc_mapping.get(&doc_match.docname_idx) {
                    doc_match.docname_idx = new_idx;
                    true
                } else {
                    false
                }
            });
        }

        // Remove empty terms
        self.terms.retain(|_, matches| !matches.is_empty());

        // Update objects with new document indices
        self.objects.retain(|_, obj_ref| {
            if let Some(&new_idx) = doc_mapping.get(&obj_ref.docname_idx) {
                obj_ref.docname_idx = new_idx;
                true
            } else {
                false
            }
        });
    }

    /// Export search index to JSON format compatible with Sphinx
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.to_sphinx_value())?)
    }

    /// Freeze the in-memory index using Sphinx's 9.1 searchindex schema.
    pub fn to_sphinx_value(&self) -> Value {
        let mut terms = BTreeMap::<String, BTreeSet<usize>>::new();
        for (term, matches) in &self.terms {
            for item in matches {
                terms
                    .entry(term.clone())
                    .or_default()
                    .insert(item.docname_idx);
            }
        }

        let mut titleterms = BTreeMap::<String, BTreeSet<usize>>::new();
        for (index, title) in self.titles.iter().enumerate() {
            for word in search_words(title) {
                if let Some(term) = self.term_for_word(&word) {
                    titleterms.entry(term).or_default().insert(index);
                }
            }
        }

        let (objects, objtypes, objnames) = direct_search_objects(self);
        freeze_search_value(FrozenSearchData {
            docnames: self.docnames.clone(),
            filenames: self.filenames.clone(),
            titles: self.titles.clone(),
            terms,
            titleterms,
            alltitles: self
                .titles
                .iter()
                .enumerate()
                .map(|(index, title)| (title.clone(), vec![(index, None)]))
                .collect(),
            objects,
            objtypes,
            objnames,
            indexentries: BTreeMap::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Sphinx's IndexBuilder (sphinx/search/__init__.py:202-636)
// ---------------------------------------------------------------------------

/// What `_feed_visit_nodes` collects from one document: Sphinx's
/// `WordStore`.
#[derive(Debug, Default)]
struct WordStore {
    words: Vec<String>,
    titles: Vec<(String, Option<String>)>,
    title_words: Vec<String>,
}

/// Sphinx's `IndexBuilder`: fed one *resolved* document at a time by the
/// build's resolve phase (Sphinx: `write_doc_serialized` → `index_page` →
/// `feed`), then frozen once into the `searchindex.js` value.
///
/// Feeding the resolved doctree is what makes cross-reference text match:
/// a `:doc:` or `:ref:` without an explicit title indexes the target's
/// title, which is the text of the reference `ReferencesResolver` built.
#[derive(Debug, Default)]
pub struct SphinxIndexBuilder {
    /// docname -> title (`_titles`); its key set is the document list.
    titles: BTreeMap<String, String>,
    /// docname -> source-relative filename (`_filenames`).
    filenames: BTreeMap<String, String>,
    /// stemmed word -> docnames (`_mapping`).
    mapping: BTreeMap<String, BTreeSet<String>>,
    /// stemmed title word -> docnames (`_title_mapping`).
    title_mapping: BTreeMap<String, BTreeSet<String>>,
    /// docname -> every title in the document with its anchor
    /// (`_all_titles`).
    all_titles: BTreeMap<String, Vec<(String, Option<String>)>>,
    /// docname -> its sorted `(entry, target_id, main)` index entries
    /// (`_index_entries`).
    index_entries: BTreeMap<String, Vec<(String, String, bool)>>,
    /// Sphinx memoises `stem` for the duration of each `feed`; one cache
    /// for the whole build gives the same terms without re-stemming the
    /// words every document repeats.
    stems: HashMap<String, String>,
}

impl SphinxIndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// `IndexBuilder.feed`, behind the `index_page` gate: a document whose
    /// metadata carries `:nosearch:` (or `:no-search:`) is fed an empty
    /// title and an empty document, so it keeps its slot in `docnames` and
    /// contributes nothing else.
    pub fn feed(
        &mut self,
        docname: &str,
        filename: &str,
        title: &str,
        doctree: &Doctree,
        env: &BuildEnvironment,
    ) {
        let no_search = env.metadata.get(docname).is_some_and(|metadata| {
            metadata.contains_key("nosearch") || metadata.contains_key("no-search")
        });
        let (title, store) = if no_search {
            ("", WordStore::default())
        } else {
            (title, collect_words(doctree, docname, env))
        };

        self.titles.insert(docname.to_string(), title.to_string());
        self.filenames
            .insert(docname.to_string(), filename.to_string());
        self.all_titles.insert(docname.to_string(), store.titles);

        for word in &store.title_words {
            if let Some(term) = self.term(word) {
                self.title_mapping
                    .entry(term)
                    .or_default()
                    .insert(docname.to_string());
            }
        }
        for word in &store.words {
            let Some(term) = self.term(word) else {
                continue;
            };
            // A term this document's titles already carry is not indexed
            // again as body text.
            let already_indexed = self
                .title_mapping
                .get(&term)
                .is_some_and(|docnames| docnames.contains(docname));
            if !already_indexed {
                self.mapping
                    .entry(term)
                    .or_default()
                    .insert(docname.to_string());
            }
        }

        self.index_entries
            .insert(docname.to_string(), index_entries_of(env, docname));
    }

    /// The memoised `stem` plus the "stemmer must not remove words" rule.
    fn term(&mut self, word: &str) -> Option<String> {
        let stemmed = match self.stems.get(word) {
            Some(stemmed) => stemmed.clone(),
            None => {
                let stemmed = stem_english(word);
                self.stems.insert(word.to_string(), stemmed.clone());
                stemmed
            }
        };
        index_term(word, stemmed, english_word_filter)
    }

    /// `IndexBuilder.freeze`: the `Search.setIndex` payload.
    pub fn freeze(&self, env: &BuildEnvironment) -> Value {
        let docnames: Vec<String> = self.titles.keys().cloned().collect();
        let titles: Vec<String> = self.titles.values().cloned().collect();
        let filenames: Vec<String> = docnames
            .iter()
            .map(|docname| self.filenames.get(docname).cloned().unwrap_or_default())
            .collect();
        let fn2index: BTreeMap<String, usize> = docnames
            .iter()
            .enumerate()
            .map(|(index, docname)| (docname.clone(), index))
            .collect();

        let terms = index_postings(&self.mapping, &fn2index);
        let titleterms = index_postings(&self.title_mapping, &fn2index);
        let (objects, objtypes, objnames) = search_objects(env, &fn2index);

        let mut alltitles = BTreeMap::<String, Vec<(usize, Option<String>)>>::new();
        for (docname, titlelist) in &self.all_titles {
            let Some(&index) = fn2index.get(docname) else {
                continue;
            };
            for (title, title_id) in titlelist {
                alltitles
                    .entry(title.clone())
                    .or_default()
                    .push((index, title_id.clone()));
            }
        }

        let mut indexentries = BTreeMap::<String, Vec<(usize, String, bool)>>::new();
        for (docname, entries) in &self.index_entries {
            let Some(&index) = fn2index.get(docname) else {
                continue;
            };
            for (entry, entry_id, main) in entries {
                indexentries.entry(entry.to_lowercase()).or_default().push((
                    index,
                    entry_id.clone(),
                    *main,
                ));
            }
        }

        freeze_search_value(FrozenSearchData {
            docnames,
            filenames,
            titles,
            terms,
            titleterms,
            alltitles,
            objects,
            objtypes,
            objnames,
            indexentries,
        })
    }
}

/// `IndexBuilder.get_terms`: term -> the sorted indices of its documents.
fn index_postings(
    mapping: &BTreeMap<String, BTreeSet<String>>,
    fn2index: &BTreeMap<String, usize>,
) -> BTreeMap<String, BTreeSet<usize>> {
    mapping
        .iter()
        .filter_map(|(term, docnames)| {
            let indices: BTreeSet<usize> = docnames
                .iter()
                .filter_map(|docname| fn2index.get(docname).copied())
                .collect();
            (!indices.is_empty()).then(|| (term.clone(), indices))
        })
        .collect()
}

/// The `(entry, target_id, main)` triples `feed` records from the
/// document's `index` nodes: every part `split_index_msg` splits an entry
/// into, as the sorted set Sphinx builds. An entry `split_index_msg`
/// rejects is skipped (`except ValueError: pass`); the general index is
/// where it gets warned about.
fn index_entries_of(env: &BuildEnvironment, docname: &str) -> Vec<(String, String, bool)> {
    // Sphinx sorts `(entry, target_id, main)` with `main` as the string
    // `'main'` or `''`, which orders `''` first.
    let mut entries = BTreeSet::<(String, String, &str)>::new();
    for record in env
        .index_entries
        .get(docname)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Ok(parts) = crate::env::genindex::split_index_msg(&record.entry_type, &record.value)
        else {
            continue;
        };
        let main = if record.main { "main" } else { "" };
        for part in parts {
            entries.insert((part, record.target_id.clone(), main));
        }
    }
    entries
        .into_iter()
        .map(|(entry, target_id, main)| (entry, target_id, main == "main"))
        .collect()
}

/// `IndexBuilder._word_collector`.
///
/// Sphinx's `MetadataCollector` has popped the `docinfo` out of the
/// doctree by the time the collector runs; this crate leaves that field
/// list in place (see [`crate::env::metadata`]), so the walk starts below
/// it.
fn collect_words(doctree: &Doctree, docname: &str, env: &BuildEnvironment) -> WordStore {
    let mut store = WordStore::default();
    let docinfo = crate::env::metadata::docinfo_field_list(doctree);
    for child in &doctree.root.children {
        if docinfo.is_some_and(|docinfo| std::ptr::eq(docinfo, child)) {
            continue;
        }
        feed_visit_nodes(child, &doctree.root.attrs.ids, &mut store, docname, env);
    }
    store
}

/// `_feed_visit_nodes` (`search/__init__.py:592-636`) over the resolved
/// doctree, with the two things Sphinx's write phase did to that tree
/// before the collector saw it folded in: a `toctree` node is visited as
/// the tree [`crate::env::toctree::resolve_toctree`] replaces it with
/// (`get_and_resolve_doctree`), and a `system_message` not at all
/// (`FilterSystemMessages` removes every one at the default
/// `keep_warnings = False`).
fn feed_visit_nodes(
    node: &Node,
    parent_ids: &[String],
    store: &mut WordStore,
    docname: &str,
    env: &BuildEnvironment,
) {
    if node.kind == kinds::COMMENT || node.kind == kinds::SYSTEM_MESSAGE {
        return;
    }
    if node.attrs.classes.iter().any(|class| class == "no-search") {
        // skip nodes marked with a 'no-search' class
        return;
    }
    match node.kind {
        "raw" => {
            if raw_format_is_html(node) {
                // "Some people might put content in raw HTML that should be
                // searched, so we just amateurishly strip HTML tags and
                // index the remaining content"
                let text = strip_html_tags(&node.astext());
                store.words.extend(search_words(&text));
            }
            return;
        }
        kinds::TOCTREE => {
            if let Some(resolved) = crate::env::toctree::resolve_toctree(env, docname, node) {
                feed_visit_nodes(&resolved, parent_ids, store, docname, env);
            }
            return;
        }
        "meta" if is_meta_keywords(node) => {
            if let Some(AttrValue::Str(content)) = node.get("content") {
                store
                    .words
                    .extend(content.split(',').map(|keyword| keyword.trim().to_string()));
            }
        }
        kinds::TEXT => store.words.extend(search_words(&node.astext())),
        kinds::TITLE => {
            let title = node.astext();
            // The first title met is the document's; only later ones carry
            // their parent's anchor.
            let is_main_title = store.titles.is_empty();
            let title_id = if is_main_title {
                None
            } else {
                parent_ids.first().cloned()
            };
            store.titles.push((title.clone(), title_id));
            store.title_words.extend(search_words(&title));
        }
        _ => {}
    }
    for child in &node.children {
        feed_visit_nodes(child, &node.attrs.ids, store, docname, env);
    }
}

/// `_is_meta_keywords` for the English search language.
fn is_meta_keywords(node: &Node) -> bool {
    matches!(node.get("name"), Some(AttrValue::Str(name)) if name == "keywords")
        && match node.get("lang") {
            None => true,
            Some(AttrValue::Str(lang)) => lang == "en",
            Some(_) => false,
        }
}

/// `'html' in node.get('format', '').split()`.
fn raw_format_is_html(node: &Node) -> bool {
    match node.get("format") {
        Some(AttrValue::Str(format)) => format.split_whitespace().any(|f| f == "html"),
        _ => false,
    }
}

/// The three substitutions Sphinx runs over raw HTML before indexing it.
fn strip_html_tags(text: &str) -> String {
    static STYLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<style.*?</style>").unwrap());
    static SCRIPT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<script.*?</script>").unwrap());
    static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^<]+?>").unwrap());
    let text = STYLE.replace_all(text, "");
    let text = SCRIPT.replace_all(&text, "");
    TAG.replace_all(&text, "").into_owned()
}

/// Render `Search.setIndex(...)` as `sphinx.search.js_index` does: the
/// payload is `json.dumps(data, separators=(',', ':'), sort_keys=True)`,
/// so every object's keys are sorted and, with the default
/// `ensure_ascii`, every non-ASCII character is a `\uXXXX` escape.
/// `objtypes` and `objnames` are keyed by int in Sphinx, so their keys sort
/// numerically rather than as the strings JSON makes of them.
pub fn dumps_sphinx_index(value: &Value) -> Result<String> {
    let mut out = String::from("Search.setIndex(");
    write_python_json(value, &mut out, false);
    out.push(')');
    Ok(out)
}

fn write_python_json(value: &Value, out: &mut String, int_keys: bool) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(string) => write_python_json_string(string, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_python_json(item, out, false);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            if int_keys {
                keys.sort_by_key(|key| key.parse::<u64>().unwrap_or(u64::MAX));
            } else {
                keys.sort();
            }
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_python_json_string(key, out);
                out.push(':');
                let child_int_keys = matches!(key.as_str(), "objtypes" | "objnames");
                write_python_json(&map[key.as_str()], out, child_int_keys);
            }
            out.push('}');
        }
    }
}

/// `json.encoder.py_encode_basestring_ascii`: `"` and `\` escaped, the
/// five short escapes, and every other character outside `U+0020..U+007E`
/// as `\uXXXX` (surrogate pairs above the BMP).
fn write_python_json_string(string: &str, out: &mut String) {
    use std::fmt::Write as _;

    out.push('"');
    for character in string.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(character),
            _ => {
                let mut units = [0u16; 2];
                for unit in character.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

struct FrozenSearchData {
    docnames: Vec<String>,
    filenames: Vec<String>,
    titles: Vec<String>,
    terms: BTreeMap<String, BTreeSet<usize>>,
    titleterms: BTreeMap<String, BTreeSet<usize>>,
    alltitles: BTreeMap<String, Vec<(usize, Option<String>)>>,
    objects: BTreeMap<String, Vec<[Value; 5]>>,
    objtypes: BTreeMap<String, String>,
    objnames: BTreeMap<String, [String; 3]>,
    indexentries: BTreeMap<String, Vec<(usize, String, bool)>>,
}

fn freeze_search_value(data: FrozenSearchData) -> Value {
    let FrozenSearchData {
        docnames,
        filenames,
        titles,
        terms,
        titleterms,
        alltitles,
        objects,
        objtypes,
        objnames,
        indexentries,
    } = data;
    let terms = postings_to_json(terms);
    let titleterms = postings_to_json(titleterms);
    let alltitles = alltitles
        .into_iter()
        .map(|(title, entries)| {
            (
                title,
                entries
                    .into_iter()
                    .map(|(index, id)| serde_json::json!([index, id]))
                    .collect::<Vec<_>>()
                    .into(),
            )
        })
        .collect::<Map<_, _>>();
    let indexentries = indexentries
        .into_iter()
        .map(|(entry, locations)| {
            (
                entry,
                locations
                    .into_iter()
                    .map(|(index, id, main)| serde_json::json!([index, id, main]))
                    .collect::<Vec<_>>()
                    .into(),
            )
        })
        .collect::<Map<_, _>>();

    serde_json::json!({
        "alltitles": alltitles,
        "docnames": docnames,
        "envversion": sphinx_91_envversion(),
        "filenames": filenames,
        "indexentries": indexentries,
        "objects": objects,
        "objnames": objnames,
        "objtypes": objtypes,
        "terms": terms,
        "titles": titles,
        "titleterms": titleterms,
    })
}

fn postings_to_json(postings: BTreeMap<String, BTreeSet<usize>>) -> Map<String, Value> {
    postings
        .into_iter()
        .map(|(term, indices)| {
            let value = if indices.len() == 1 {
                Value::from(*indices.iter().next().unwrap())
            } else {
                Value::Array(indices.into_iter().map(Value::from).collect())
            };
            (term, value)
        })
        .collect()
}

/// `SearchLanguage.split`: Python's `\w+`. For a `str` pattern `\w` is
/// `str.isalnum()` plus the underscore, i.e. the letter and number
/// categories — narrower than `char::is_alphanumeric`, whose `Alphabetic`
/// property also covers combining marks and some symbols, so a Devanagari
/// vowel sign or an enclosed letter would join a word here that Sphinx
/// splits on.
fn search_words(text: &str) -> Vec<String> {
    static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{N}_]+").unwrap());
    WORD.find_iter(text)
        .map(|word| word.as_str().to_string())
        .collect()
}

/// The term Sphinx's English search indexes for `word`, or `None` when the
/// word is dropped: [`index_term`] over the Snowball stem and
/// `SearchEnglish.word_filter`.
fn search_term(word: &str) -> Option<String> {
    index_term(word, stem_english(word), english_word_filter)
}

/// The one rule `IndexBuilder.feed` applies to title words and body words
/// alike: the stem when `word_filter` accepts it, else the unstemmed word
/// when it accepts *that* ("the stemmer must not remove words from search
/// index" — `Some` stems to the stopword `some` and is indexed verbatim),
/// else nothing.
fn index_term(word: &str, stemmed: String, word_filter: impl Fn(&str) -> bool) -> Option<String> {
    if word_filter(&stemmed) {
        Some(stemmed)
    } else if word_filter(word) {
        Some(word.to_string())
    } else {
        None
    }
}

/// `SearchEnglish.word_filter`, which runs on the *stemmed* word.
fn english_word_filter(word: &str) -> bool {
    !is_digit_word(word) && !is_english_stopword(word)
}

/// Python `str.isdigit()`: non-empty and every character a digit.
fn is_digit_word(word: &str) -> bool {
    !word.is_empty() && word.chars().all(is_sphinx_digit)
}

/// `SearchEnglish.stem`: Snowball English over the lowercased word.
fn stem_english(word: &str) -> String {
    EnglishStemmer::new(word).stem()
}

/// Match Python 3.12's `str.isdigit()` used by Sphinx's English search.
fn is_sphinx_digit(character: char) -> bool {
    matches!(
        character as u32,
        0x30..=0x39
            | 0xB2..=0xB3
            | 0xB9
            | 0x660..=0x669
            | 0x6F0..=0x6F9
            | 0x7C0..=0x7C9
            | 0x966..=0x96F
            | 0x9E6..=0x9EF
            | 0xA66..=0xA6F
            | 0xAE6..=0xAEF
            | 0xB66..=0xB6F
            | 0xBE6..=0xBEF
            | 0xC66..=0xC6F
            | 0xCE6..=0xCEF
            | 0xD66..=0xD6F
            | 0xDE6..=0xDEF
            | 0xE50..=0xE59
            | 0xED0..=0xED9
            | 0xF20..=0xF29
            | 0x1040..=0x1049
            | 0x1090..=0x1099
            | 0x1369..=0x1371
            | 0x17E0..=0x17E9
            | 0x1810..=0x1819
            | 0x1946..=0x194F
            | 0x19D0..=0x19DA
            | 0x1A80..=0x1A89
            | 0x1A90..=0x1A99
            | 0x1B50..=0x1B59
            | 0x1BB0..=0x1BB9
            | 0x1C40..=0x1C49
            | 0x1C50..=0x1C59
            | 0x2070
            | 0x2074..=0x2079
            | 0x2080..=0x2089
            | 0x2460..=0x2468
            | 0x2474..=0x247C
            | 0x2488..=0x2490
            | 0x24EA
            | 0x24F5..=0x24FD
            | 0x24FF
            | 0x2776..=0x277E
            | 0x2780..=0x2788
            | 0x278A..=0x2792
            | 0xA620..=0xA629
            | 0xA8D0..=0xA8D9
            | 0xA900..=0xA909
            | 0xA9D0..=0xA9D9
            | 0xA9F0..=0xA9F9
            | 0xAA50..=0xAA59
            | 0xABF0..=0xABF9
            | 0xFF10..=0xFF19
            | 0x104A0..=0x104A9
            | 0x10A40..=0x10A43
            | 0x10D30..=0x10D39
            | 0x10E60..=0x10E68
            | 0x11052..=0x1105A
            | 0x11066..=0x1106F
            | 0x110F0..=0x110F9
            | 0x11136..=0x1113F
            | 0x111D0..=0x111D9
            | 0x112F0..=0x112F9
            | 0x11450..=0x11459
            | 0x114D0..=0x114D9
            | 0x11650..=0x11659
            | 0x116C0..=0x116C9
            | 0x11730..=0x11739
            | 0x118E0..=0x118E9
            | 0x11950..=0x11959
            | 0x11C50..=0x11C59
            | 0x11D50..=0x11D59
            | 0x11DA0..=0x11DA9
            | 0x11F50..=0x11F59
            | 0x16A60..=0x16A69
            | 0x16AC0..=0x16AC9
            | 0x16B50..=0x16B59
            | 0x1D7CE..=0x1D7FF
            | 0x1E140..=0x1E149
            | 0x1E2F0..=0x1E2F9
            | 0x1E4F0..=0x1E4F9
            | 0x1E950..=0x1E959
            | 0x1F100..=0x1F10A
            | 0x1FBF0..=0x1FBF9
    )
}

fn is_english_stopword(word: &str) -> bool {
    ENGLISH_STOPWORDS.binary_search(&word).is_ok()
}

/// Sphinx's `ENGLISH_STOPWORDS` (`search/en.py`), kept in byte order so
/// [`is_english_stopword`] can bisect it.
const ENGLISH_STOPWORDS: &[&str] = &[
    "a",
    "about",
    "above",
    "after",
    "again",
    "against",
    "all",
    "am",
    "an",
    "and",
    "any",
    "are",
    "aren't",
    "as",
    "at",
    "be",
    "because",
    "been",
    "before",
    "being",
    "below",
    "between",
    "both",
    "but",
    "by",
    "can't",
    "cannot",
    "could",
    "couldn't",
    "did",
    "didn't",
    "do",
    "does",
    "doesn't",
    "doing",
    "don't",
    "down",
    "during",
    "each",
    "few",
    "for",
    "from",
    "further",
    "had",
    "hadn't",
    "has",
    "hasn't",
    "have",
    "haven't",
    "having",
    "he",
    "he'd",
    "he'll",
    "he's",
    "her",
    "here",
    "here's",
    "hers",
    "herself",
    "him",
    "himself",
    "his",
    "how",
    "how's",
    "i",
    "i'd",
    "i'll",
    "i'm",
    "i've",
    "if",
    "in",
    "into",
    "is",
    "isn't",
    "it",
    "it's",
    "its",
    "itself",
    "let's",
    "me",
    "more",
    "most",
    "mustn't",
    "my",
    "myself",
    "no",
    "nor",
    "not",
    "of",
    "off",
    "on",
    "once",
    "only",
    "or",
    "other",
    "ought",
    "our",
    "ours",
    "ourselves",
    "out",
    "over",
    "own",
    "same",
    "shan't",
    "she",
    "she'd",
    "she'll",
    "she's",
    "should",
    "shouldn't",
    "so",
    "some",
    "such",
    "than",
    "that",
    "that's",
    "the",
    "their",
    "theirs",
    "them",
    "themselves",
    "then",
    "there",
    "there's",
    "these",
    "they",
    "they'd",
    "they'll",
    "they're",
    "they've",
    "this",
    "those",
    "through",
    "to",
    "too",
    "under",
    "until",
    "up",
    "very",
    "was",
    "wasn't",
    "we",
    "we'd",
    "we'll",
    "we're",
    "we've",
    "were",
    "weren't",
    "what",
    "what's",
    "when",
    "when's",
    "where",
    "where's",
    "which",
    "while",
    "who",
    "who's",
    "whom",
    "why",
    "why's",
    "with",
    "won't",
    "would",
    "wouldn't",
    "you",
    "you'd",
    "you'll",
    "you're",
    "you've",
    "your",
    "yours",
    "yourself",
    "yourselves",
];

struct EnglishStemmer {
    chars: Vec<char>,
    region1_start: usize,
    region2_start: usize,
    y_found: bool,
}

impl EnglishStemmer {
    fn new(word: &str) -> Self {
        Self {
            chars: word.to_lowercase().chars().collect(),
            region1_start: 0,
            region2_start: 0,
            y_found: false,
        }
    }

    fn stem(mut self) -> String {
        let original = self.as_str();
        match original.as_str() {
            "andes" | "atlas" | "bias" | "cosmos" | "howe" | "news" | "sky" => {
                return original;
            }
            "early" => return "earli".to_string(),
            "gently" => return "gentl".to_string(),
            "idly" => return "idl".to_string(),
            "only" => return "onli".to_string(),
            "singly" => return "singl".to_string(),
            "skies" => return "sky".to_string(),
            "skis" => return "ski".to_string(),
            "ugly" => return "ugli".to_string(),
            _ => {}
        }
        if self.chars.len() < 3 {
            return original;
        }

        self.prelude();
        self.mark_regions();
        self.step_1a();
        self.step_1b();
        self.step_1c();
        self.step_2();
        self.step_3();
        self.step_4();
        self.step_5();
        self.postlude();
        self.as_str()
    }

    fn as_str(&self) -> String {
        self.chars.iter().collect()
    }

    fn prelude(&mut self) {
        if self.chars.first() == Some(&'\'') {
            self.chars.remove(0);
        }
        if self.chars.first() == Some(&'y') {
            self.chars[0] = 'Y';
            self.y_found = true;
        }
        for index in 0..self.chars.len().saturating_sub(1) {
            if Self::is_vowel(self.chars[index]) && self.chars[index + 1] == 'y' {
                self.chars[index + 1] = 'Y';
                self.y_found = true;
            }
        }
    }

    fn mark_regions(&mut self) {
        const SPECIAL_PREFIXES: &[&str] = &[
            "arsen", "commun", "emerg", "gener", "inter", "later", "organ", "past", "univers",
        ];
        self.region1_start = SPECIAL_PREFIXES
            .iter()
            .find_map(|prefix| self.starts_with(prefix).then(|| prefix.chars().count()))
            .unwrap_or_else(|| self.region_after(0));
        self.region2_start = self.region_after(self.region1_start);
    }

    fn region_after(&self, start: usize) -> usize {
        let mut index = start;
        while index < self.chars.len() && !Self::is_vowel(self.chars[index]) {
            index += 1;
        }
        if index == self.chars.len() {
            return self.chars.len();
        }
        index += 1;
        while index < self.chars.len() && Self::is_vowel(self.chars[index]) {
            index += 1;
        }
        if index == self.chars.len() {
            return self.chars.len();
        }
        index + 1
    }

    fn step_1a(&mut self) {
        if let Some(suffix) = self.matching_suffix(&["'s'", "'s", "'"]) {
            self.remove_suffix(suffix);
        }
        let Some(suffix) = self.matching_suffix(&["sses", "ied", "ies", "us", "ss", "s"]) else {
            return;
        };
        match suffix {
            "sses" => self.replace_suffix("sses", "ss"),
            "ied" | "ies" => {
                let replacement = if self.suffix_start(suffix).unwrap() >= 2 {
                    "i"
                } else {
                    "ie"
                };
                self.replace_suffix(suffix, replacement);
            }
            "s" => {
                let start = self.suffix_start("s").unwrap();
                if start > 0 {
                    let mut cursor = start - 1;
                    while cursor > 0 && !Self::is_vowel(self.chars[cursor - 1]) {
                        cursor -= 1;
                    }
                    if cursor > 0 && Self::is_vowel(self.chars[cursor - 1]) {
                        self.remove_suffix("s");
                    }
                }
            }
            "us" | "ss" => {}
            _ => unreachable!(),
        }
    }

    fn step_1b(&mut self) {
        if let Some(suffix) = self.matching_suffix(&["eedly", "eed"]) {
            let start = self.suffix_start(suffix).unwrap();
            // Snowball 3's `a_3`: the text *before* the suffix is exactly
            // `succ`/`proc`/`exc`, so `exceedly` keeps its `eed` here (step
            // 1c and step 2 then take it to `exceed`), while the whole-word
            // exceptions of Snowball 2 would have let it become `excee`.
            let exception = ["succ", "proc", "exc"]
                .iter()
                .any(|stem| self.chars[..start].iter().copied().eq(stem.chars()));
            if start >= self.region1_start && !exception {
                self.replace_suffix(suffix, "ee");
            }
            return;
        }
        let Some(suffix) = self.matching_suffix(&["ingly", "edly", "ing", "ed"]) else {
            return;
        };
        let start = self.suffix_start(suffix).unwrap();
        // Snowball 3's `a_5` routes `ingly` with `ed`/`edly` through the
        // generic branch; only a bare `ing` consults the `a_4` exceptions
        // (`evening`, `inning`, ... stay; `dying` becomes `die`).
        if suffix == "ing" {
            let special = ["even", "cann", "inn", "earr", "herr", "out"];
            if special
                .iter()
                .any(|word| self.chars[..start].iter().copied().eq(word.chars()))
            {
                return;
            }
            if start == 2 && self.chars[start - 1] == 'y' && !Self::is_vowel(self.chars[start - 2])
            {
                self.chars.truncate(start - 1);
                self.chars.extend("ie".chars());
                return;
            }
        }
        if !self.chars[..start].iter().copied().any(Self::is_vowel) {
            return;
        }
        self.chars.truncate(start);
        if self.ends_with("at") || self.ends_with("bl") || self.ends_with("iz") {
            self.chars.push('e');
        } else if self.ends_with_double_consonant() {
            let double_start = self.chars.len() - 2;
            let keep_short_double =
                double_start == 1 && matches!(self.chars[double_start - 1], 'a' | 'e' | 'o');
            if !keep_short_double {
                self.chars.pop();
            }
        } else if self.chars.len() == self.region1_start && self.short_syllable(self.chars.len()) {
            self.chars.push('e');
        }
    }

    fn step_1c(&mut self) {
        let Some(suffix) = self.matching_suffix(&["y", "Y"]) else {
            return;
        };
        let start = self.suffix_start(suffix).unwrap();
        if start > 1 && !Self::is_vowel(self.chars[start - 1]) {
            self.replace_suffix(suffix, "i");
        }
    }

    fn step_2(&mut self) {
        const RULES: &[(&str, &str)] = &[
            ("tional", "tion"),
            ("enci", "ence"),
            ("anci", "ance"),
            ("abli", "able"),
            ("entli", "ent"),
            ("ization", "ize"),
            ("izer", "ize"),
            ("ational", "ate"),
            ("ation", "ate"),
            ("ator", "ate"),
            ("alism", "al"),
            ("aliti", "al"),
            ("alli", "al"),
            ("fulness", "ful"),
            ("ousli", "ous"),
            ("ousness", "ous"),
            ("iveness", "ive"),
            ("iviti", "ive"),
            ("biliti", "ble"),
            ("bli", "ble"),
            ("fulli", "ful"),
            ("lessli", "less"),
            ("ogist", "og"),
            ("ogi", "og"),
            ("li", ""),
        ];
        let Some((suffix, replacement)) = RULES
            .iter()
            .filter(|(suffix, _)| self.ends_with(suffix))
            .max_by_key(|(suffix, _)| suffix.chars().count())
            .copied()
        else {
            return;
        };
        let start = self.suffix_start(suffix).unwrap();
        if start < self.region1_start {
            return;
        }
        if suffix == "ogi" && (start == 0 || self.chars[start - 1] != 'l') {
            return;
        }
        if suffix == "li"
            && (start == 0
                || !matches!(
                    self.chars[start - 1],
                    'c' | 'd' | 'e' | 'g' | 'h' | 'k' | 'm' | 'n' | 'r' | 't'
                ))
        {
            return;
        }
        self.replace_suffix(suffix, replacement);
    }

    fn step_3(&mut self) {
        const RULES: &[(&str, &str, bool)] = &[
            ("tional", "tion", false),
            ("ational", "ate", false),
            ("alize", "al", false),
            ("icate", "ic", false),
            ("iciti", "ic", false),
            ("ical", "ic", false),
            ("ful", "", false),
            ("ness", "", false),
            ("ative", "", true),
        ];
        let Some((suffix, replacement, region2)) = RULES
            .iter()
            .filter(|(suffix, _, _)| self.ends_with(suffix))
            .max_by_key(|(suffix, _, _)| suffix.chars().count())
            .copied()
        else {
            return;
        };
        let start = self.suffix_start(suffix).unwrap();
        if start
            >= if region2 {
                self.region2_start
            } else {
                self.region1_start
            }
        {
            self.replace_suffix(suffix, replacement);
        }
    }

    fn step_4(&mut self) {
        const SUFFIXES: &[&str] = &[
            "ement", "ance", "ence", "able", "ible", "ment", "ant", "ent", "ism", "ate", "iti",
            "ous", "ive", "ize", "al", "er", "ic", "ion",
        ];
        let Some(suffix) = SUFFIXES
            .iter()
            .copied()
            .filter(|suffix| self.ends_with(suffix))
            .max_by_key(|suffix| suffix.chars().count())
        else {
            return;
        };
        let start = self.suffix_start(suffix).unwrap();
        if start < self.region2_start {
            return;
        }
        if suffix == "ion" && (start == 0 || !matches!(self.chars[start - 1], 's' | 't')) {
            return;
        }
        self.remove_suffix(suffix);
    }

    fn step_5(&mut self) {
        if self.ends_with("e") {
            let start = self.suffix_start("e").unwrap();
            if start >= self.region2_start
                || (start >= self.region1_start && !self.short_syllable(start))
            {
                self.remove_suffix("e");
            }
        } else if self.ends_with("l") {
            let start = self.suffix_start("l").unwrap();
            if start >= self.region2_start && start > 0 && self.chars[start - 1] == 'l' {
                self.remove_suffix("l");
            }
        }
    }

    fn postlude(&mut self) {
        if self.y_found {
            for character in &mut self.chars {
                if *character == 'Y' {
                    *character = 'y';
                }
            }
        }
    }

    fn is_vowel(character: char) -> bool {
        matches!(character, 'a' | 'e' | 'i' | 'o' | 'u' | 'y')
    }

    fn short_syllable(&self, end: usize) -> bool {
        if end >= 4 && self.chars[..end].ends_with(&['p', 'a', 's', 't']) {
            return true;
        }
        if end >= 3
            && !Self::is_vowel(self.chars[end - 1])
            && !matches!(self.chars[end - 1], 'w' | 'x' | 'Y')
            && Self::is_vowel(self.chars[end - 2])
            && !Self::is_vowel(self.chars[end - 3])
        {
            return true;
        }
        end == 2 && Self::is_vowel(self.chars[0]) && !Self::is_vowel(self.chars[1])
    }

    fn ends_with_double_consonant(&self) -> bool {
        self.chars.len() >= 2
            && self.chars[self.chars.len() - 1] == self.chars[self.chars.len() - 2]
            && matches!(
                self.chars[self.chars.len() - 1],
                'b' | 'd' | 'f' | 'g' | 'm' | 'n' | 'p' | 'r' | 't'
            )
    }

    fn starts_with(&self, prefix: &str) -> bool {
        self.chars
            .iter()
            .take(prefix.chars().count())
            .copied()
            .eq(prefix.chars())
    }

    fn ends_with(&self, suffix: &str) -> bool {
        self.suffix_start(suffix).is_some()
    }

    fn suffix_start(&self, suffix: &str) -> Option<usize> {
        let suffix_len = suffix.chars().count();
        if self.chars.len() < suffix_len {
            return None;
        }
        let start = self.chars.len() - suffix_len;
        self.chars[start..]
            .iter()
            .copied()
            .eq(suffix.chars())
            .then_some(start)
    }

    fn matching_suffix<'a>(&self, suffixes: &'a [&'a str]) -> Option<&'a str> {
        suffixes
            .iter()
            .copied()
            .filter(|suffix| self.ends_with(suffix))
            .max_by_key(|suffix| suffix.chars().count())
    }

    fn replace_suffix(&mut self, suffix: &str, replacement: &str) {
        let start = self.suffix_start(suffix).expect("suffix must be present");
        self.chars.truncate(start);
        self.chars.extend(replacement.chars());
    }

    fn remove_suffix(&mut self, suffix: &str) {
        self.replace_suffix(suffix, "");
    }
}

type SearchObjects = (
    BTreeMap<String, Vec<[Value; 5]>>,
    BTreeMap<String, String>,
    BTreeMap<String, [String; 3]>,
);

/// `IndexBuilder._objtypes`/`_objnames`: one index per `(domain, type)`
/// in first-seen order, with the type's localized name.
#[derive(Default)]
struct ObjectTypes {
    indices: BTreeMap<(String, String), usize>,
    objtypes: BTreeMap<String, String>,
    objnames: BTreeMap<String, [String; 3]>,
}

impl ObjectTypes {
    fn index_of(&mut self, domain: &str, objtype: &str) -> usize {
        let key = (domain.to_string(), objtype.to_string());
        if let Some(&index) = self.indices.get(&key) {
            return index;
        }
        let index = self.indices.len();
        self.objtypes
            .insert(index.to_string(), format!("{domain}:{objtype}"));
        self.objnames.insert(
            index.to_string(),
            [
                domain.to_string(),
                objtype.to_string(),
                object_type_name(domain, objtype),
            ],
        );
        self.indices.insert(key, index);
        index
    }
}

/// `IndexBuilder.get_objects` (`search/__init__.py:363-402`) over
/// [`crate::env::objects::domain_objects`].
fn search_objects(env: &BuildEnvironment, fn2index: &BTreeMap<String, usize>) -> SearchObjects {
    let mut types = ObjectTypes::default();
    let mut objects = BTreeMap::<String, Vec<[Value; 5]>>::new();
    for (domain, domain_objects) in crate::env::objects::domain_objects(env) {
        for object in domain_objects {
            let Some(&doc_index) = fn2index.get(&object.docname) else {
                continue;
            };
            if object.priority < 0 {
                continue;
            }
            let fullname = py_html_escape(&object.name);
            let dispname = py_html_escape(&object.dispname);
            let (prefix, name) = rpartition_dot(&dispname);
            let type_index = types.index_of(domain, &object.objtype);
            objects.entry(prefix).or_default().push([
                Value::from(doc_index),
                Value::from(type_index),
                Value::from(object.priority),
                Value::String(short_anchor(&object.anchor, &fullname, &object.objtype)),
                Value::String(name),
            ]);
        }
    }
    (objects, types.objtypes, types.objnames)
}

/// The same table for the direct [`HTMLBuilder`](crate::HTMLBuilder)
/// path, whose objects come from [`SearchIndex::add_object`] rather than a
/// domain registry.
fn direct_search_objects(index: &SearchIndex) -> SearchObjects {
    let mut references: Vec<&ObjectReference> = index.objects.values().collect();
    references.sort_by(|left, right| {
        left.obj_type
            .cmp(&right.obj_type)
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut types = ObjectTypes::default();
    let mut objects = BTreeMap::<String, Vec<[Value; 5]>>::new();
    for object in references {
        let (domain, objtype) = split_object_type(&object.obj_type);
        let priority = direct_object_priority(domain, objtype);
        if priority < 0 {
            continue;
        }
        let fullname = py_html_escape(&object.name);
        let dispname = py_html_escape(object.description.as_deref().unwrap_or(&object.name));
        let (prefix, name) = rpartition_dot(&dispname);
        let type_index = types.index_of(domain, objtype);
        let anchor = object.anchor.as_deref().unwrap_or_default();
        objects.entry(prefix).or_default().push([
            Value::from(object.docname_idx),
            Value::from(type_index),
            Value::from(priority),
            Value::String(short_anchor(anchor, &fullname, objtype)),
            Value::String(name),
        ]);
    }
    (objects, types.objtypes, types.objnames)
}

/// A direct-path `obj_type` is `domain:type`, or a bare std type.
pub(crate) fn split_object_type(obj_type: &str) -> (&str, &str) {
    obj_type.split_once(':').unwrap_or(("std", obj_type))
}

/// The search priority of a direct-path object, which has no domain
/// registry behind it: Sphinx's std table, and the Python domain's `0` for
/// a module and `1` for anything else (aliases are a registry concept).
pub(crate) fn direct_object_priority(domain: &str, objtype: &str) -> i32 {
    match domain {
        "std" => std_search_priority(objtype),
        "py" if objtype == "module" => 0,
        _ => 1,
    }
}

/// `prefix, _, name = dispname.rpartition('.')`.
fn rpartition_dot(dispname: &str) -> (String, String) {
    match dispname.rsplit_once('.') {
        Some((prefix, name)) => (prefix.to_string(), name.to_string()),
        None => (String::new(), dispname.to_string()),
    }
}

/// The anchor abbreviation `get_objects` applies: `''` for an anchor equal
/// to the full name, `'-'` for `type-fullname`, else the anchor itself.
fn short_anchor(anchor: &str, fullname: &str, objtype: &str) -> String {
    if anchor == fullname {
        String::new()
    } else if anchor == format!("{objtype}-{fullname}") {
        "-".to_string()
    } else {
        anchor.to_string()
    }
}

/// Python `html.escape(s)` with its default `quote=True`.
fn py_html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Search result returned by the search index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub docname: String,
    pub filename: String,
    pub title: String,
    pub score: f32,
    pub excerpt: String,
}

/// Search index builder for incremental updates
pub struct SearchIndexBuilder {
    index: SearchIndex,
    processed_docs: std::collections::HashSet<String>,
}

impl SearchIndexBuilder {
    pub fn new(language: String) -> Self {
        Self {
            index: SearchIndex::new(language),
            processed_docs: std::collections::HashSet::new(),
        }
    }

    /// Add or update a document in the search index
    pub fn add_or_update_document(
        &mut self,
        docname: String,
        filename: String,
        title: String,
        content: &str,
    ) -> Result<()> {
        // Remove existing document if it exists
        if self.processed_docs.contains(&docname) {
            self.remove_document(&docname);
        }

        // Add the document
        self.index
            .add_document(docname.clone(), filename, title, content)?;
        self.processed_docs.insert(docname);

        Ok(())
    }

    /// Remove a document from the search index
    pub fn remove_document(&mut self, docname: &str) {
        if let Some(docname_idx) = self.index.docnames.iter().position(|d| d == docname) {
            // Remove from document lists
            self.index.docnames.remove(docname_idx);
            if docname_idx < self.index.filenames.len() {
                self.index.filenames.remove(docname_idx);
            }
            if docname_idx < self.index.titles.len() {
                self.index.titles.remove(docname_idx);
            }

            // Update indices in terms
            for matches in self.index.terms.values_mut() {
                matches.retain_mut(|doc_match| {
                    if doc_match.docname_idx == docname_idx {
                        false
                    } else if doc_match.docname_idx > docname_idx {
                        doc_match.docname_idx -= 1;
                        true
                    } else {
                        true
                    }
                });
            }

            // Remove empty terms
            self.index.terms.retain(|_, matches| !matches.is_empty());

            // Update indices in objects
            self.index.objects.retain(|_, obj_ref| {
                if obj_ref.docname_idx == docname_idx {
                    false
                } else if obj_ref.docname_idx > docname_idx {
                    obj_ref.docname_idx -= 1;
                    true
                } else {
                    true
                }
            });
        }

        self.processed_docs.remove(docname);
    }

    /// Get the built search index
    pub fn build(self) -> SearchIndex {
        self.index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_index_creation() {
        let index = SearchIndex::new("en".to_string());
        assert_eq!(index.language, "en");
        assert_eq!(index.docnames.len(), 0);
    }

    #[test]
    fn test_add_document() {
        let mut index = SearchIndex::new("en".to_string());
        index
            .add_document(
                "test".to_string(),
                "test.html".to_string(),
                "Test Document".to_string(),
                "This is a test document with some content.",
            )
            .unwrap();

        assert_eq!(index.docnames.len(), 1);
        assert_eq!(index.docnames[0], "test");
        assert!(index.terms.contains_key("test"));
        assert!(index.terms.contains_key("document"));
    }

    #[test]
    fn test_word_normalization() {
        assert_eq!(stem_english("running"), "run");
        assert_eq!(stem_english("walked"), "walk");
        assert_eq!(stem_english("tests"), "test");
        assert_eq!(stem_english("test"), "test");
    }

    #[test]
    fn legacy_search_uses_sphinx_token_pipeline() {
        let mut index = SearchIndex::new("en".to_string());
        index
            .add_document(
                "test".to_string(),
                "test.html".to_string(),
                "Test".to_string(),
                "Running, cats. The quick-brown_fox ½ Ⅳ",
            )
            .unwrap();

        assert!(index.terms.contains_key("run"));
        assert!(index.terms.contains_key("cat"));
        assert!(index.terms.contains_key("quick"));
        assert!(index.terms.contains_key("brown_fox"));
        assert!(!index.terms.contains_key("the"));
        assert!(index.terms.contains_key("½"));
        assert!(index.terms.contains_key("ⅳ"));
        assert!(!index.search("running, cats").is_empty());
    }

    #[test]
    fn words_whose_stems_are_stopwords_are_indexed_verbatim_in_both_paths() {
        // `IndexBuilder.feed`: "the stemmer must not remove words from
        // search index" — only a word that is a stopword *itself* is dropped.
        assert_eq!(search_term("all"), None);
        assert_eq!(search_term("some"), None);
        let kept = ["underlying", "others", "ifs", "mostly", "Some", "SOME", "A"];
        for word in kept {
            assert_eq!(
                search_term(word).as_deref(),
                Some(word),
                "Sphinx keeps {word:?} unstemmed"
            );
        }

        let mut index = SearchIndex::new("en".to_string());
        index
            .add_document(
                "test".to_string(),
                "test.html".to_string(),
                "Title".to_string(),
                "all some underlying others ifs mostly Some SOME A",
            )
            .unwrap();
        for word in kept {
            assert!(index.terms.contains_key(word), "missing {word:?}");
        }
        assert!(!index.terms.contains_key("all"));
        assert!(!index.terms.contains_key("some"));
        let frozen_index = index.to_sphinx_value();
        let frozen_terms = frozen_index["terms"]
            .as_object()
            .expect("frozen terms must be an object");
        for word in kept {
            assert!(frozen_terms.contains_key(word), "frozen {word:?}");
        }
    }

    #[test]
    fn non_english_indexes_lowercase_and_drops_digits_only() {
        let index = SearchIndex::new("de".to_string());
        assert_eq!(index.term_for_word("Laufen").as_deref(), Some("laufen"));
        assert_eq!(index.term_for_word("the").as_deref(), Some("the"));
        assert_eq!(index.term_for_word("2024"), None);

        // Title terms go through the same language rule as body terms.
        let mut index = SearchIndex::new("de".to_string());
        index
            .add_document(
                "test".to_string(),
                "test.html".to_string(),
                "Running Tests".to_string(),
                "running tests",
            )
            .unwrap();
        let value = index.to_sphinx_value();
        assert_eq!(
            value["titleterms"],
            serde_json::json!({"running": 0, "tests": 0})
        );
        assert_eq!(
            value["terms"],
            serde_json::json!({"running": 0, "tests": 0})
        );
    }

    #[derive(Debug)]
    struct EnglishSearchWord {
        word: String,
        stem: String,
        term: Option<String>,
    }

    fn english_search_fixture() -> Vec<EnglishSearchWord> {
        let content = match std::env::var_os("SPHINX_ULTRA_SEARCH_FIXTURE") {
            Some(path) => std::fs::read_to_string(path)
                .expect("SPHINX_ULTRA_SEARCH_FIXTURE must point to a readable TSV"),
            None => include_str!("../tests/fixtures/search_english.tsv").to_string(),
        };
        let headers: Vec<_> = content.lines().take(9).collect();
        assert!(headers.contains(&"# sphinx=9.1.0"));
        assert!(headers.contains(&"# docutils=0.22.4"));
        assert!(headers.contains(&"# snowballstemmer=3.1.1"));
        assert!(headers.contains(&"# generator=tools/gen_search_english_fixture.py"));
        assert!(headers.contains(&"# columns=word<TAB>stem<TAB>term"));
        content
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                let fields: Vec<_> = line.split('\t').collect();
                assert_eq!(
                    fields.len(),
                    3,
                    "search English TSV rows must have word, stem and term columns"
                );
                EnglishSearchWord {
                    word: fields[0].to_string(),
                    stem: fields[1].to_string(),
                    term: (fields[2] != "-").then(|| fields[2].to_string()),
                }
            })
            .collect()
    }

    #[test]
    fn english_search_matches_sphinx_golden_words() {
        let fixture = english_search_fixture();

        assert!(
            fixture.len() >= 20_000,
            "golden corpus unexpectedly shrank to {} words",
            fixture.len()
        );
        for expected in &fixture {
            assert_eq!(
                stem_english(&expected.word),
                expected.stem,
                "stem mismatch for {:?}",
                expected.word
            );
            assert_eq!(
                search_term(&expected.word),
                expected.term,
                "term mismatch for {:?}",
                expected.word
            );
        }
        assert!(fixture
            .windows(2)
            .all(|words| words[0].word < words[1].word));
    }

    #[test]
    fn english_stopword_table_is_sphinx_s_and_bisectable() {
        // Sphinx's `ENGLISH_STOPWORDS` has 174 entries; the golden fixture
        // pins their effect word by word, this pins the table's shape.
        assert_eq!(ENGLISH_STOPWORDS.len(), 174);
        assert!(
            ENGLISH_STOPWORDS.windows(2).all(|pair| pair[0] < pair[1]),
            "the stopword table must stay sorted for binary_search"
        );
        for stopword in ["a", "can't", "doing", "it's", "yourselves"] {
            assert!(is_english_stopword(stopword), "{stopword:?}");
        }
        // The filter is case-sensitive: `Some` is not the stopword `some`.
        for word in ["Some", "The", "document", ""] {
            assert!(!is_english_stopword(word), "{word:?}");
        }
    }

    #[test]
    fn splitter_matches_python_word_regex() {
        for (input, expected) in SPLIT_CASES {
            assert_eq!(
                search_words(input),
                *expected,
                "split mismatch for {:?}",
                input
            );
        }
    }

    #[test]
    fn english_search_terms_match_sphinx_edge_cases() {
        // Apostrophes never reach the stemmer through `split` (`\w+` cuts
        // `can't` into `can` and `t`), so only `\w`-only inputs are
        // observable behaviour.
        let cases = [
            ("the", None),
            ("The", Some("The")),
            ("A", Some("A")),
            ("only", Some("onli")),
            ("inventory", Some("inventori")),
            ("eveningly", Some("even")),
            ("evening", Some("evening")),
            ("dying", Some("die")),
            ("exceedly", Some("exceed")),
            ("exceeded", Some("exceed")),
            ("agreed", Some("agre")),
            ("½", Some("½")),
            ("Ⅳ", Some("ⅳ")),
            ("¹", None),
            ("2024", None),
        ];
        for (word, expected) in cases {
            assert_eq!(
                search_term(word).as_deref(),
                expected,
                "term mismatch for {word:?}"
            );
        }
    }

    #[test]
    fn raw_html_is_indexed_without_tags_styles_or_scripts() {
        let text = strip_html_tags(
            "<p>rawhtmlword <b>boldword</b></p><STYLE>css { rule }</STYLE><script>var x;</script>",
        );
        assert_eq!(search_words(&text), vec!["rawhtmlword", "boldword"]);
    }

    #[test]
    fn dumps_like_python_json_with_ensure_ascii_and_int_keys() {
        let value = serde_json::json!({
            "titles": ["Café \u{1F600}", "tab\there"],
            "objtypes": {"0": "py:class", "10": "py:data", "2": "py:function"},
            "alltitles": {"10": [[0, null]], "2": [[1, "two"]]},
        });
        let dumped = dumps_sphinx_index(&value).unwrap();
        assert_eq!(
            dumped,
            "Search.setIndex({\"alltitles\":{\"10\":[[0,null]],\"2\":[[1,\"two\"]]},\
             \"objtypes\":{\"0\":\"py:class\",\"2\":\"py:function\",\"10\":\"py:data\"},\
             \"titles\":[\"Caf\\u00e9 \\ud83d\\ude00\",\"tab\\there\"]})"
        );
    }

    const SPLIT_CASES: &[(&str, &[&str])] = &[
        (
            "The quick-brown_fox, can't email@example.com 123",
            &[
                "The",
                "quick",
                "brown_fox",
                "can",
                "t",
                "email",
                "example",
                "com",
                "123",
            ],
        ),
        (
            "Café naïve coöperate; one—two.",
            &["Café", "naïve", "coöperate", "one", "two"],
        ),
        (
            "a\u{301} e\u{301} C++ Rust's _private",
            &["a", "e", "C", "Rust", "s", "_private"],
        ),
        (
            "version 3.12.0 and U.S.A.",
            &["version", "3", "12", "0", "and", "U", "S", "A"],
        ),
        // Vowel signs (`Mc`/`Mn`) split a Devanagari word where
        // `char::is_alphanumeric` would keep it whole.
        ("नमस्ते", &["नमस", "त"]),
        // Enclosed alphanumerics are symbols (`So`), not letters.
        ("ⓐⓑⓒ x", &["x"]),
        ("a\u{345}b", &["a", "b"]),
    ];

    #[test]
    fn test_search() {
        let mut index = SearchIndex::new("en".to_string());
        index
            .add_document(
                "test1".to_string(),
                "test1.html".to_string(),
                "First Test".to_string(),
                "This is the first test document.",
            )
            .unwrap();
        index
            .add_document(
                "test2".to_string(),
                "test2.html".to_string(),
                "Second Test".to_string(),
                "This is the second test document with more content.",
            )
            .unwrap();

        let results = index.search("test document");
        assert!(!results.is_empty());
        assert!(results
            .iter()
            .any(|r| r.docname == "test1" || r.docname == "test2"));
    }

    #[test]
    fn test_search_index_builder() {
        let mut builder = SearchIndexBuilder::new("en".to_string());

        builder
            .add_or_update_document(
                "test".to_string(),
                "test.html".to_string(),
                "Test".to_string(),
                "Content",
            )
            .unwrap();

        let index = builder.build();
        assert_eq!(index.docnames.len(), 1);
    }

    #[test]
    fn direct_search_freeze_preserves_domain_objects() {
        let mut index = SearchIndex::new("en".to_string());
        index
            .add_document(
                "index".to_string(),
                "index.rst".to_string(),
                "Welcome".to_string(),
                "Body",
            )
            .unwrap();
        index
            .add_object(
                "Thing".to_string(),
                "index",
                Some("thing".to_string()),
                "py:function",
                Some("Thing".to_string()),
            )
            .unwrap();

        let value = index.to_sphinx_value();
        assert_eq!(value["objtypes"]["0"], "py:function");
        assert_eq!(
            value["objnames"]["0"],
            serde_json::json!(["py", "function", "Python function"])
        );
        assert_eq!(
            value["objects"][""],
            serde_json::json!([[0, 0, 1, "thing", "Thing"]])
        );
    }

    #[test]
    fn direct_search_drops_unsearchable_std_types_and_defaults_bare_types_to_std() {
        let mut index = SearchIndex::new("en".to_string());
        index
            .add_document(
                "index".to_string(),
                "index.rst".to_string(),
                "Welcome".to_string(),
                "Body",
            )
            .unwrap();
        index
            .add_object("Zebra".to_string(), "index", None, "term", None)
            .unwrap();
        index
            .add_object(
                "MYVAR".to_string(),
                "index",
                Some("envvar-MYVAR".to_string()),
                "envvar",
                None,
            )
            .unwrap();
        index
            .add_object(
                "pkg".to_string(),
                "index",
                Some("module-pkg".to_string()),
                "py:module",
                None,
            )
            .unwrap();

        let value = index.to_sphinx_value();
        // A glossary term has `searchprio=-1` and is not listed; the bare
        // `envvar` is a std type at priority 1; a module is priority 0 and
        // its `module-pkg` anchor abbreviates to `-`.
        assert_eq!(
            value["objects"][""],
            serde_json::json!([[0, 0, 1, "-", "MYVAR"], [0, 1, 0, "-", "pkg"]])
        );
        assert_eq!(
            value["objtypes"],
            serde_json::json!({"0": "std:envvar", "1": "py:module"})
        );
        assert_eq!(
            value["objnames"]["0"],
            serde_json::json!(["std", "envvar", "environment variable"])
        );
        assert_eq!(
            value["objnames"]["1"],
            serde_json::json!(["py", "module", "Python module"])
        );
    }

    /// The builder's read, merge and resolve steps in miniature: parse every
    /// `(docname, source)` into one environment that knows each document's
    /// title, metadata and toc, then feed them all. Reading everything
    /// first matters — a toctree resolves against the tocs of the documents
    /// it names.
    fn feed_project(docs: &[(&str, &str)], env: &mut BuildEnvironment) -> SphinxIndexBuilder {
        use crate::env::toctree as env_toctree;

        let found: BTreeSet<String> = docs.iter().map(|(name, _)| (*name).to_string()).collect();
        let mut read = Vec::new();
        for (docname, source) in docs {
            let doctree = crate::rst::parse_rst(
                source,
                &crate::rst::ParseOptions {
                    source_path: format!("{docname}.rst"),
                    sphinx: true,
                    docname: (*docname).to_string(),
                    found_docs: Some(std::sync::Arc::new(found.clone())),
                    ..Default::default()
                },
            );
            let title = env_toctree::document_title(&doctree);
            env.longtitles.insert((*docname).to_string(), title.clone());
            env.titles.insert((*docname).to_string(), title.clone());
            env.metadata.insert(
                (*docname).to_string(),
                crate::env::metadata::document_metadata(&doctree),
            );
            let (toc, _) = env_toctree::build_toc(&doctree, docname);
            for toctree in env_toctree::toctree_copies(&toc) {
                env_toctree::note_toctree(env, docname, toctree);
            }
            env.tocs.insert((*docname).to_string(), toc);
            env.all_docs.insert((*docname).to_string(), 0);
            read.push((*docname, doctree, title.astext()));
        }

        let mut builder = SphinxIndexBuilder::new();
        for (docname, doctree, title) in &read {
            builder.feed(docname, &format!("{docname}.rst"), title, doctree, env);
        }
        builder
    }

    #[test]
    fn feed_collects_titles_like_sphinx() {
        let mut env = BuildEnvironment::default();
        let builder = feed_project(
            &[
                (
                    "index",
                    "Welcome Guide\n=============\n\n\
                     .. toctree::\n   :caption: Contents\n\n   child\n\n\
                     .. toctree::\n   :hidden:\n   :caption: Hidden Caption\n\n   hidden\n\n\
                     .. secretword in a comment\n\n\
                     Installation Guide\n------------------\n\n\
                     Some text. You can not do this.\n\n\
                     Contents\n~~~~~~~~\n\nMore.\n",
                ),
                (
                    "child",
                    "Child Title\n===========\n\nChild body.\n\nChild Section\n-------------\n\nDeep.\n",
                ),
                ("hidden", "Hidden Doc\n==========\n\nhiddenword.\n"),
            ],
            &mut env,
        );
        let value = builder.freeze(&env);

        assert_eq!(
            value["docnames"],
            serde_json::json!(["child", "hidden", "index"])
        );
        assert_eq!(
            value["titles"],
            serde_json::json!(["Child Title", "Hidden Doc", "Welcome Guide"])
        );
        // Section titles and the caption are titles; the hidden toctree's
        // caption is not; the duplicate heading lists in document order.
        assert_eq!(
            value["alltitles"],
            serde_json::json!({
                "Child Section": [[0, "child-section"]],
                "Child Title": [[0, null]],
                "Contents": [[2, null], [2, "contents"]],
                "Hidden Doc": [[1, null]],
                "Installation Guide": [[2, "installation-guide"]],
                "Welcome Guide": [[2, null]],
            })
        );
        assert_eq!(
            value["titleterms"],
            serde_json::json!({
                "child": 0, "content": 2, "doc": 1, "guid": 2, "hidden": 1,
                "instal": 2, "section": 0, "titl": 0, "welcom": 2,
            })
        );
        let terms = value["terms"].as_object().unwrap();
        // Title words are not body terms for their own document, child
        // titles are body text of the parent through the resolved toctree,
        // and the comment is not indexed.
        assert!(!terms.contains_key("welcom"));
        assert_eq!(terms["child"], serde_json::json!(2));
        assert_eq!(terms["section"], serde_json::json!(2));
        assert!(!terms.contains_key("secretword"));
        // The hidden document is indexed itself; only its parent's toctree
        // does not mention it.
        assert_eq!(terms["hiddenword"], serde_json::json!(1));
        assert!(!terms.contains_key("hidden"));
        // `Some`, `You` and `can` survive as unstemmed words; `not`, `do`
        // and `this` are stopwords.
        assert_eq!(terms["Some"], serde_json::json!(2));
        assert_eq!(terms["You"], serde_json::json!(2));
        assert_eq!(terms["can"], serde_json::json!(2));
        assert!(!terms.contains_key("not"));
        assert!(!terms.contains_key("thi"));
    }

    #[test]
    fn nosearch_documents_keep_their_slot_and_contribute_nothing() {
        let mut env = BuildEnvironment::default();
        let builder = feed_project(
            &[
                (
                    "hidden",
                    ":nosearch:\n\nHidden Doc\n==========\n\nhiddenword text.\n",
                ),
                (
                    "shown",
                    ":tocdepth: 1\n\nShown Doc\n=========\n\nshownword text.\n",
                ),
            ],
            &mut env,
        );
        let value = builder.freeze(&env);
        assert_eq!(value["docnames"], serde_json::json!(["hidden", "shown"]));
        assert_eq!(value["titles"], serde_json::json!(["", "Shown Doc"]));
        assert_eq!(
            value["alltitles"],
            serde_json::json!({"Shown Doc": [[1, null]]})
        );
        // The leading field list is metadata, not body text.
        assert_eq!(
            value["terms"],
            serde_json::json!({"shownword": 1, "text": 1})
        );
    }

    #[test]
    fn index_entries_are_split_like_split_index_msg() {
        let mut env = BuildEnvironment::default();
        env.index_entries.insert(
            "index".to_string(),
            vec![
                crate::env::IndexEntryRecord {
                    entry_type: "pair".to_string(),
                    value: "alpha; beta".to_string(),
                    target_id: "index-0".to_string(),
                    main: false,
                    category_key: None,
                },
                crate::env::IndexEntryRecord {
                    entry_type: "single".to_string(),
                    value: "Zebra".to_string(),
                    target_id: "term-Zebra".to_string(),
                    main: true,
                    category_key: None,
                },
                crate::env::IndexEntryRecord {
                    entry_type: "pair".to_string(),
                    value: "lonely".to_string(),
                    target_id: "index-1".to_string(),
                    main: false,
                    category_key: None,
                },
            ],
        );
        let builder = feed_project(&[("index", "T\n=\n\nBody.\n")], &mut env);
        let value = builder.freeze(&env);
        assert_eq!(
            value["indexentries"],
            serde_json::json!({
                "alpha": [[0, "index-0", false]],
                "beta": [[0, "index-0", false]],
                "zebra": [[0, "term-Zebra", true]],
            })
        );
    }
}
