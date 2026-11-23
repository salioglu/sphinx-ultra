//! The census vocabulary of the read differentials (wave 5a, T6): the fix
//! families a read divergence can show, the task that fixes each, and the
//! doctree classifier both the post-read census
//! (`tests/sphinx_doctree_differential.rs`) and the read-state census
//! (`tests/read_state_differential.rs`) print their gap rows with.
//!
//! A gap row's reason is `"<families>: <owner>"`: every family its
//! divergence shows, and the latest task that fixes one of them (the plan's
//! Global Constraints). The family names are the corpus-baseline report's
//! [BASE §3.8] (`dangling refs`, `docinfo`, `autonumber ids`, `footnotes`,
//! `citations`, `indirect targets`, `substitutions`, and the
//! writer-invisible `candidates`), plus, for read state the HTML oracle's
//! inputs never show, `class attribute` and `pending`. The families fixed
//! since are gone — the dispatch families T7 fixed (`unknown role`, `code
//! directive`, `` `class` is `py:class` ``, `pep/rfc index order` and the
//! math directive's names), T9b's `system_message` (an INFO message
//! `FilterSystemMessages`, 999, removes), T9c's `doctest class` and
//! `transitions` (`DoctestTransform`, 500, and `Transitions`, 830) and
//! T10's `propagate targets` (`PropagateTargets`, 260), and T11's
//! `citations` and the tree shapes and records of `footnotes` and
//! `substitutions` (Substitutions, 220; the citation transforms, 619;
//! Footnotes, 620; the unreferenced-footnote warning, 622): a divergence of
//! that shape now is a regression, which the census reports unexplained.
//! T10's `dangling refs` and `indirect targets` stay, for what later tasks
//! shift: a `problematic` whose ids, and the target transforms' id
//! allocations and records, move when AutoNumbering (210) or a pending
//! transform takes ids first, in rows those tasks own. The read-state
//! census keeps `footnotes` and `substitutions` for the same reason, as the
//! families of the ids 620 and 220 allocate in such rows.

use std::collections::BTreeSet;

/// The owners of gap rows in the order their tasks land: a row is owned by
/// the latest task that fixes one of its families.
pub const OWNERS: &[&str] = &[
    "5a-T7",
    "5a-T8",
    "5a-T9a",
    "5a-T9b",
    "5a-T9c",
    "5a-T10",
    "5a-T11",
    "5a-T12",
    "5a-T13a",
    "5a-T13b",
    "5a-T14",
    "5a-T15",
    "5a-T16a",
    "5a-T16b",
    "5a-T17a",
    "5a-T17b",
    "5a-T17c",
    "5a-T18a",
    "5a-T18b",
    "5a-T18c",
    "5a-T19a",
    "5b (oracle regen)",
    "5b",
    "5c",
];

/// One fix family of a gap row and the task that fixes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GapFix {
    pub family: &'static str,
    pub owner: &'static str,
}

pub const fn gap_fix(family: &'static str, owner: &'static str) -> GapFix {
    GapFix { family, owner }
}

// The families, each with the task that ports what it lacks.
pub const DANGLING_REFS: GapFix = gap_fix("dangling refs", "5a-T10");
pub const INDIRECT_TARGETS: GapFix = gap_fix("indirect targets", "5a-T10");
pub const AUTONUMBER_IDS: GapFix = gap_fix("autonumber ids", "5a-T12");
pub const CLASS_ATTRIBUTE: GapFix = gap_fix("class attribute", "5a-T12");
pub const DOCINFO: GapFix = gap_fix("docinfo", "5a-T12");
pub const CANDIDATES: GapFix = gap_fix("candidates", "5a-T13a");
/// The structure directives' parse halves (`contents`, `sectnum`,
/// `target-notes`).
pub const PENDING_DIRECTIVES: GapFix = gap_fix("pending", "5a-T18a");
/// Their transforms (SectNum 710, Contents 720, TargetNotes 540).
pub const PENDING_TRANSFORMS: GapFix = gap_fix("pending", "5a-T18b");

/// What a census learned about one divergence.
#[derive(Debug, Default)]
pub struct Census {
    pub fixes: BTreeSet<GapFix>,
    /// Differences no family explains, printed above the row.
    pub unexplained: Vec<String>,
}

impl Census {
    pub fn add(&mut self, fix: GapFix) {
        self.fixes.insert(fix);
    }

    pub fn merge(&mut self, other: Census) {
        self.fixes.extend(other.fixes);
        self.unexplained.extend(other.unexplained);
    }

    /// `"<families>: <owner>"`, a `?` family for anything unexplained.
    pub fn reason(&self) -> String {
        let mut families: Vec<&str> = self.fixes.iter().map(|fix| fix.family).collect();
        if !self.unexplained.is_empty() {
            families.push("?");
        }
        families.sort_unstable();
        families.dedup();
        let owner = self
            .fixes
            .iter()
            .map(|fix| fix.owner)
            .max_by_key(|owner| {
                OWNERS
                    .iter()
                    .position(|known| known == owner)
                    .unwrap_or_else(|| panic!("unknown gap owner {owner:?}"))
            })
            .unwrap_or("?");
        format!("{}: {owner}", families.join("+"))
    }
}

