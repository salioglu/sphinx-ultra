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

    fn term_for_word(&self, word: &str) -> Option<String> {
        match self.language.as_str() {
            "en" => search_term(word),
            _ => (!word.is_empty()).then(|| word.to_lowercase()),
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
                if let Some(stem) = search_term(&word) {
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
    if !word.is_empty() && word.chars().all(is_sphinx_digit) {
        return None;
    }
    let stemmed = stem_english(word);
    if is_english_stopword(&stemmed) {
        return None;
    }
    (!stemmed.is_empty()).then_some(stemmed)
}

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
    ENGLISH_STOPWORDS.contains(&word)
}

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
            if start >= self.region1_start
                && !matches!(self.as_str().as_str(), "succeed" | "proceed" | "exceed")
            {
                self.replace_suffix(suffix, "ee");
            }
            return;
        }
        let Some(suffix) = self.matching_suffix(&["ingly", "edly", "ing", "ed"]) else {
            return;
        };
        let start = self.suffix_start(suffix).unwrap();
        if matches!(suffix, "ing" | "ingly") {
            let special = ["even", "cann", "inn", "earr", "herr", "out"];
            let prefix = self.chars[..start].iter().collect::<String>();
            if special.iter().any(|word| *word == prefix) {
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
    fn search_drops_words_whose_stems_are_stopwords_in_both_paths() {
        let words = ["all", "underlying", "others", "ifs", "mostly"];
        for word in words {
            assert_eq!(
                search_term(word),
                None,
                "Sphinx drops the stopword stem for {word:?}"
            );
        }

        let mut index = SearchIndex::new("en".to_string());
        index
            .add_document(
                "test".to_string(),
                "test.html".to_string(),
                words.join(" "),
                &words.join(" "),
            )
            .unwrap();
        for word in words {
            assert!(!index.terms.contains_key(word), "indexed {word:?}");
        }
        let frozen_index = index.to_sphinx_value();
        let frozen_terms = frozen_index["terms"]
            .as_object()
            .expect("frozen terms must be an object");
        for word in words {
            assert!(!frozen_terms.contains_key(word), "frozen {word:?}");
        }
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
        let headers: Vec<_> = content.lines().take(8).collect();
        assert!(headers.contains(&"# sphinx=9.1.0"));
        assert!(headers.contains(&"# docutils=0.22.4"));
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
    fn english_search_matches_sphinx_stopwords_and_splitter() {
        assert_eq!(ENGLISH_STOPWORDS.len(), 174);
        for stopword in ENGLISH_STOPWORDS {
            assert!(
                is_english_stopword(stopword),
                "missing Sphinx stopword {stopword:?}"
            );
        }
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
        let cases = [
            ("the", None),
            ("only", Some("onli")),
            ("'s", Some("'s")),
            ("dogs'", Some("dog")),
            ("children's", Some("children")),
            ("'quoted", Some("quot")),
            ("O'Reilly", Some("o'reilli")),
            ("½", Some("½")),
            ("Ⅳ", Some("ⅳ")),
            ("¹", None),
        ];
        for (word, expected) in cases {
            assert_eq!(
                search_term(word).as_deref(),
                expected,
                "term mismatch for {word:?}"
            );
        }
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
            "á é C++ Rust's _private",
            &["a", "e", "C", "Rust", "s", "_private"],
        ),
        (
            "version 3.12.0 and U.S.A.",
            &["version", "3", "12", "0", "and", "U", "S", "A"],
        ),
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
            serde_json::json!(["py", "function", "function"])
        );
        assert_eq!(
            value["objects"][""],
            serde_json::json!([[0, 0, 1, "thing", "Thing"]])
        );
    }
}
