//! `ImageCollector` (`SP/sphinx/environment/collectors/asset.py:33-134`)
//! and `env.images`, the image files a build copies to `_images/`
//! ([`FilenameUniqDict`]).
//!
//! The collector visits every `image` node of a read document, those in a
//! `substitution_definition` included ([TW A2]): it stamps the node with
//! the files the writer may take for it (`candidates`), rewrites its `uri`
//! to the path relative to the source directory, notes each candidate as a
//! dependency of the document, and registers each readable one in
//! `env.images`, or, for one it cannot read, warns `image file not
//! readable`. It runs in the `doctree-read` slot of the read
//! (`DoctreeReadEvent`, 880-030), after the Dependencies collector and
//! before the Metadata one ([`crate::transforms::run_merge_transforms`]).
//!
//! Two of Sphinx's settings are fixed at their defaults, which no
//! configuration here can change: `figure_language_filename`
//! (`'{root}.{language}{ext}'`), the name of a language's variant of an
//! image, and the order in which a `*` pattern's matches are found, which
//! Sphinx takes from the directory listing and this port takes sorted by
//! name.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diagnostics::Category;
use crate::doctree::{kinds, AttrValue};
use crate::env::BuildEnvironment;
use crate::transforms::tree::TxId;
use crate::transforms::TransformCx;
use crate::utils::{py_repr_str, relfn2path_rel};

/// Sphinx's `FilenameUniqDict` (`SP/sphinx/util/_files.py:14-61`), the
/// type of `env.images`: each file the documents use, by its path relative
/// to the source directory, with the documents that use it and the unique
/// basename the builder copies it under, so that every image can go into
/// one directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilenameUniqDict {
    /// path → (docnames, unique basename).
    files: BTreeMap<String, (BTreeSet<String>, String)>,
    /// `_existing`: the unique basenames handed out.
    existing: BTreeSet<String>,
}

impl FilenameUniqDict {
    /// `add_file(docname, newfile)`: records that `docname` uses `file`
    /// and returns the file's unique basename. A new file takes its own
    /// basename, or, when another file holds that already, the first of
    /// `<stem>1<suffix>`, `<stem>2<suffix>`, … nobody holds (Python 3.12's
    /// `PurePath.name`, `.stem` and `.suffix`).
    pub fn add_file(&mut self, docname: &str, file: &str) -> String {
        if let Some((docnames, unique)) = self.files.get_mut(file) {
            docnames.insert(docname.to_string());
            return unique.clone();
        }
        let name = file.rsplit('/').next().unwrap_or(file);
        let (stem, suffix) = split_suffix(name);
        let mut unique = name.to_string();
        let mut i = 0;
        while self.existing.contains(&unique) {
            i += 1;
            unique = format!("{stem}{i}{suffix}");
        }
        self.files.insert(
            file.to_string(),
            (BTreeSet::from([docname.to_string()]), unique.clone()),
        );
        self.existing.insert(unique.clone());
        unique
    }

    /// `purge_doc(docname)`: forgets that `docname` uses any file, and every
    /// file no document uses any more, its basename freed.
    pub fn purge_doc(&mut self, docname: &str) {
        let existing = &mut self.existing;
        self.files.retain(|_, (docnames, unique)| {
            docnames.remove(docname);
            if docnames.is_empty() {
                existing.remove(unique.as_str());
            }
            !docnames.is_empty()
        });
    }

    /// The documents that use `file` and its unique basename.
    pub fn get(&self, file: &str) -> Option<&(BTreeSet<String>, String)> {
        self.files.get(file)
    }

    /// Every file, with the documents that use it and its unique basename.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &(BTreeSet<String>, String))> {
        self.files
            .iter()
            .map(|(file, entry)| (file.as_str(), entry))
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// Python 3.12's `PurePath.stem` and `.suffix` of a file name: split at
/// its last `.`, unless that is its first or its last character.
fn split_suffix(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot < name.len() - 1 => name.split_at(dot),
        _ => (name, ""),
    }
}

