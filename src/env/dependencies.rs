//! `env.dependencies[docname]` — the files a document is built from besides
//! its own source, and the reason a build re-reads a document whose source
//! never changed.
//!
//! Port of the `note_dependency` half of Sphinx's asset collectors
//! (`environment/collectors/asset.py:26-104`, `ImageCollector.process_doc`)
//! plus the path arithmetic it relies on,
//! `BuildEnvironment.relfn2path` (`environment/__init__.py:378-400`).
//! [`crate::env::BuildEnvironment::get_outdated_files`] is the consumer: a
//! dependency that is missing, or newer than the time the document was
//! read, makes the document outdated.
//!
//! **Scope.** Sphinx notes a dependency for every file-inserting construct:
//! images, `figure`, `literalinclude`, `include`, `download`, `docutils.conf`
//! and the gettext catalogs. Of those, images (`figure` builds one) are the
//! only ones this crate parses today — `include`/`literalinclude` are wave
//! 4.5 — so an image `uri` is the whole population, and adding the rest is
//! a matter of calling [`note`] from wherever those nodes are collected.
//!
//! Deliberate omissions, each of which only costs a *warning*, never a
//! wrong rebuild decision:
//!
//! * `image file not readable: %s` — Sphinx stats the image at read time
//!   and warns; here a missing image simply makes the document outdated on
//!   every build (the honest consequence of "a dependency is missing"), and
//!   nothing is said about it.
//! * `candidates` — Sphinx expands `foo.*` into per-mimetype candidates and
//!   notes each. Nothing in this crate resolves candidates, so a `*` uri is
//!   noted as no dependency at all rather than as one file that does not
//!   exist and would peg the document to a re-read forever.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::doctree::{kinds, AttrValue, Doctree, Node};

use super::BuildEnvironment;

/// Record every file `docname`'s doctree pulls in, replacing whatever the
/// previous read of that document left behind.
///
/// Mirrors Sphinx's collector contract: the entry is dropped entirely when
/// the document depends on nothing, so `dependencies` holds only documents
/// that have dependencies (Sphinx's `defaultdict(set)` behaves the same way
/// after `clear_doc`).
pub fn process_doc(env: &mut BuildEnvironment, docname: &str, doctree: &Doctree, srcdir: &Path) {
    let mut paths = BTreeSet::new();
    collect(&doctree.root, docname, srcdir, &mut paths);
    if paths.is_empty() {
        env.dependencies.remove(docname);
    } else {
        env.dependencies.insert(docname.to_string(), paths);
    }
}

/// `env.note_dependency(filename)`: the absolute path a `uri` seen in
/// `docname` refers to, or `None` when the uri names no local file.
///
/// Skipped, exactly as `ImageCollector.process_doc` skips them: `data:`
/// URIs (the image is inline), anything with a scheme (`http://…`), and
/// candidate patterns (see the module note).
pub fn note(uri: &str, docname: &str, srcdir: &Path) -> Option<PathBuf> {
    if uri.starts_with("data:") || uri.contains("://") || uri.contains('*') || uri.is_empty() {
        return None;
    }
    Some(relfn2path(uri, docname, srcdir))
}

/// `BuildEnvironment.relfn2path` (`environment/__init__.py:378-400`): a
/// filename written in a document resolves relative to that document's
/// directory, unless it is written absolute (`/pic.png`), in which case it
/// is relative to the source directory. The result is normalized (`.` and
/// `..` collapsed, Sphinx's `os.path.normpath`) and joined onto srcdir.
fn relfn2path(uri: &str, docname: &str, srcdir: &Path) -> PathBuf {
    let relative = match uri.strip_prefix('/') {
        Some(rooted) => rooted.to_string(),
        None => match docname.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/{uri}"),
            None => uri.to_string(),
        },
    };

    let mut segments: Vec<&str> = Vec::new();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                // `normpath` only drops a `..` that has something to undo;
                // a leading one stays and walks out of the source tree,
                // which is a path that simply will not exist.
                if matches!(segments.last(), Some(&last) if last != "..") {
                    segments.pop();
                } else {
                    segments.push("..");
                }
            }
            other => segments.push(other),
        }
    }

    let mut path = srcdir.to_path_buf();
    for segment in segments {
        path.push(segment);
    }
    path
}

/// Walk the doctree for every node that names a file. `figure` needs no
/// case of its own: it holds the `image` node this matches.
fn collect(node: &Node, docname: &str, srcdir: &Path, out: &mut BTreeSet<PathBuf>) {
    if node.kind == kinds::IMAGE {
        if let Some(AttrValue::Str(uri)) = node.get("uri") {
            if let Some(path) = note(uri, docname, srcdir) {
                out.insert(path);
            }
        }
    }
    for child in &node.children {
        collect(child, docname, srcdir, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rst;

    fn deps(docname: &str, source: &str) -> Vec<PathBuf> {
        let doctree = rst::parse_rst(source, &rst::ParseOptions::default());
        let mut env = BuildEnvironment::default();
        process_doc(&mut env, docname, &doctree, Path::new("/src"));
        env.dependencies
            .get(docname)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn an_image_uri_resolves_against_the_documents_own_directory() {
        assert_eq!(
            deps("chapters/intro", ".. image:: pic.png\n"),
            vec![PathBuf::from("/src/chapters/pic.png")]
        );
        assert_eq!(
            deps("index", ".. image:: img/pic.png\n"),
            vec![PathBuf::from("/src/img/pic.png")]
        );
    }

    #[test]
    fn a_rooted_uri_resolves_against_the_source_directory() {
        assert_eq!(
            deps("chapters/intro", ".. image:: /img/pic.png\n"),
            vec![PathBuf::from("/src/img/pic.png")]
        );
    }

    #[test]
    fn dot_segments_are_normalized_away() {
        assert_eq!(
            deps("chapters/intro", ".. image:: ../img/./pic.png\n"),
            vec![PathBuf::from("/src/img/pic.png")]
        );
    }

    #[test]
    fn a_figures_image_is_a_dependency_too() {
        assert_eq!(
            deps("index", ".. figure:: pic.png\n\n   A caption.\n"),
            vec![PathBuf::from("/src/pic.png")]
        );
    }

    #[test]
    fn remote_inline_and_candidate_uris_are_not_local_files() {
        assert!(deps("index", ".. image:: https://example.com/pic.png\n").is_empty());
        assert!(deps("index", ".. image:: data:image/png;base64,iVBOR\n").is_empty());
        assert!(
            deps("index", ".. image:: pic.*\n").is_empty(),
            "a candidate pattern names no single file; noting it would peg \
             the document to a re-read on every build"
        );
    }

    #[test]
    fn a_document_without_files_has_no_entry_at_all() {
        let doctree = rst::parse_rst("Title\n=====\n", &rst::ParseOptions::default());
        let mut env = BuildEnvironment::default();
        env.dependencies
            .insert("index".to_string(), BTreeSet::from([PathBuf::from("/old")]));

        process_doc(&mut env, "index", &doctree, Path::new("/src"));

        assert!(
            !env.dependencies.contains_key("index"),
            "a re-read that finds no dependencies must drop the stale ones"
        );
    }

    #[test]
    fn several_images_all_count_once() {
        assert_eq!(
            deps(
                "index",
                ".. image:: a.png\n\n.. image:: b.png\n\n.. image:: a.png\n"
            ),
            vec![PathBuf::from("/src/a.png"), PathBuf::from("/src/b.png")]
        );
    }
}
