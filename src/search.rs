use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::doctree::{Doctree, Node};
use crate::document::Document;
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

        for (word, positions) in words {
            let normalized_word = self.normalize_word(&word);
            if !normalized_word.is_empty() && normalized_word.len() >= 2 {
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
        }

        Ok(())
    }

    /// Extract words and their positions from content
    fn extract_words(&self, content: &str) -> HashMap<String, Vec<usize>> {
        let mut words = HashMap::new();

        for (position, word) in content.split_whitespace().enumerate() {
            let cleaned_word = self.clean_word(word);
            if !cleaned_word.is_empty() {
                words
                    .entry(cleaned_word)
                    .or_insert_with(Vec::new)
                    .push(position);
            }
        }

        words
    }

    /// Clean a word by removing punctuation
    fn clean_word(&self, word: &str) -> String {
        word.chars()
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect::<String>()
            .to_lowercase()
    }

    /// Normalize a word for indexing
    fn normalize_word(&self, word: &str) -> String {
        // Apply language-specific normalization
        match self.language.as_str() {
            "en" => self.normalize_english(word),
            _ => word.to_lowercase(),
        }
    }

    /// English-specific word normalization (basic stemming)
    fn normalize_english(&self, word: &str) -> String {
        let word = word.to_lowercase();

        // Very basic stemming - remove common suffixes
        if word.ends_with("ing") && word.len() > 4 {
            word[..word.len() - 3].to_string()
        } else if word.ends_with("ed") && word.len() > 3 {
            word[..word.len() - 2].to_string()
        } else if word.ends_with("s") && word.len() > 2 {
            word[..word.len() - 1].to_string()
        } else {
            word
        }
    }

    /// Search for documents matching a query
    pub fn search(&self, query: &str) -> Vec<SearchResult> {
        let query_terms: Vec<String> = query
            .split_whitespace()
            .map(|term| self.normalize_word(&self.clean_word(term)))
            .filter(|term| !term.is_empty())
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
            for word in title.split_ascii_whitespace() {
                let stem = stem_english(word);
                if !stem.is_empty() {
                    titleterms.entry(stem).or_default().insert(index);
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

/// Build the complete Sphinx search index from the live builder state.
pub fn build_sphinx_index(
    documents: &[Document],
    doctrees: &[Doctree],
    env: &BuildEnvironment,
    source_dir: &Path,
) -> Value {
    let mut records: Vec<(&Document, &Doctree, String)> = documents
        .iter()
        .zip(doctrees)
        .map(|(document, doctree)| {
            (
                document,
                doctree,
                docname_from_path(&document.source_path, source_dir),
            )
        })
        .collect();
    records.sort_by(|a, b| a.2.cmp(&b.2));

    let docnames: Vec<String> = records.iter().map(|(_, _, name)| name.clone()).collect();
    let doc_indices: BTreeMap<String, usize> = docnames
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect();
    let filenames: Vec<String> = records
        .iter()
        .map(|(document, _, _)| {
            document
                .source_path
                .strip_prefix(source_dir)
                .unwrap_or(&document.source_path)
                .to_string_lossy()
                .to_string()
        })
        .collect();
    let titles: Vec<String> = records
        .iter()
        .map(|(document, _, _)| document.title.clone())
        .collect();
    let documents_by_docname: BTreeMap<String, &Document> = records
        .iter()
        .map(|(document, _, docname)| (docname.clone(), *document))
        .collect();

    let mut terms = BTreeMap::<String, BTreeSet<usize>>::new();
    let mut titleterms = BTreeMap::<String, BTreeSet<usize>>::new();
    let mut alltitles = BTreeMap::<String, Vec<(usize, Option<String>)>>::new();

    for (index, (document, doctree, docname)) in records.iter().enumerate() {
        let body_text = search_text(&doctree.root);
        let document_title_terms: BTreeSet<String> = search_words(&document.title)
            .into_iter()
            .filter_map(|word| search_term(&word))
            .collect();
        for word in search_words(&body_text) {
            if let Some(term) = search_term(&word) {
                // Sphinx's WordCollector does not add a body occurrence when
                // the same stem is already indexed by this document's title.
                if !document_title_terms.contains(&term) {
                    terms.entry(term).or_default().insert(index);
                }
            }
        }
        let mut toctree_text = String::new();
        collect_toctree_search_text(
            docname,
            &documents_by_docname,
            env,
            &mut BTreeSet::new(),
            &mut toctree_text,
        );
        for word in search_words(&toctree_text) {
            if let Some(term) = search_term(&word) {
                terms.entry(term).or_default().insert(index);
            }
        }
        for term in document_title_terms {
            titleterms.entry(term).or_default().insert(index);
        }
        for toctree in &document.toctrees {
            if let Some(caption) = &toctree.caption {
                alltitles
                    .entry(caption.clone())
                    .or_default()
                    .push((index, None));
                for word in search_words(caption) {
                    if let Some(term) = search_term(&word) {
                        titleterms.entry(term).or_default().insert(index);
                    }
                }
            }
        }

        let mut titles_in_doc = Vec::new();
        collect_titles(&doctree.root, 0, &mut titles_in_doc);
        if titles_in_doc.is_empty() {
            titles_in_doc.push((document.title.clone(), None));
        }
        for (title, title_id) in titles_in_doc {
            alltitles.entry(title).or_default().push((index, title_id));
        }

        // A document's index records are already in Sphinx's document order;
        // freeze only has to translate docnames to the sorted numeric index.
        let _ = docname;
    }

    let (objects, objtypes, objnames) = search_objects(env, &doc_indices);
    let mut indexentries = BTreeMap::<String, Vec<(usize, String, bool)>>::new();
    for (docname, entries) in &env.index_entries {
        let Some(&doc_index) = doc_indices.get(docname) else {
            continue;
        };
        for entry in entries {
            indexentries
                .entry(entry.value.to_lowercase())
                .or_default()
                .push((doc_index, entry.target_id.clone(), entry.main));
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

fn collect_toctree_search_text(
    docname: &str,
    documents: &BTreeMap<String, &Document>,
    env: &BuildEnvironment,
    seen: &mut BTreeSet<String>,
    out: &mut String,
) {
    if !seen.insert(docname.to_string()) {
        return;
    }
    let Some(document) = documents.get(docname) else {
        return;
    };
    for toctree in &document.toctrees {
        // Glob entries are expanded by the environment for navigation. Their
        // raw patterns are not doctree text, but Sphinx does index the
        // resolved child titles; use the expanded include list so a dead
        // pattern contributes nothing while `pages/*` contributes Alpha and
        // Beta rather than the literal pattern or `missing`.
        if toctree.glob {
            if let Some(includes) = env.toctree_includes.get(docname) {
                for target in includes {
                    if let Some(title) = env.titles.get(target) {
                        out.push(' ');
                        out.push_str(&crate::env::numbers::clean_astext(title));
                    }
                    collect_toctree_search_text(target, documents, env, seen, out);
                }
            }
            continue;
        }
        for entry in &toctree.entries {
            let joined = crate::env::toctree::docname_join(docname, &entry.target);
            let target = joined.strip_suffix(".rst").unwrap_or(&joined).to_string();
            let title = entry.title.clone().or_else(|| {
                env.titles
                    .get(&target)
                    .map(crate::env::numbers::clean_astext)
            });
            if let Some(title) = title {
                out.push(' ');
                out.push_str(&title);
            }
            collect_toctree_search_text(&target, documents, env, seen, out);
        }
    }
}

/// Render the JavaScript wrapper used by `sphinx.search.js_index`.
pub fn dumps_sphinx_index(value: &Value) -> Result<String> {
    Ok(format!(
        "Search.setIndex({})",
        serde_json::to_string(value)?
    ))
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

fn docname_from_path(path: &Path, source_dir: &Path) -> String {
    path.strip_prefix(source_dir)
        .unwrap_or(path)
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/")
}

fn search_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() || character == '_' {
            current.push(character);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn search_term(word: &str) -> Option<String> {
    if word.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let stemmed = stem_english(word);
    if is_english_stopword(&stemmed) {
        return (!is_english_stopword(word)).then(|| word.to_string());
    }
    (!stemmed.is_empty()).then_some(stemmed)
}

// Sphinx 9.1 delegates English stemming to snowballstemmer's complete
// Snowball algorithm. Keep this small fallback local for now: the basic
// search-index contract is covered, but words outside these rules can differ
// from Sphinx until Ultra either ports Snowball or adds a compatible dependency.
fn stem_english(word: &str) -> String {
    let mut word = word.to_lowercase();
    if word.ends_with("ational") || (word.ends_with("ation") && word.len() > 6) {
        word.truncate(word.len() - 5);
    } else if word.ends_with("ing") && word.len() > 4 {
        word.truncate(word.len() - 3);
    } else if word.ends_with("ed") && word.len() > 3 {
        word.truncate(word.len() - 2);
    } else if (word.ends_with('s') && word.len() > 2) || (word.ends_with('e') && word.len() > 4) {
        word.pop();
    }
    if word.ends_with("ll") {
        word.pop();
    }
    word
}

fn is_english_stopword(word: &str) -> bool {
    matches!(
        word,
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "by"
            | "for"
            | "from"
            | "in"
            | "is"
            | "it"
            | "of"
            | "on"
            | "or"
            | "that"
            | "the"
            | "this"
            | "to"
            | "was"
            | "were"
            | "with"
            | "some"
    )
}

fn search_text(node: &Node) -> String {
    if node.kind == crate::doctree::kinds::TITLE {
        return String::new();
    }
    if let Some(text) = &node.text {
        return text.clone();
    }
    let text = node
        .children
        .iter()
        .map(search_text)
        .collect::<Vec<_>>()
        .join(" ");
    text
}

fn collect_titles(node: &Node, depth: usize, out: &mut Vec<(String, Option<String>)>) {
    for child in &node.children {
        if child.kind == crate::doctree::kinds::TITLE {
            // The first title is the document title. Sphinx omits only that
            // title's ID; section titles retain their first-level anchors.
            let id = if !out.is_empty() && depth > 0 {
                node.attrs.ids.first().cloned()
            } else {
                None
            };
            out.push((child.astext(), id));
        }
        collect_titles(child, depth + 1, out);
    }
}

type SearchObjects = (
    BTreeMap<String, Vec<[Value; 5]>>,
    BTreeMap<String, String>,
    BTreeMap<String, [String; 3]>,
);

fn direct_search_objects(index: &SearchIndex) -> SearchObjects {
    let mut references: Vec<&ObjectReference> = index.objects.values().collect();
    references.sort_by(|left, right| {
        left.obj_type
            .cmp(&right.obj_type)
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut type_indices = BTreeMap::<String, usize>::new();
    let mut objects = BTreeMap::<String, Vec<[Value; 5]>>::new();
    let mut objtypes = BTreeMap::new();
    let mut objnames = BTreeMap::new();

    for object in references {
        let (domain, objtype) = object
            .obj_type
            .split_once(':')
            .unwrap_or(("std", object.obj_type.as_str()));
        let type_index = if let Some(index) = type_indices.get(&object.obj_type) {
            *index
        } else {
            let index = type_indices.len();
            type_indices.insert(object.obj_type.clone(), index);
            objtypes.insert(index.to_string(), object.obj_type.clone());
            objnames.insert(
                index.to_string(),
                [
                    domain.to_string(),
                    objtype.to_string(),
                    object_type_label(domain, objtype),
                ],
            );
            index
        };

        let (prefix, name) = object
            .name
            .rsplit_once('.')
            .map(|(prefix, name)| (prefix.to_string(), name.to_string()))
            .unwrap_or_else(|| (String::new(), object.name.clone()));
        let anchor = object.anchor.as_deref().unwrap_or_default();
        let shortanchor = if anchor == object.name {
            String::new()
        } else if anchor == format!("{objtype}-{}", object.name) {
            "-".to_string()
        } else {
            anchor.to_string()
        };
        objects.entry(prefix).or_default().push([
            Value::from(object.docname_idx),
            Value::from(type_index),
            Value::from(1),
            Value::String(shortanchor),
            Value::String(name),
        ]);
    }
    (objects, objtypes, objnames)
}

fn search_objects(env: &BuildEnvironment, doc_indices: &BTreeMap<String, usize>) -> SearchObjects {
    let mut rows = Vec::new();
    for ((program, option), (docname, anchor)) in &env.std.progoptions {
        let fullname = program
            .as_deref()
            .map(|program| format!("{program}.{option}"))
            .unwrap_or_else(|| option.clone());
        rows.push((
            "std",
            fullname.clone(),
            fullname,
            "cmdoption".to_string(),
            docname.clone(),
            anchor.clone(),
            1,
        ));
    }
    for ((objtype, name), (docname, anchor)) in &env.std.objects {
        let priority = match objtype.as_str() {
            "confval" | "envvar" => 1,
            _ => -1,
        };
        rows.push((
            "std",
            name.clone(),
            name.clone(),
            objtype.clone(),
            docname.clone(),
            anchor.clone(),
            priority,
        ));
    }
    for (name, module) in &env.py.modules {
        rows.push((
            "py",
            name.clone(),
            name.clone(),
            "module".to_string(),
            module.docname.clone(),
            module.node_id.clone(),
            0,
        ));
    }
    for (name, object) in &env.py.objects {
        rows.push((
            "py",
            name.clone(),
            name.clone(),
            object.objtype.clone(),
            object.docname.clone(),
            object.node_id.clone(),
            if object.aliased { -1 } else { 1 },
        ));
    }

    rows.sort_by(|a, b| {
        a.0.cmp(b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
            .then_with(|| a.3.cmp(&b.3))
            .then_with(|| a.4.cmp(&b.4))
            .then_with(|| a.5.cmp(&b.5))
            .then_with(|| a.6.cmp(&b.6))
    });

    let mut type_indices = BTreeMap::<(String, String), usize>::new();
    let mut objtypes = BTreeMap::new();
    let mut objnames = BTreeMap::new();
    let mut objects = BTreeMap::<String, Vec<[Value; 5]>>::new();
    for (domain, fullname, _dispname, objtype, docname, anchor, priority) in rows {
        if priority < 0 {
            continue;
        }
        let Some(&doc_index) = doc_indices.get(&docname) else {
            continue;
        };
        let key = (domain.to_string(), objtype.clone());
        let type_index = if let Some(index) = type_indices.get(&key) {
            *index
        } else {
            let index = type_indices.len();
            type_indices.insert(key.clone(), index);
            objtypes.insert(index.to_string(), format!("{}:{}", domain, objtype));
            objnames.insert(
                index.to_string(),
                [
                    domain.to_string(),
                    objtype.clone(),
                    object_type_label(domain, &objtype),
                ],
            );
            index
        };

        let (prefix, name) = fullname
            .rsplit_once('.')
            .map(|(prefix, name)| (prefix.to_string(), name.to_string()))
            .unwrap_or_else(|| (String::new(), fullname.clone()));
        let shortanchor = if anchor == fullname {
            String::new()
        } else if anchor == format!("{}-{}", objtype, fullname) {
            "-".to_string()
        } else {
            anchor
        };
        objects.entry(prefix).or_default().push([
            Value::from(doc_index),
            Value::from(type_index),
            Value::from(priority),
            Value::String(shortanchor),
            Value::String(name),
        ]);
    }
    (objects, objtypes, objnames)
}

fn object_type_label(domain: &str, objtype: &str) -> String {
    if domain == "std" {
        return match objtype {
            "cmdoption" => "program option",
            "confval" => "configuration value",
            "envvar" => "environment variable",
            "term" => "glossary term",
            "token" => "grammar token",
            "label" => "reference label",
            "doc" => "document",
            _ => objtype,
        }
        .to_string();
    }
    match objtype {
        "classmethod" => "class method",
        "staticmethod" => "static method",
        "type" => "type alias",
        _ => objtype,
    }
    .to_string()
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
        let index = SearchIndex::new("en".to_string());

        assert_eq!(index.normalize_english("running"), "runn");
        assert_eq!(index.normalize_english("walked"), "walk");
        assert_eq!(index.normalize_english("tests"), "test");
        assert_eq!(index.normalize_english("test"), "test");
    }

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
            serde_json::json!(["py", "function", "function"])
        );
        assert_eq!(
            value["objects"][""],
            serde_json::json!([[0, 0, 1, "thing", "Thing"]])
        );
    }
}