/// `posixpath.splitext(path)`: the path and its extension, the last `.`
/// of its last component and what follows, unless that component is
/// nothing but dots before it.
fn splitext(path: &str) -> (&str, &str) {
    let name_start = path.rfind('/').map_or(0, |slash| slash + 1);
    match path.rfind('.') {
        Some(dot) if dot > name_start && !path[name_start..dot].chars().all(|c| c == '.') => {
            path.split_at(dot)
        }
        _ => (path, ""),
    }
}

/// `ImageCollector.process_doc` for the document `cx` reads, folding its
/// images into `env`: every `image` node in the document's tree, in tree
/// order (`doctree.findall(nodes.image)`, `asset.py:52-105`).
///
/// - A `data:` URI or one with a scheme (`://`) is its own candidate,
///   `'?'`, and nothing else is done with it.
/// - A URI ending in `.*` names every file its pattern matches: the `uri`
///   becomes the pattern relative to the source directory, and the
///   candidates are the matches of the language's variant of the pattern
///   and then those of the pattern itself, one per mimetype
///   ([`collect_candidates`]).
/// - Any other URI names one file, its language's variant when that
///   exists ([`search_image_for_language`]): the `uri` becomes that file's
///   path relative to the source directory, its only candidate `'*'`, and
///   the URI as written stays as `original_uri` when it differs.
///
/// Each candidate is then a dependency of the document; one the build can
/// read joins `env.images`, and one it cannot read is logged, `location=
/// node`: `image file not readable: <path>` (`type='image',
/// subtype='not_readable'`, [`TransformCx::warn`]).
pub fn process_doc(cx: &mut TransformCx<'_>, env: &mut BuildEnvironment) {
    let root = cx.tree.root();
    for image in cx.tree.findall(root, |node| node.kind == kinds::IMAGE) {
        let Some(uri) = cx.tree.node(image).str_attr("uri").map(str::to_string) else {
            continue;
        };
        let mut candidates: Vec<(String, String)> = Vec::new();
        if uri.starts_with("data:") || uri.contains("://") {
            candidates.push(("?".to_string(), uri));
            set_candidates(cx, image, &candidates);
            continue;
        }
        if uri.ends_with(".*") {
            let pattern = relfn2path_rel(&uri, cx.docname);
            cx.tree
                .node_mut(image)
                .set("uri", AttrValue::Str(pattern.clone()));
            let translated = image_filename_for_language(&uri, &cx.settings.language);
            let translated = relfn2path_rel(&translated, cx.docname);
            collect_candidates(cx, image, &translated, &mut candidates);
            collect_candidates(cx, image, &pattern, &mut candidates);
        } else {
            let uri = search_image_for_language(&uri, cx.docname, cx.srcdir, &cx.settings.language);
            let node = cx.tree.node_mut(image);
            let original = node.str_attr("uri").unwrap_or_default().to_string();
            let path = relfn2path_rel(&uri, cx.docname);
            node.set("uri", AttrValue::Str(path.clone()));
            if path != original {
                node.set("original_uri", AttrValue::Str(original));
            }
            candidates.push(("*".to_string(), path));
        }
        set_candidates(cx, image, &candidates);

        for (_, path) in &candidates {
            let file = in_srcdir(cx.srcdir, path);
            env.dependencies
                .entry(cx.docname.to_string())
                .or_default()
                .insert(file.clone());
            if !is_readable(&file) {
                cx.warn(
                    image,
                    format!("image file not readable: {path}"),
                    not_readable(),
                );
                continue;
            }
            env.images.add_file(cx.docname, path);
        }
    }
}

/// `type='image', subtype='not_readable'`.
fn not_readable() -> Category {
    Category::from_parts(Some("image"), Some("not_readable"))
}

