//! Line-by-line comparison of two texts, shared by the differential
//! harnesses' mismatch reports.

/// The first line, numbered from 1, where two texts differ, and that line
/// of each (empty past a text's end); `None` when every line is the same,
/// which two texts that differ only in a final line break are.
pub fn first_differing_line<'a>(
    expected: &'a str,
    actual: &'a str,
) -> Option<(usize, &'a str, &'a str)> {
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    let mut number = 1;
    loop {
        match (expected_lines.next(), actual_lines.next()) {
            (None, None) => return None,
            (left, right) if left == right => number += 1,
            (left, right) => {
                return Some((number, left.unwrap_or_default(), right.unwrap_or_default()))
            }
        }
    }
}
