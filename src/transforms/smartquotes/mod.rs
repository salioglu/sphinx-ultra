//! Sphinx's smart quotes: `SphinxSmartQuotes`, the read transform at
//! 750-029 (`SP/sphinx/transforms/__init__.py:361-415`), which curls the
//! quotes and educates the dashes and ellipses of every block of text.
//!
//! [`educate`] ports the docutils functions the transform hands each
//! block's text to (`docutils.utils.smartquotes`); [`tables`] holds the
//! data they read, which `tools/gen_smartquotes_fixture.py --tables`
//! generates from docutils and Python. The transform's own walk over the
//! blocks is still a no-op row of [`super::READ_TRANSFORMS`].

pub mod educate;
pub mod tables;