/// One run of differing lines: what only the expected text has, and what
/// only the actual text has, at the same place.
#[derive(Debug, Default)]
pub struct Hunk<'a> {
    pub removed: Vec<&'a str>,
    pub added: Vec<&'a str>,
    /// Where `added` starts in the actual text.
    pub added_from: usize,
}

/// The line diff of two texts (a longest common subsequence), as hunks.
pub fn line_hunks<'a>(expected: &'a [&'a str], actual: &'a [&'a str]) -> Vec<Hunk<'a>> {
    let (n, m) = (expected.len(), actual.len());
    // lcs[i][j]: the common subsequence length of expected[i..], actual[j..].
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if expected[i] == actual[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut hunks = Vec::new();
    let mut hunk = Hunk::default();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && expected[i] == actual[j] {
            if !hunk.removed.is_empty() || !hunk.added.is_empty() {
                hunks.push(std::mem::take(&mut hunk));
            }
            i += 1;
            j += 1;
        } else {
            if hunk.removed.is_empty() && hunk.added.is_empty() {
                hunk.added_from = j;
            }
            if j == m || (i < n && lcs[i + 1][j] >= lcs[i][j + 1]) {
                hunk.removed.push(expected[i]);
                i += 1;
            } else {
                hunk.added.push(actual[j]);
                j += 1;
            }
        }
    }
    if !hunk.removed.is_empty() || !hunk.added.is_empty() {
        hunks.push(hunk);
    }
    hunks
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The text directly under `lines[at]`: the lines after it indented deeper
/// than it, up to its first child element, each trimmed.
fn text_under(lines: &[&str], at: usize) -> String {
    let Some(node) = lines.get(at) else {
        return String::new();
    };
    let depth = indent(node);
    lines[at + 1..]
        .iter()
        .take_while(|line| indent(line) > depth && !line.trim_start().starts_with('<'))
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n")
}

/// `line` without its `attribute="…"`.
fn without_attribute(line: &str, attribute: &str) -> String {
    let needle = format!(" {attribute}=\"");
    match line.find(&needle) {
        Some(start) => {
            let value = start + needle.len();
            match line[value..].find('"') {
                Some(end) => format!("{}{}", &line[..start], &line[value + end + 1..]),
                None => line.to_string(),
            }
        }
        None => line.to_string(),
    }
}

/// The family of a `problematic` the expected tree has, by its text (the
/// `rawsource` of the node a transform replaced) [DS §5.2, §5.5].
/// Substitutions' (220, `|name|`) and Footnotes' (620, `[#]_`, `[*]_`) are
/// ported since T11, so one of theirs that differs is not explained.
fn problematic_fixes(text: &str, census: &mut Census) {
    let substitution = text.strip_prefix('|').and_then(|rest| rest.split_once('|'));
    match substitution {
        // `|name|_` / `|name|__`: the reference is 850's or 440's.
        Some((_, "_" | "__")) => census.add(DANGLING_REFS),
        Some((_, "")) => census
            .unexplained
            .push(format!("problematic with text {text:?}")),
        _ if text == "[#]_" || text == "[*]_" => census
            .unexplained
            .push(format!("problematic with text {text:?}")),
        // A named or anonymous reference, a `[label]_` footnote reference
        // with no footnote, or an image `:target:` reference (no text):
        // AnonymousHyperlinks (440) or SphinxDanglingReferences (850).
        _ if text.is_empty() || text.ends_with('_') => census.add(DANGLING_REFS),
        _ => census
            .unexplained
            .push(format!("problematic with text {text:?}")),
    }
}

/// For each line of a pformat, the message of the `system_message` it
/// lies in, when it lies in one: the text of the message's paragraph.
fn message_lines(lines: &[&str]) -> Vec<Option<String>> {
    let mut messages = vec![None; lines.len()];
    let mut at = 0;
    while at < lines.len() {
        if !lines[at].trim_start().starts_with("<system_message ") {
            at += 1;
            continue;
        }
        let depth = indent(lines[at]);
        let end = (at + 1..lines.len())
            .find(|&line| indent(lines[line]) <= depth)
            .unwrap_or(lines.len());
        let message = text_under(lines, at + 1);
        for slot in &mut messages[at..end] {
            *slot = Some(message.clone());
        }
        at = end;
    }
    messages
}

/// The structure directives whose parse halves T18a ports.
const PENDING_DIRECTIVE_NAMES: [&str; 3] = ["contents", "sectnum", "target-notes"];

/// The families a doctree divergence shows: one diff hunk at a time, the
/// rules below over the lines only Sphinx's tree has (`removed`) and those
/// only ours has (`added`). Anything they do not explain is reported.
pub fn tree_census(expected: &str, actual: &str) -> Census {
    let expected_lines: Vec<&str> = expected.lines().collect();
    let actual_lines: Vec<&str> = actual.lines().collect();
    let messages = message_lines(&actual_lines);
    let notes = target_notes(&expected_lines);
    let mut census = Census::default();
    for hunk in line_hunks(&expected_lines, &actual_lines) {
        census.merge(hunk_census(&hunk, &messages, &notes));
    }
    census
}

/// The ids of the footnotes TargetNotes (540) makes in the expected tree,
/// those it names `TARGET_NOTE: <id>`: a footnote reference to one is
/// TargetNotes' too.
fn target_notes(expected: &[&str]) -> BTreeSet<String> {
    expected
        .iter()
        .map(|line| line.trim_start())
        .filter(|line| line.starts_with("<footnote ") && line.contains("TARGET_NOTE"))
        .filter_map(|line| line.split(" ids=\"").nth(1)?.split('"').next())
        .map(str::to_string)
        .collect()
}

/// One hunk's families. `messages` is [`message_lines`] of the actual
/// text: a `system_message` only our tree has is judged by its text,
/// whichever of its lines the hunk holds, since the line diff can match a
/// line inside it (a `<paragraph>`) and split it across hunks. `notes` are
/// the ids of TargetNotes' footnotes ([`target_notes`]).
fn hunk_census(hunk: &Hunk<'_>, messages: &[Option<String>], notes: &BTreeSet<String>) -> Census {
    let mut census = Census::default();
    let removed: Vec<&str> = hunk.removed.iter().map(|line| line.trim_start()).collect();
    let added: Vec<&str> = hunk.added.iter().map(|line| line.trim_start()).collect();
    let starts = |lines: &[&str], prefix: &str| lines.iter().any(|line| line.starts_with(prefix));

    // DocInfo (340) and MetadataCollector's pop (880): a leading field list
    // Sphinx moved out of the body. What it held went with it, so only the
    // lines Sphinx kept are classified further.
    let docinfo = hunk.added.contains(&"    <field_list>") && !starts(&removed, "<field_list>");
    if docinfo {
        census.add(DOCINFO);
    }
    // A reference or target Sphinx resolved: its line less its
    // `refuri`/`refid` is ours, less ours' `refname` when it has one.
    let unresolved: BTreeSet<String> = added
        .iter()
        .filter(|line| line.contains(" refname=\""))
        .map(|line| without_attribute(line, "refname"))
        .chain(added.iter().map(|line| line.to_string()))
        .collect();

    for (at, line) in removed.iter().enumerate() {
        if line.starts_with("<problematic ") {
            problematic_fixes(&text_under(&hunk.removed, at), &mut census);
        }
        if line.starts_with("<reference ") || line.starts_with("<target ") {
            let resolved = ["refuri", "refid"].iter().any(|attribute| {
                line.contains(&format!(" {attribute}=\""))
                    && unresolved.contains(&without_attribute(line, attribute))
            });
            if resolved {
                census.add(INDIRECT_TARGETS);
            }
        }
        if line.contains(" candidates=\"") {
            census.add(CANDIDATES);
        }
        if (line.starts_with("<figure ") || line.starts_with("<table "))
            && line.contains(" ids=\"id")
        {
            census.add(AUTONUMBER_IDS);
        }
        if line.starts_with("<section classes=") {
            census.add(CLASS_ATTRIBUTE);
        }
        let target_note_reference = line.starts_with("<footnote_reference ")
            && line
                .split(" refid=\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .is_some_and(|refid| notes.contains(refid));
        if line.contains("classes=\"sectnum\"")
            || line.contains("classes=\"contents\"")
            || line.contains("auto-toc")
            || line.contains("TARGET_NOTE")
            || line.starts_with("<title auto=\"1\"")
            || target_note_reference
        {
            census.add(PENDING_TRANSFORMS);
        }
        if line.starts_with("<system_message ") {
            let message = text_under(&hunk.removed, at + 1);
            census
                .unexplained
                .push(format!("a message only Sphinx's tree has: {message:?}"));
        }
    }

    if !docinfo {
        let mut our_messages: Vec<&str> = Vec::new();
        for (at, line) in added.iter().enumerate() {
            if let Some(message) = messages
                .get(hunk.added_from + at)
                .and_then(Option::as_deref)
            {
                if !our_messages.contains(&message) {
                    our_messages.push(message);
                }
                continue;
            }
            if line.starts_with("<paragraph classes=") && !starts(&removed, "<desc ") {
                census.add(CLASS_ATTRIBUTE);
            }
        }
        for message in our_messages {
            let pending = PENDING_DIRECTIVE_NAMES
                .iter()
                .any(|name| message.contains(&format!("\"{name}\"")));
            if pending {
                census.add(PENDING_DIRECTIVES);
            } else {
                census
                    .unexplained
                    .push(format!("a message only our tree has: {message:?}"));
            }
        }
    }

    if census.fixes.is_empty() && census.unexplained.is_empty() {
        census.unexplained.push(format!(
            "unexplained hunk\n- {}\n+ {}",
            hunk.removed.join("\n- "),
            hunk.added.join("\n+ ")
        ));
    }
    census
}