/// `node['candidates'] = candidates`: the dict, printed as Python prints
/// it, its keys in the order they were added.
fn set_candidates(cx: &mut TransformCx<'_>, image: TxId, candidates: &[(String, String)]) {
    let entries: Vec<String> = candidates
        .iter()
        .map(|(key, path)| format!("{}: {}", py_repr_str(key), py_repr_str(path)))
        .collect();
    cx.tree.node_mut(image).set(
        "candidates",
        AttrValue::Str(format!("{{{}}}", entries.join(", "))),
    );
}

/// `srcdir / path`: a path relative to the source directory, with `/` as
/// its separator, joined onto it one component at a time.
fn in_srcdir(srcdir: &Path, path: &str) -> PathBuf {
    let mut file = srcdir.to_path_buf();
    file.extend(path.split('/').filter(|part| !part.is_empty()));
    file
}

/// `os.access(path, os.R_OK)`, as far as the build can tell: whether it
/// can open the file, or list the directory, at `path`.
fn is_readable(path: &Path) -> bool {
    std::fs::File::open(path).is_ok() || std::fs::read_dir(path).is_ok()
}

/// `get_image_filename_for_language(filename, env)`
/// (`SP/sphinx/util/i18n.py:316-334`) under the default
/// `figure_language_filename`, `'{root}.{language}{ext}'`.
fn image_filename_for_language(filename: &str, language: &str) -> String {
    let (root, ext) = splitext(filename);
    format!("{root}.{language}{ext}")
}

/// `search_image_for_language(filename, env)` (`i18n.py:337-343`): the
/// language's variant of the image `filename` written in `docname`, when
/// that file exists, and `filename` otherwise.
fn search_image_for_language(
    filename: &str,
    docname: &str,
    srcdir: &Path,
    language: &str,
) -> String {
    let translated = image_filename_for_language(filename, language);
    if crate::utils::relfn2path(&translated, docname, srcdir).exists() {
        translated
    } else {
        filename.to_string()
    }
}

/// `ImageCollector.collect_candidates` (`asset.py:107-134`): the files the
/// pattern `path` (relative to the source directory) matches, each keyed by
/// its mimetype, or `image/x-<extension>` for one with none, join
/// `candidates` unless their mimetype is there already; of several matches
/// of one mimetype, the shortest path. A match whose type cannot be read
/// off its first bytes is logged (`image file <path> not readable: <OSError>`)
/// and left out.
fn collect_candidates(
    cx: &mut TransformCx<'_>,
    image: TxId,
    path: &str,
    candidates: &mut Vec<(String, String)>,
) {
    let (dir, pattern) = match path.rsplit_once('/') {
        Some((dir, pattern)) => (dir, pattern),
        None => ("", path),
    };
    let directory = in_srcdir(cx.srcdir, dir);
    let mut globbed: Vec<(String, Vec<String>)> = Vec::new();
    for name in glob_names(&directory, pattern) {
        let filename = directory.join(&name);
        let relative = if dir.is_empty() {
            name.clone()
        } else {
            format!("{dir}/{name}")
        };
        let mimetype = match guess_mimetype(&filename, &name) {
            Ok(Some(mimetype)) => mimetype,
            Ok(None) => format!("image/x-{}", splitext(&name).1.get(1..).unwrap_or_default()),
            Err(err) => {
                let filename = filename.display().to_string();
                let message = format!(
                    "image file {filename} not readable: {}",
                    python_os_error(&err, &filename)
                );
                cx.warn(image, message, not_readable());
                continue;
            }
        };
        if candidates.iter().any(|(key, _)| *key == mimetype) {
            continue;
        }
        match globbed.iter_mut().find(|(key, _)| *key == mimetype) {
            Some((_, files)) => files.push(relative),
            None => globbed.push((mimetype, vec![relative])),
        }
    }
    for (mimetype, files) in globbed {
        // `min(files, key=len)`: the first of the shortest.
        let shortest = files
            .iter()
            .min_by_key(|file| file.chars().count())
            .cloned()
            .unwrap_or_default();
        candidates.push((mimetype, shortest));
    }
}

