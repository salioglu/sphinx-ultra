//! `DownloadFileCollector` (`SP/sphinx/environment/collectors/asset.py:136-174`)
//! and `env.dlfiles`, the files the `download` role names, which a build
//! copies to `_downloads/` ([`DownloadFiles`]).

use std::collections::{BTreeMap, BTreeSet};

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use super::{in_srcdir, is_readable};
use crate::diagnostics::Category;
use crate::doctree::{kinds, AttrValue};
use crate::env::BuildEnvironment;
use crate::transforms::TransformCx;
use crate::utils::{relative_path_walk_up, relfn2path_io, resolve_path};

/// Sphinx's `DownloadFiles` (`SP/sphinx/util/_files.py:64-100`), the type
/// of `env.dlfiles`: each file a document offers for download, by its path
/// relative to the source directory (`/`-separated), with the documents
/// that offer it and where under `_downloads/` the builder copies it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadFiles {
    /// path → (docnames, destination).
    files: BTreeMap<String, (BTreeSet<String>, String)>,
}

impl DownloadFiles {
    /// `add_file(docname, filename)`: records that `docname` offers `file`
    /// and returns the file's destination, `<md5 of its path>/<its name>`
    /// (`hashlib.md5(filename.as_posix().encode())`), which a file keeps
    /// whoever else offers it.
    pub fn add_file(&mut self, docname: &str, file: &str) -> String {
        if let Some((docnames, destination)) = self.files.get_mut(file) {
            docnames.insert(docname.to_string());
            return destination.clone();
        }
        let name = file.rsplit('/').next().unwrap_or(file);
        let destination = format!("{:x}/{name}", Md5::digest(file.as_bytes()));
        self.files.insert(
            file.to_string(),
            (BTreeSet::from([docname.to_string()]), destination.clone()),
        );
        destination
    }

    /// `purge_doc(docname)`: forgets that `docname` offers any file, and
    /// every file no document offers any more.
    pub fn purge_doc(&mut self, docname: &str) {
        self.files.retain(|_, (docnames, _)| {
            docnames.remove(docname);
            !docnames.is_empty()
        });
    }

    /// The documents that offer `file` and its destination.
    pub fn get(&self, file: &str) -> Option<&(BTreeSet<String>, String)> {
        self.files.get(file)
    }

    /// Every file, with the documents that offer it and its destination.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &(BTreeSet<String>, String))> {
        self.files
            .iter()
            .map(|(file, entry)| (file.as_str(), entry))
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// `DownloadFileCollector.process_doc` for the document `cx` reads, folding
/// its downloads into `env`: every `download_reference` node in the
/// document's tree, in tree order (`doctree.findall(download_reference)`),
/// those in a `substitution_definition` included.
///
/// - A target with `://` in it is the node's `refuri`, and nothing else is
///   done with it.
/// - Any other names a file, as `env.relfn2path` resolves it
///   (`SP/sphinx/environment/__init__.py:454-478`): relative to the
///   document's directory or, rooted, to the source directory, its
///   symlinks followed and its `.` and `..` collapsed. The file is a
///   dependency of the document, by its path relative to the source
///   directory (`note_dependency(rel_filename)`). One the build can read
///   joins `env.dlfiles` ([`DownloadFiles::add_file`]), whose destination
///   the node takes as its `filename`; one it cannot read is logged,
///   `location=node`: `download file not readable: <absolute path>`
///   (`type='download', subtype='not_readable'`, [`TransformCx::warn`]).
pub fn process_doc(cx: &mut TransformCx<'_>, env: &mut BuildEnvironment) {
    let root = cx.tree.root();
    for reference in cx
        .tree
        .findall(root, |node| node.kind == kinds::DOWNLOAD_REFERENCE)
    {
        let Some(target) = cx
            .tree
            .node(reference)
            .str_attr("reftarget")
            .map(str::to_string)
        else {
            continue;
        };
        if target.contains("://") {
            cx.tree
                .node_mut(reference)
                .set("refuri", AttrValue::Str(target));
            continue;
        }
        let absolute = relfn2path_io(&target, cx.docname, cx.srcdir);
        let relative = relative_path_walk_up(&absolute, &resolve_path(cx.srcdir));
        env.dependencies
            .entry(cx.docname.to_string())
            .or_default()
            .insert(in_srcdir(cx.srcdir, &relative));
        if !is_readable(&absolute) {
            cx.warn(
                reference,
                format!("download file not readable: {}", absolute.display()),
                Category::from_parts(Some("download"), Some("not_readable")),
            );
            continue;
        }
        let destination = env.dlfiles.add_file(cx.docname, &relative);
        cx.tree
            .node_mut(reference)
            .set("filename", AttrValue::Str(destination));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use crate::read::{read_standalone, ReadSettings, StandaloneRead};
    use crate::rst::ParseOptions;

    /// A project in a temporary directory, `files` written under it, and
    /// its source directory as the build spells it: canonical, as Sphinx's
    /// `relfn2path` resolves the paths it reports.
    fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        for (path, text) in files {
            let file = tmp.path().join(path);
            std::fs::create_dir_all(file.parent().expect("a file has a parent")).expect("mkdir");
            std::fs::write(file, text).expect("write");
        }
        let srcdir = crate::utils::canonicalize_simplified(tmp.path()).expect("canonical");
        (tmp, srcdir)
    }

    /// `srcdir` joined with the `/`-separated `path`, one component at a
    /// time.
    fn under(srcdir: &Path, path: &str) -> PathBuf {
        path.split('/')
            .fold(srcdir.to_path_buf(), |dir, part| dir.join(part))
    }

    /// The document `docname` of the project at `srcdir`, read standalone.
    fn read_in(srcdir: &Path, docname: &str, source: &str) -> StandaloneRead {
        let parse = ParseOptions {
            source_path: under(srcdir, &format!("{docname}.rst"))
                .display()
                .to_string(),
            sphinx: true,
            docname: docname.to_string(),
            srcdir: Some(srcdir.to_path_buf()),
            ..ParseOptions::default()
        };
        read_standalone(source, &parse, &ReadSettings::sphinx_defaults())
    }

    /// The read's post-read pformat, less its document line.
    fn body(read: &StandaloneRead) -> String {
        let pformat = read.outcome.doctree().root.pformat();
        let (_, body) = pformat.split_once('\n').expect("a document line");
        body.to_string()
    }

    fn rendered(read: &StandaloneRead) -> Vec<String> {
        read.diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(true))
            .collect()
    }

