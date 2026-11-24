//! The parse options of a case of the sphinx snippet corpus
//! (`tools/gen_sphinx_fixture.py`), shared by its two consumers,
//! tests/sphinx_doctree_differential.rs and
//! tests/sphinx_smartquotes_differential.rs: the case read as the snippet
//! harness reads it (`<snippet>`, Sphinx mode, docname `index`), under the
//! configuration its `conf` dict — the confoverrides the generator applied
//! for the case — sets.
//!
//! Every key maps through the crate's own mappers, onto the py-domain
//! configuration ([`PySigConfig::apply_conf_key`]) or the read
//! configuration ([`ReadConfig::apply_conf_key`]); a key neither knows is
//! an error, so a generator-side conf addition fails here instead of
//! silently parsing under defaults (serde ignores unknown struct fields).

use std::collections::BTreeMap;

use sphinx_ultra::py::PySigConfig;
use sphinx_ultra::rst::{ParseOptions, ReadConfig};

/// The parse options of a snippet case whose `conf` is `conf`, or the
/// error naming the first key (or value shape) no mapper understands.
pub fn snippet_parse_options(
    conf: &BTreeMap<String, serde_json::Value>,
) -> Result<ParseOptions, String> {
    let mut py = PySigConfig::default();
    let mut read = ReadConfig::default();
    for (key, value) in conf {
        if !py.apply_conf_key(key, value)? && !read.apply_conf_key(key, value)? {
            return Err(format!(
                "unmapped conf key {key:?}: teach tests/support/fixture_conf.rs about it \
                 (and the parse layer, if it reads it)"
            ));
        }
    }
    Ok(ParseOptions {
        source_path: "<snippet>".into(),
        sphinx: true,
        docname: "index".into(),
        py,
        read,
        ..ParseOptions::default()
    })
}