/// The names in `directory` that the last component of a `glob` pattern
/// matches: `fnmatch` over the directory's entries, a name that starts
/// with `.` only for a pattern that does, sorted (see the module docs). A
/// directory that cannot be listed matches nothing.
fn glob_names(directory: &Path, pattern: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let hidden = pattern.starts_with('.');
    let pattern: Vec<char> = pattern.chars().collect();
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| hidden || !name.starts_with('.'))
        .filter(|name| fnmatch(&name.chars().collect::<Vec<_>>(), &pattern))
        .collect();
    names.sort();
    names
}

/// `fnmatch.fnmatchcase(name, pattern)`: `*` matches any run of
/// characters, `?` any one, `[seq]` one in the set and `[!seq]` one out of
/// it (ranges `a-z` included); a `[` without its `]` is itself.
fn fnmatch(name: &[char], pattern: &[char]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some(('*', rest)) => (0..=name.len()).any(|skip| fnmatch(&name[skip..], rest)),
        Some(('?', rest)) => !name.is_empty() && fnmatch(&name[1..], rest),
        Some(('[', rest)) => match set_end(rest) {
            Some(end) => match name.split_first() {
                Some((&c, name)) => in_set(&rest[..end], c) && fnmatch(name, &rest[end + 1..]),
                None => false,
            },
            None => name.first() == Some(&'[') && fnmatch(&name[1..], rest),
        },
        Some((c, rest)) => name.first() == Some(c) && fnmatch(&name[1..], rest),
    }
}

/// Where the `]` that closes the set a `[` opens stands, in the pattern
/// after the `[`; `None` when nothing closes it. A `]` first in the set,
/// after `[` or `[!`, belongs to it.
fn set_end(pattern: &[char]) -> Option<usize> {
    let first = usize::from(pattern.first() == Some(&'!'));
    (first + 1..pattern.len()).find(|&at| pattern[at] == ']')
}

/// Whether `c` is in the set `set`, the text between `[` and `]`.
fn in_set(set: &[char], c: char) -> bool {
    let (negated, set) = match set.split_first() {
        Some(('!', rest)) => (true, rest),
        _ => (false, set),
    };
    let mut found = false;
    let mut at = 0;
    while at < set.len() {
        if at + 2 < set.len() && set[at + 1] == '-' {
            found |= set[at] <= c && c <= set[at + 2];
            at += 3;
        } else {
            found |= set[at] == c;
            at += 1;
        }
    }
    found != negated
}

/// Sphinx's `mime_suffixes` (`SP/sphinx/util/images.py:22-31`).
const MIME_SUFFIXES: &[(&str, &str)] = &[
    (".gif", "image/gif"),
    (".jpg", "image/jpeg"),
    (".png", "image/png"),
    (".pdf", "application/pdf"),
    (".svg", "image/svg+xml"),
    (".svgz", "image/svg+xml"),
    (".ai", "application/illustrator"),
    (".webp", "image/webp"),
];

/// `guess_mimetype(filename)` (`images.py:71-87`): by the file name's
/// suffix, lowercased, else by the first bytes of the file when it exists
/// ([`image_type_from_file`]), else none. Reading those bytes can fail,
/// which Sphinx does not catch here.
fn guess_mimetype(file: &Path, name: &str) -> std::io::Result<Option<String>> {
    let suffix = split_suffix(name).1.to_lowercase();
    if let Some((_, mimetype)) = MIME_SUFFIXES.iter().find(|(known, _)| *known == suffix) {
        return Ok(Some((*mimetype).to_string()));
    }
    if !file.exists() {
        return Ok(None);
    }
    Ok(image_type_from_file(file)?.map(|kind| format!("image/{kind}")))
}