    /// P02's `r_download` ([RC §3.8]).
    const R_DOWNLOAD: &str = ":download:`f.txt`\n\n:download:`the file <f.txt>`\n\n\
                              :download:`sub/g.txt`\n\n:download:`/f.txt`\n\n\
                              :download:`missing.txt`\n\n:download:`https://example.com/x.zip`\n\n\
                              :download:`!f.txt`\n\n:download:`f  .txt`\n";

    /// `DownloadFileCollector.process_doc` ([RC §3.8], the collector's
    /// half), P02's `r_download` read as the document `a` of a project
    /// holding `f.txt` and `sub/g.txt`: a target with `://` is the
    /// reference's `refuri`; any other is a path, relative to the
    /// document's directory or, rooted, to the source directory, which is
    /// a dependency of the document whether or not it can be read. A file
    /// the build can read joins `env.dlfiles` under the destination
    /// `<md5 of its srcdir-relative path>/<name>` (`f.txt` and `/f.txt`
    /// are one file), which the reference takes as its `filename`; one it
    /// cannot read is logged, `location=node`, `download file not readable:
    /// <its absolute path>` (`type='download', subtype='not_readable'`).
    /// Probed on Sphinx 9.1.0 (`scratchpad/t17c/p2_download.py`, the dummy
    /// builder's post-read doctree, `env.dlfiles` and `env.dependencies`).
    #[test]
    fn download_collector_registers_md5_destination() {
        let (_tmp, srcdir) = project(&[("f.txt", "f\n"), ("sub/g.txt", "g\n")]);
        let read = read_in(&srcdir, "a", R_DOWNLOAD);

        assert_eq!(
            body(&read),
            "    <paragraph>\n        \
             <download_reference filename=\"8a7cbf1fec08be06692c97095048ab68/f.txt\" refdoc=\"a\" \
             refdomain=\"\" refexplicit=\"0\" reftarget=\"f.txt\" reftype=\"download\" \
             refwarn=\"0\">\n            <literal classes=\"xref download\">\n                \
             f.txt\n    <paragraph>\n        \
             <download_reference filename=\"8a7cbf1fec08be06692c97095048ab68/f.txt\" refdoc=\"a\" \
             refdomain=\"\" refexplicit=\"1\" reftarget=\"f.txt\" reftype=\"download\" \
             refwarn=\"0\">\n            <literal classes=\"xref download\">\n                \
             the file\n    <paragraph>\n        \
             <download_reference filename=\"feae585b0a17a20e9e41978e85bfc62f/g.txt\" refdoc=\"a\" \
             refdomain=\"\" refexplicit=\"0\" reftarget=\"sub/g.txt\" reftype=\"download\" \
             refwarn=\"0\">\n            <literal classes=\"xref download\">\n                \
             sub/g.txt\n    <paragraph>\n        \
             <download_reference filename=\"8a7cbf1fec08be06692c97095048ab68/f.txt\" refdoc=\"a\" \
             refdomain=\"\" refexplicit=\"0\" reftarget=\"/f.txt\" reftype=\"download\" \
             refwarn=\"0\">\n            <literal classes=\"xref download\">\n                \
             /f.txt\n    <paragraph>\n        \
             <download_reference refdoc=\"a\" refdomain=\"\" refexplicit=\"0\" \
             reftarget=\"missing.txt\" reftype=\"download\" refwarn=\"0\">\n            \
             <literal classes=\"xref download\">\n                missing.txt\n    \
             <paragraph>\n        \
             <download_reference refdoc=\"a\" refdomain=\"\" refexplicit=\"0\" \
             reftarget=\"https://example.com/x.zip\" reftype=\"download\" \
             refuri=\"https://example.com/x.zip\" refwarn=\"0\">\n            \
             <literal classes=\"xref download\">\n                https://example.com/x.zip\n    \
             <paragraph>\n        <literal classes=\"xref download\">\n            f.txt\n    \
             <paragraph>\n        \
             <download_reference refdoc=\"a\" refdomain=\"\" refexplicit=\"0\" \
             reftarget=\"f .txt\" reftype=\"download\" refwarn=\"0\">\n            \
             <literal classes=\"xref download\">\n                f  .txt\n"
        );
        let dlfiles: Vec<(&str, Vec<&str>, &str)> = read
            .env
            .dlfiles
            .iter()
            .map(|(file, (docnames, destination))| {
                (
                    file,
                    docnames.iter().map(String::as_str).collect(),
                    destination.as_str(),
                )
            })
            .collect();
        assert_eq!(
            dlfiles,
            [
                ("f.txt", vec!["a"], "8a7cbf1fec08be06692c97095048ab68/f.txt"),
                (
                    "sub/g.txt",
                    vec!["a"],
                    "feae585b0a17a20e9e41978e85bfc62f/g.txt"
                ),
            ]
        );
        assert_eq!(
            read.env.dependencies["a"],
            BTreeSet::from(
                ["f .txt", "f.txt", "missing.txt", "sub/g.txt"].map(|path| under(&srcdir, path))
            )
        );
        let a = under(&srcdir, "a.rst");
        assert_eq!(
            rendered(&read),
            [
                format!(
                    "{}:9: WARNING: download file not readable: {} [download.not_readable]",
                    a.display(),
                    under(&srcdir, "missing.txt").display()
                ),
                format!(
                    "{}:15: WARNING: download file not readable: {} [download.not_readable]",
                    a.display(),
                    under(&srcdir, "f .txt").display()
                ),
            ]
        );
    }

