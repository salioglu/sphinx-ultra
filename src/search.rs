use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
                    .or_insert_with(Vec::new)
                    .push(doc_match);
            }
        }

        Ok(())
    }

    /// Extract words and their positions from content
    fn extract_words(&self, content: &str) -> HashMap<String, Vec<usize>> {
        let mut words = HashMap::new();
        let mut position = 0;

        for word in content.split_whitespace() {
            let cleaned_word = self.clean_word(word);
            if !cleaned_word.is_empty() {
                words
                    .entry(cleaned_word)
                    .or_insert_with(Vec::new)
                    .push(position);
            }
            position += 1;
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
        #[derive(Serialize)]
        struct JsonSearchIndex<'a> {
            docnames: &'a Vec<String>,
            filenames: &'a Vec<String>,
            titles: &'a Vec<String>,
            terms: &'a HashMap<String, Vec<DocumentMatch>>,
            objects: &'a HashMap<String, ObjectReference>,
            objnames: &'a HashMap<String, String>,
            objtypes: &'a HashMap<String, String>,
        }

        let json_index = JsonSearchIndex {
            docnames: &self.docnames,
            filenames: &self.filenames,
            titles: &self.titles,
            terms: &self.terms,
            objects: &self.objects,
            objnames: &self.objnames,
            objtypes: &self.objtypes,
        };

        Ok(serde_json::to_string(&json_index)?)
    }
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
        assert!(results.len() >= 1);
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
}
