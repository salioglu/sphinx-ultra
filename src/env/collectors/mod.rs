//! Sphinx's environment collectors (`SP/sphinx/environment/collectors/`):
//! the `doctree-read` listeners that fold one read document into the build
//! environment, in the slot `DoctreeReadEvent` (880-030) gives them
//! ([`crate::transforms::run_merge_transforms`]).

pub mod images;
