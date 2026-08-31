//! `&'static str` interner for the doctree wire format.
//!
//! `Node::kind` and `Attrs::extra` keys are `&'static str` in memory (see
//! `mod.rs`), but deserializing produces owned bytes with no `'static`
//! lifetime. The vocabulary they draw from is closed — `kinds.rs` consts
//! plus a bounded set of attribute-key literals — so leaking each distinct
//! string once, the first time it is seen, is a fixed, small cost, not a
//! growing leak: the interner never leaks more than that vocabulary size,
//! no matter how many doctrees are decoded.
//!
//! `intern` is also `pub` (not just `pub(crate)`) so callers can pre-seed or
//! inspect the process-wide table if wave 5+ needs it; today only this
//! module's serde glue calls it.

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserializer, Serializer};
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Mutex;

use super::AttrValue;

static INTERNED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

/// Return the process-wide `&'static str` for `s`, leaking a fresh
/// allocation only the first time this exact string is interned. Repeat
/// calls with equal content return the same pointer.
pub fn intern(s: &str) -> &'static str {
    let mut interned = INTERNED.lock().unwrap();
    if let Some(&existing) = interned.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    interned.insert(leaked);
    leaked
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
/// `Vec<(&'static str, AttrValue)>` that behaves like a sorted map. Written
/// as a map (`serialize_map`) so the wire format reflects that shape;
/// insertion order — the sorted-by-key invariant `Node::set` maintains — is
/// preserved by both bincode and JSON, so no re-sort is needed on the way
/// back in. Keys are interned on deserialize.
pub(crate) fn serialize_extra<S>(
    extra: &[(&'static str, AttrValue)],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut map = serializer.serialize_map(Some(extra.len()))?;
    for (key, value) in extra {
        map.serialize_entry(key, value)?;
    }
    map.end()
}

struct ExtraVisitor;

impl<'de> Visitor<'de> for ExtraVisitor {
    type Value = Vec<(&'static str, AttrValue)>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a map of attribute keys to values")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut out = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some((key, value)) = map.next_entry::<String, AttrValue>()? {
            out.push((intern(&key), value));
        }
        Ok(out)
    }
}

pub(crate) fn deserialize_extra<'de, D>(
    deserializer: D,
) -> Result<Vec<(&'static str, AttrValue)>, D::Error>
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
        let a = intern("paragraph");
        let b = intern("paragraph");
        assert_eq!(a, "paragraph");
        assert!(std::ptr::eq(a, b));
    }

    #[test]
    fn intern_distinguishes_different_strings() {
        let a = intern("section");
        let b = intern("title");
        assert_ne!(a, b);
    }
}
