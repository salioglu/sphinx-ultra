//! Sphinx's environment collectors (`SP/sphinx/environment/collectors/`):
//! the `doctree-read` listeners that fold one read document into the build
//! environment, in the slot `DoctreeReadEvent` (880-030) gives them
//! ([`crate::transforms::run_merge_transforms`]).

pub mod downloads;
pub mod images;

use std::path::{Path, PathBuf};

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