/// `_image_type_from_file(filename)` (`images.py:124-160`): the image
/// type the file's first 32 bytes show, or none.
fn image_type_from_file(file: &Path) -> std::io::Result<Option<&'static str>> {
    let mut header = Vec::with_capacity(32);
    std::fs::File::open(file)?
        .take(32)
        .read_to_end(&mut header)?;
    let kind = if header.starts_with(b"BM") {
        "bmp"
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        "gif"
    } else if header.starts_with(b"\xff\xd8") {
        "jpeg"
    } else if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if header
        .to_ascii_lowercase()
        .windows(4)
        .any(|window| window == b"<svg")
    {
        "svg+xml"
    } else if header.starts_with(b"MM") || header.starts_with(b"II") {
        "tiff"
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP") {
        "webp"
    } else {
        return Ok(None);
    };
    Ok(Some(kind))
}

/// `str(err)` for the `OSError` Python raises opening or reading the file
/// `filename`: `[Errno <n>] <strerror>: '<filename>'`.
fn python_os_error(err: &std::io::Error, filename: &str) -> String {
    let Some(code) = err.raw_os_error() else {
        return err.to_string();
    };
    let text = err.to_string();
    let strerror = text
        .strip_suffix(&format!(" (os error {code})"))
        .unwrap_or(&text);
    format!("[Errno {code}] {strerror}: {}", py_repr_str(filename))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use crate::read::{read_standalone, ReadSettings};
    use crate::rst::ParseOptions;

    fn sphinx_options() -> ParseOptions {
        ParseOptions {
            source_path: "<SRCDIR>/index.rst".to_string(),
            sphinx: true,
            docname: "index".to_string(),
            ..ParseOptions::default()
        }
    }

    /// `ImageCollector.process_doc` stamps every image with its candidates
    /// and warns about a candidate it cannot read, `location=node`, with
    /// `type='image', subtype='not_readable'` (`asset.py:94-105`): the
    /// dependency is noted either way. It runs in `DoctreeReadEvent`
    /// (880-030), after the parse, so `docutils-0496` prints the figure's
    /// ERROR first and then the warning ([TW §5.5]), both at the
    /// directive's line. Probed on Sphinx 9.1.0 (the HTML oracle's
    /// `docutils-0496` warnings and post-read doctree).
    #[test]
    fn image_collector_sets_candidates_and_warns() {
        let read = read_standalone(
            ".. figure:: pic.png\n\n   - bullet\n   - list\n",
            &sphinx_options(),
            &ReadSettings::sphinx_defaults(),
        );
        let rendered: Vec<String> = read
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(true))
            .collect();
        assert_eq!(
            rendered,
            [
                "<SRCDIR>/index.rst:1: ERROR: Figure caption must be a paragraph or empty \
                 comment.\n\n.. figure:: pic.png\n\n   - bullet\n   - list [docutils]",
                "<SRCDIR>/index.rst:1: WARNING: image file not readable: pic.png \
                 [image.not_readable]",
            ]
        );
        assert_eq!(
            read.outcome.doctree().root.pformat(),
            "<document source=\"<SRCDIR>/index.rst\" \
             translation_progress=\"{'total': 0, 'translated': 0}\">\n    \
             <figure>\n        <image candidates=\"{'*': 'pic.png'}\" uri=\"pic.png\">\n"
        );
        assert_eq!(
            read.env.dependencies.get("index"),
            Some(&BTreeSet::from([PathBuf::from("<SRCDIR>").join("pic.png")]))
        );
    }

    /// The rendered records of `source`, read as `<SRCDIR>/index.rst`.
    fn records(source: &str) -> Vec<String> {
        read_standalone(source, &sphinx_options(), &ReadSettings::sphinx_defaults())
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(true))
            .collect()
    }

    fn not_readable(line: u32, path: &str) -> String {
        format!(
            "<SRCDIR>/index.rst:{line}: WARNING: image file not readable: {path} \
             [image.not_readable]"
        )
    }

    /// The collector walks every image of the tree in tree order, those in
    /// substitution definitions too ([TW A2]): a substituted image warns
    /// for the copy Substitutions (220) put in the paragraph and again for
    /// the definition's own, both at the definition's line, and a
    /// `:target:` reference around an image changes nothing. Probed on
    /// Sphinx 9.1.0.
    #[test]
    fn substitution_images_warn_for_each_copy_and_definition() {
        assert_eq!(
            records(
                "Title\n=====\n\nPara |img| text and |img2|.\n\n.. |img| image:: pic.png\n\n\
                 .. |img2| image:: pic2.png\n   :target: http://x.org\n\n\
                 .. image:: pic3.png\n   :target: http://y.org\n"
            ),
            [
                not_readable(6, "pic.png"),
                not_readable(8, "pic2.png"),
                not_readable(6, "pic.png"),
                not_readable(8, "pic2.png"),
                not_readable(11, "pic3.png"),
            ]
        );
    }

    /// The image collector runs before the Metadata collector pops the
    /// docinfo out of the tree (both at 880-030, in Sphinx's registration
    /// order), so the copy of a substituted image in a docinfo field warns
    /// as the definition's does, though the post-read doctree no longer
    /// holds it. Probed on Sphinx 9.1.0: two records at line 3.
    #[test]
    fn an_image_in_the_docinfo_is_collected_before_the_pop() {
        let read = read_standalone(
            ":Logo: |img|\n\n.. |img| image:: pic.png\n\nTitle\n=====\n\nText.\n",
            &sphinx_options(),
            &ReadSettings::sphinx_defaults(),
        );
        let rendered: Vec<String> = read
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.render(true))
            .collect();
        assert_eq!(
            rendered,
            [not_readable(3, "pic.png"), not_readable(3, "pic.png")]
        );
        assert!(
            !read.outcome.doctree().root.pformat().contains("docinfo"),
            "the Metadata collector popped the docinfo"
        );
    }

    /// docutils locates an image at its directive's line
    /// (`get_source_and_line(self.lineno)`, `images.py:99-101`), which in a
    /// substitution definition's hanging form is the line after the
    /// definition's own. Probed on Sphinx 9.1.0: both records at line 4.
    #[test]
    fn a_hanging_substitution_image_is_located_at_its_directive() {
        assert_eq!(
            records("Para |img|.\n\n.. |img|\n   image:: pic.png\n"),
            [not_readable(4, "pic.png"), not_readable(4, "pic.png")]
        );
    }

    /// A project in a temporary directory: `files` written under it.
    fn project(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        for (path, bytes) in files {
            let file = tmp.path().join(path);
            std::fs::create_dir_all(file.parent().expect("a file has a parent")).expect("mkdir");
            std::fs::write(file, bytes).expect("write");
        }
        tmp
    }

    /// The document `docname` of the project at `srcdir`, read standalone
    /// with `settings`.
    fn read_in(
        srcdir: &std::path::Path,
        docname: &str,
        source: &str,
        settings: &ReadSettings,
    ) -> crate::read::StandaloneRead {
        let mut source_path = srcdir.to_path_buf();
        source_path.extend(docname.split('/'));
        source_path.set_extension("rst");
        let parse = ParseOptions {
            source_path: source_path.display().to_string(),
            docname: docname.to_string(),
            srcdir: Some(srcdir.to_path_buf()),
            ..sphinx_options()
        };
        read_standalone(source, &parse, settings)
    }

    /// Each image's `uri`, `original_uri` and `candidates`, in tree order.
    fn images_of(read: &crate::read::StandaloneRead) -> Vec<[String; 3]> {
        fn walk(node: &crate::doctree::Node, out: &mut Vec<[String; 3]>) {
            if node.kind == crate::doctree::kinds::IMAGE {
                let attr = |key| match node.get(key) {
                    Some(crate::doctree::AttrValue::Str(value)) => value.clone(),
                    _ => String::new(),
                };
                out.push([attr("uri"), attr("original_uri"), attr("candidates")]);
            }
            for child in &node.children {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        walk(&read.outcome.doctree().root, &mut out);
        out
    }

    fn row(uri: &str, original_uri: &str, candidates: &str) -> [String; 3] {
        [uri, original_uri, candidates].map(str::to_string)
    }

    /// A URI resolves against its document's directory, or, written
    /// rooted, against the source directory; its `uri` becomes the path
    /// relative to the source directory, the URI as written staying as
    /// `original_uri`. Each file the build can read joins `env.images`
    /// once, under a basename no other file holds (`pic1.png`, `pic2.png`),
    /// and every candidate is a dependency, a file it cannot read too. A
    /// `data:` URI and one with a scheme are their own candidate, `?`, and
    /// nothing else. Probed on Sphinx 9.1.0 (the dummy builder's
    /// `env.images`, `env.dependencies` and post-read doctree; a data URI's
    /// `env.images` entry comes from the write phase).
    #[test]
    fn readable_images_join_env_images_under_unique_names() {
        let tmp = project(&[("a/pic.png", b"x"), ("pic.png", b"x"), ("b/pic.png", b"x")]);
        let read = read_in(
            tmp.path(),
            "a/doc",
            "Doc\n===\n\n.. image:: pic.png\n\n.. image:: /pic.png\n\n\
             .. image:: ../b/pic.png\n\n.. image:: pic.png\n\n\
             .. image:: data:image/png;base64,iVBORw0KGgo=\n\n\
             .. image:: https://example.com/x.png\n\n.. image:: missing.png\n",
            &ReadSettings::sphinx_defaults(),
        );
        assert_eq!(
            images_of(&read),
            [
                row("a/pic.png", "pic.png", "{'*': 'a/pic.png'}"),
                row("pic.png", "/pic.png", "{'*': 'pic.png'}"),
                row("b/pic.png", "../b/pic.png", "{'*': 'b/pic.png'}"),
                row("a/pic.png", "pic.png", "{'*': 'a/pic.png'}"),
                row(
                    "data:image/png;base64,iVBORw0KGgo=",
                    "",
                    "{'?': 'data:image/png;base64,iVBORw0KGgo='}"
                ),
                row(
                    "https://example.com/x.png",
                    "",
                    "{'?': 'https://example.com/x.png'}"
                ),
                row("a/missing.png", "missing.png", "{'*': 'a/missing.png'}"),
            ]
        );
        let images: Vec<(&str, Vec<&str>, &str)> = read
            .env
            .images
            .iter()
            .map(|(file, (docnames, unique))| {
                (
                    file,
                    docnames.iter().map(String::as_str).collect(),
                    unique.as_str(),
                )
            })
            .collect();
        assert_eq!(
            images,
            [
                ("a/pic.png", vec!["a/doc"], "pic.png"),
                ("b/pic.png", vec!["a/doc"], "pic2.png"),
                ("pic.png", vec!["a/doc"], "pic1.png"),
            ]
        );
        assert_eq!(
            read.env.dependencies["a/doc"],
            BTreeSet::from(
                ["a/missing.png", "a/pic.png", "b/pic.png", "pic.png"].map(|path| path
                    .split('/')
                    .fold(tmp.path().to_path_buf(), |dir, part| dir.join(part)))
            )
        );
        let missing = tmp.path().join("a").join("doc.rst");
        assert_eq!(
            read.diagnostics
                .iter()
                .map(|diagnostic| diagnostic.render(true))
                .collect::<Vec<_>>(),
            [format!(
                "{}:16: WARNING: image file not readable: a/missing.png [image.not_readable]",
                missing.display()
            )]
        );
    }

    /// A URI ending in `.*` names every file its pattern matches: the
    /// candidates are the matches of the language's variant of the pattern
    /// and then those of the pattern itself, one per mimetype, by suffix
    /// (`.png`, `.svg`), else by the file's first bytes (a PNG named
    /// `pic.bin`), else `image/x-<extension>`; the `uri` is the pattern,
    /// relative to the source directory. A pattern that matches nothing has
    /// no candidates.
    /// A URI whose language variant exists (`lang.en.png` for `en`, the
    /// default) takes it, its own staying as `original_uri`. Probed on
    /// Sphinx 9.1.0, with `language` `en` and `de`.
    #[test]
    fn patterns_and_language_variants_choose_the_candidates() {
        let tmp = project(&[
            ("img/pic.png", b"x"),
            ("img/pic.svg", b"<svg/>"),
            ("img/pic.en.png", b"x"),
            ("img/pic.bin", b"\x89PNG\r\n\x1a\nxxxx"),
            ("img/pic.xyz", b"junk"),
            ("lang.png", b"x"),
            ("lang.en.png", b"x"),
        ]);
        let source = "Index\n=====\n\n.. image:: img/pic.*\n\n.. image:: lang.png\n\n\
                      .. image:: none.*\n";
        let read = read_in(
            tmp.path(),
            "index",
            source,
            &ReadSettings::sphinx_defaults(),
        );
        assert_eq!(
            images_of(&read),
            [
                row(
                    "img/pic.*",
                    "",
                    "{'image/png': 'img/pic.en.png', 'image/svg+xml': 'img/pic.svg', \
                     'image/x-xyz': 'img/pic.xyz'}"
                ),
                row("lang.en.png", "lang.png", "{'*': 'lang.en.png'}"),
                row("none.*", "", "{}"),
            ]
        );
        assert!(read.diagnostics.is_empty(), "{:?}", read.diagnostics);
        let files: Vec<(&str, &str)> = read
            .env
            .images
            .iter()
            .map(|(file, (_, unique))| (file, unique.as_str()))
            .collect();
        assert_eq!(
            files,
            [
                ("img/pic.en.png", "pic.en.png"),
                ("img/pic.svg", "pic.svg"),
                ("img/pic.xyz", "pic.xyz"),
                ("lang.en.png", "lang.en.png"),
            ]
        );
        assert_eq!(
            read.env.dependencies["index"],
            BTreeSet::from(
                [
                    ["img", "pic.en.png"].as_slice(),
                    &["img", "pic.svg"],
                    &["img", "pic.xyz"],
                    &["lang.en.png"],
                ]
                .map(|parts| parts
                    .iter()
                    .fold(tmp.path().to_path_buf(), |dir, part| dir.join(part)))
            ),
            "each candidate, and no file for a pattern that matches none"
        );

        let german = ReadSettings {
            language: "de".to_string(),
            ..ReadSettings::sphinx_defaults()
        };
        let read = read_in(tmp.path(), "index", ".. image:: lang.png\n", &german);
        assert_eq!(images_of(&read), [row("lang.png", "", "{'*': 'lang.png'}")]);
    }

    /// `FilenameUniqDict`: a file keeps the basename it was given whoever
    /// else uses it, and a document's purge frees the basenames of the
    /// files nobody uses any more, which the next file of that name takes.
    #[test]
    fn unique_names_are_freed_with_the_last_document_that_uses_them() {
        let mut images = super::FilenameUniqDict::default();
        assert_eq!(images.add_file("a", "a/pic.png"), "pic.png");
        assert_eq!(images.add_file("b", "b/pic.png"), "pic1.png");
        assert_eq!(images.add_file("b", "a/pic.png"), "pic.png");
        assert_eq!(images.add_file("b", "c/.png"), ".png");
        assert_eq!(images.add_file("b", "d/.png"), ".png1");
        images.purge_doc("a");
        assert_eq!(
            images.get("a/pic.png"),
            Some(&(BTreeSet::from(["b".to_string()]), "pic.png".to_string()))
        );
        images.purge_doc("b");
        assert!(images.is_empty());
        assert_eq!(images.add_file("c", "c/pic.png"), "pic.png");
    }
}
