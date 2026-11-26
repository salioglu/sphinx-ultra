//! `&'static str` interner for the doctree wire format.
//!
//! `Node::kind` is a `&'static str` in memory (see `mod.rs`), but
//! deserializing produces owned bytes with no `'static` lifetime. For a
//! doctree bincode produced by this crate's own [`super::to_bincode`], the
//! kinds that flow through here are always this crate's own literals — a
//! small vocabulary, closed at compile time — so leaking each distinct
//! string once is a fixed, tiny cost.
//!
//! `Attrs::extra` keys are not interned, because their source is open: a
//! `meta` node's attribute names are whatever the document's author wrote
//! (`MetaBody.parsemeta`'s `node[attname.lower()] = val`), as many distinct
//! ones as the author likes, and Sphinx keeps every one of them. So a key
//! is a `Cow<'static, str>`: borrowed when it is one of this crate's
//! literals, owned when the parse takes it from the document, and owned
//! whenever a decode reads it ([`deserialize_extra`]). No number of
//! distinct keys can then use up the table the kinds need, drop an
//! attribute, or make a doctree or `env.bin` decode fail.
//!
//! Nothing here checks that the kinds are this crate's, though: `intern`
//! leaks and caches whatever `&str` it's handed, unconditionally. That's
//! harmless for this crate's own well-formed output, but `from_bincode`
//! (and, transitively, `intern`) also has to accept bytes it did *not*
//! produce — a corrupted file, or one written by a future/older version of
//! this crate with a different vocabulary, landing in the
//! wipe-on-fingerprint-mismatch doctree cache. `MAX_INTERNED` bounds the
//! damage from that case: once the table would grow past it, `intern`
//! refuses instead of leaking further, and deserialization fails with an
//! error instead of the process quietly accumulating unbounded leaked
//! memory. `intern` is `pub(crate)`, not `pub` — nothing outside this
//! crate needs it; widen deliberately if that changes.

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserializer, Serializer};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Mutex;

use super::AttrValue;

/// Generous upper bound on distinct interned strings. The real vocabulary
/// (node kinds) is a few hundred entries at most and fixed at compile
/// time, so legitimate input never comes close; this exists only to cap
/// growth from input that didn't come from this crate's own encoder.
pub(crate) const MAX_INTERNED: usize = 4096;

static INTERNED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

/// `intern` refused to leak another string: the process-wide table already
/// holds [`MAX_INTERNED`] distinct entries, far more than this crate's own
/// output ever produces — a sign the bytes being decoded didn't come from
/// this crate's encoder.
#[derive(Debug)]
pub(crate) struct InternLimitExceeded;

impl fmt::Display for InternLimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "doctree interner exceeded {MAX_INTERNED} distinct strings; \
             refusing to intern more (this input likely wasn't produced by \
             this crate's own encoder)"
        )
    }
}

impl std::error::Error for InternLimitExceeded {}

/// Return the process-wide `&'static str` for `s`, leaking a fresh
/// allocation only the first time this exact string is interned. Repeat
/// calls with equal content return the same pointer. Fails once the table
/// would grow past [`MAX_INTERNED`] — see the module docs.
pub(crate) fn intern(s: &str) -> Result<&'static str, InternLimitExceeded> {
    let mut interned = INTERNED.lock().unwrap();
    if let Some(&existing) = interned.get(s) {
        return Ok(existing);
    }
    if interned.len() >= MAX_INTERNED {
        return Err(InternLimitExceeded);
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    interned.insert(leaked);
    Ok(leaked)
}

/// `serialize_with` for `&'static str` fields (e.g. `Node::kind`): plain
/// string serialization, no interning needed on the write side.
pub(crate) fn serialize_str<S>(value: &&'static str, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(value)
}

/// `serialize_with`/`deserialize_with` for `Attrs::extra`: a
/// `Vec<(Cow<'static, str>, AttrValue)>` that behaves like a sorted map.
/// Written as a map (`serialize_map`) of plain strings so the wire format
/// reflects that shape; insertion order — the sorted-by-key invariant
/// `Node::set` maintains — is preserved by both bincode and JSON, so no
/// re-sort is needed on the way back in. Keys are never interned (see the
/// module docs): a decoded key is owned.
pub(crate) fn serialize_extra<S>(
    extra: &[(Cow<'static, str>, AttrValue)],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut map = serializer.serialize_map(Some(extra.len()))?;
    for (key, value) in extra {
        map.serialize_entry(key.as_ref(), value)?;
    }
    map.end()
}

struct ExtraVisitor;

impl<'de> Visitor<'de> for ExtraVisitor {
    type Value = Vec<(Cow<'static, str>, AttrValue)>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a map of attribute keys to values")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut out = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, value)) = map.next_entry::<String, AttrValue>()? {
            out.push((Cow::Owned(key), value));
        }
        Ok(out)
    }
}

pub(crate) fn deserialize_extra<'de, D>(
    deserializer: D,
) -> Result<Vec<(Cow<'static, str>, AttrValue)>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_map(ExtraVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_returns_pointer_equal_str_on_repeat_calls() {
        let a = intern("paragraph").unwrap();
        let b = intern("paragraph").unwrap();
        assert_eq!(a, "paragraph");
        assert!(std::ptr::eq(a, b));
    }

    #[test]
    fn intern_distinguishes_different_strings() {
        let a = intern("section").unwrap();
        let b = intern("title").unwrap();
        assert_ne!(a, b);
    }
}
