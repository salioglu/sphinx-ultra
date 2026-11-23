//! `env.dependencies[docname]` — the files a document is built from besides
//! its own source, and the reason a build re-reads a document whose source
//! never changed.
//!
//! Port of Sphinx's `DependenciesCollector`
//! (`SP/sphinx/environment/collectors/dependencies.py`), the first
//! `doctree-read` collector: the files docutils recorded while it parsed
//! the document (`settings.record_dependencies`), here the parse's records
//! ([`crate::rst::RegistryExport::dependencies`]), each noted as
//! `env.note_dependency` notes it (`SP/sphinx/environment/__init__.py:605-618`).
//! The image collector notes each image after it
//! ([`crate::env::collectors::images`]).
//! [`crate::env::BuildEnvironment::get_outdated_files`] is the consumer: a
//! dependency that is missing, or newer than the time the document was
//! read, makes the document outdated.
//!
//! **Scope.** Sphinx notes a dependency for every file-inserting construct:
//! images, `figure`, `literalinclude`, `include`, `download`, `docutils.conf`
//! and the gettext catalogs. This crate covers images and — since wave 4.5 —
//! `include`. The remaining producers are a matter of extending the record
//! stream from wherever their nodes are collected.

use std::path::Path;

use super::BuildEnvironment;

/// `DependenciesCollector.process_doc`: notes every file the parse of
/// `docname` recorded, a path relative to the source directory `srcdir`
/// (a leading `..` walks outside it, like Sphinx's `srcdir / relpath` for
/// an out-of-tree include), as `env.note_dependency` notes it — the
/// document's entry, which the read's `clear_doc` emptied, holds only
/// documents that depend on something (Sphinx's `setdefault`).
pub fn process_doc(
    env: &mut BuildEnvironment,
    docname: &str,
    srcdir: &Path,
    parse_dependencies: &[String],
) {
    for dependency in parse_dependencies {
        env.dependencies
            .entry(docname.to_string())
            .or_default()
            .insert(srcdir.join(dependency));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The parse-recorded dependencies are noted srcdir-joined; a document
    /// that recorded none gets no entry.
    #[test]
    fn parse_recorded_dependencies_are_noted_against_the_source_directory() {
        let mut env = BuildEnvironment::default();
        process_doc(
            &mut env,
            "index",
            Path::new("/src"),
            &["part.rst".to_string(), "data/snippet.txt".to_string()],
        );
        assert_eq!(
            env.dependencies["index"],
            BTreeSet::from([
                Path::new("/src").join("data/snippet.txt"),
                Path::new("/src").join("part.rst"),
            ])
        );

        process_doc(&mut env, "other", Path::new("/src"), &[]);
        assert!(
            !env.dependencies.contains_key("other"),
            "a document without dependencies has no entry at all"
        );
    }
}