    /// Each warning is located at its reference, `location=node`, which
    /// carries the line of the role that made it: a docinfo field's at its
    /// own line (the collector runs before the Metadata collector pops the
    /// docinfo), a substitution's copy at its definition's line, as the
    /// definition's own does, and an included file's at its line in that
    /// file. A path is resolved as `relfn2path` resolves it, so
    /// `sub/../f.txt` is `f.txt`, and `../outside.txt` is the file beside
    /// the source directory, which the warning names by its absolute path
    /// and the dependency by its path relative to the source directory.
    /// Probed on Sphinx 9.1.0 (`scratchpad/t17c/p5_dl_unit.py`).
    #[test]
    fn downloads_are_located_at_their_role_and_resolved_like_relfn2path() {
        let (_tmp, root) = project(&[
            ("src/f.txt", "f\n"),
            (
                "src/part.inc",
                "Included :download:`gone.txt` and :download:`f.txt`.\n",
            ),
        ]);
        let srcdir = root.join("src");
        let read = read_in(
            &srcdir,
            "index",
            ":Download: :download:`docinfo.txt`\n\nTitle\n=====\n\nSub |d|.\n\n\
             .. |d| replace:: :download:`subst.txt`\n\n.. include:: part.inc\n\n\
             Para :download:`sub/../f.txt` and :download:`../outside.txt`.\n",
        );

        let index = under(&srcdir, "index.rst");
        let unreadable = |at: &Path, line: u32, path: PathBuf| {
            format!(
                "{}:{line}: WARNING: download file not readable: {} [download.not_readable]",
                at.display(),
                path.display()
            )
        };
        assert_eq!(
            rendered(&read),
            [
                unreadable(&index, 1, under(&srcdir, "docinfo.txt")),
                unreadable(&index, 8, under(&srcdir, "subst.txt")),
                unreadable(&index, 8, under(&srcdir, "subst.txt")),
                unreadable(&under(&srcdir, "part.inc"), 1, under(&srcdir, "gone.txt")),
                unreadable(&index, 12, root.join("outside.txt")),
            ]
        );
        assert_eq!(
            body(&read),
            "    <section ids=\"title\" names=\"title\">\n        <title>\n            Title\n        \
             <paragraph>\n            Sub \n            \
             <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" \
             reftarget=\"subst.txt\" reftype=\"download\" refwarn=\"0\">\n                \
             <literal classes=\"xref download\">\n                    subst.txt\n            .\n        \
             <substitution_definition names=\"d\">\n            \
             <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" \
             reftarget=\"subst.txt\" reftype=\"download\" refwarn=\"0\">\n                \
             <literal classes=\"xref download\">\n                    subst.txt\n        \
             <paragraph>\n            Included \n            \
             <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" \
             reftarget=\"gone.txt\" reftype=\"download\" refwarn=\"0\">\n                \
             <literal classes=\"xref download\">\n                    gone.txt\n             and \n            \
             <download_reference filename=\"8a7cbf1fec08be06692c97095048ab68/f.txt\" \
             refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"f.txt\" \
             reftype=\"download\" refwarn=\"0\">\n                \
             <literal classes=\"xref download\">\n                    f.txt\n            .\n        \
             <paragraph>\n            Para \n            \
             <download_reference filename=\"8a7cbf1fec08be06692c97095048ab68/f.txt\" \
             refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" reftarget=\"sub/../f.txt\" \
             reftype=\"download\" refwarn=\"0\">\n                \
             <literal classes=\"xref download\">\n                    sub/../f.txt\n             and \n            \
             <download_reference refdoc=\"index\" refdomain=\"\" refexplicit=\"0\" \
             reftarget=\"../outside.txt\" reftype=\"download\" refwarn=\"0\">\n                \
             <literal classes=\"xref download\">\n                    ../outside.txt\n            .\n"
        );
        assert_eq!(
            read.env.dlfiles.iter().collect::<Vec<_>>(),
            [(
                "f.txt",
                &(
                    BTreeSet::from(["index".to_string()]),
                    "8a7cbf1fec08be06692c97095048ab68/f.txt".to_string()
                )
            )]
        );
        assert_eq!(
            read.env.dependencies["index"],
            BTreeSet::from([
                srcdir.join("..").join("outside.txt"),
                under(&srcdir, "docinfo.txt"),
                under(&srcdir, "f.txt"),
                under(&srcdir, "gone.txt"),
                under(&srcdir, "part.inc"),
                under(&srcdir, "subst.txt"),
            ])
        );
    }

    /// `DownloadFiles`: a file keeps the destination it was given whoever
    /// else offers it, and stays while a document offers it.
    #[test]
    fn a_download_is_forgotten_with_the_last_document_that_offers_it() {
        let mut dlfiles = super::DownloadFiles::default();
        assert_eq!(
            dlfiles.add_file("a", "f.txt"),
            "8a7cbf1fec08be06692c97095048ab68/f.txt"
        );
        assert_eq!(
            dlfiles.add_file("index", "f.txt"),
            "8a7cbf1fec08be06692c97095048ab68/f.txt"
        );
        assert_eq!(
            dlfiles.add_file("b", "sub/f.txt"),
            "65c749bae045d9ca1d8f7e825ba9a46f/f.txt"
        );
        dlfiles.purge_doc("a");
        assert_eq!(
            dlfiles.get("f.txt"),
            Some(&(
                BTreeSet::from(["index".to_string()]),
                "8a7cbf1fec08be06692c97095048ab68/f.txt".to_string()
            ))
        );
        dlfiles.purge_doc("index");
        assert_eq!(dlfiles.get("f.txt"), None);
        dlfiles.purge_doc("b");
        assert!(dlfiles.is_empty());
    }
}
