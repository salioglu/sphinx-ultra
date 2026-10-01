//! Sphinx's `format_date` (`sphinx/util/i18n.py:177-313`): the build date
//! `|today|` reads, and the strftime-like tokens it is formatted with,
//! which Sphinx maps onto Babel patterns and formats with the project
//! language's CLDR data. The data ported here is Babel's English (`en`,
//! Babel 2.18.0 — the locale `language = 'en'`, Sphinx's default, names):
//! other languages format in English (a ledgered divergence: their month
//! and day names, date patterns and first weekday are Babel data this crate
//! does not carry).

use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};

use super::BuildDate;

/// The instant `format_date(..., date=None)` formats (`i18n.py:270-280`):
/// `$SOURCE_DATE_EPOCH` when it is set, else now — UTC either way.
pub(super) fn build_date(source: BuildDate) -> DateTime<Utc> {
    date_from(source, std::env::var("SOURCE_DATE_EPOCH").ok(), Utc::now)
}

/// [`build_date`] with the environment variable and the clock handed in. A
/// value Python's `float()` or `datetime.fromtimestamp` rejects — which
/// aborts the Sphinx build (`ValueError`/`OverflowError`) — falls back to
/// the clock here; so does a pinned [`BuildDate::Epoch`] outside the same
/// range.
fn date_from(
    source: BuildDate,
    source_date_epoch: Option<String>,
    now: impl FnOnce() -> DateTime<Utc>,
) -> DateTime<Utc> {
    match source {
        BuildDate::Epoch(seconds) => python_datetime(seconds).unwrap_or_else(now),
        BuildDate::Environment => source_date_epoch
            .as_deref()
            .and_then(from_epoch_value)
            .unwrap_or_else(now),
    }
}

/// The first and last whole seconds a Python `datetime` holds, as Unix
/// time: `datetime(1, 1, 1)` and `datetime(9999, 12, 31, 23, 59, 59)` in
/// UTC (`MINYEAR`/`MAXYEAR`).
const PYTHON_SECONDS: std::ops::RangeInclusive<i64> = -62_135_596_800..=253_402_300_799;

/// `datetime.fromtimestamp(seconds, tz=UTC)` for a whole second: `None`
/// where it raises — a year outside 1 to 9999 (`ValueError: year 33658 is
/// out of range` for `1e12`), which `chrono` would format.
fn python_datetime(seconds: i64) -> Option<DateTime<Utc>> {
    if PYTHON_SECONDS.contains(&seconds) {
        DateTime::from_timestamp(seconds, 0)
    } else {
        None
    }
}

/// `datetime.fromtimestamp(float(value), tz=UTC)` (`i18n.py:274`), to the
/// whole second every format token reads. Python's `float()` strips
/// surrounding whitespace and takes `_` between digits (PEP 515);
/// `fromtimestamp` rounds to the microsecond, half to even, so a fraction a
/// hair below one second carries into the next.
fn from_epoch_value(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim_matches(crate::utils::py_isspace);
    let chars: Vec<char> = value.chars().collect();
    let digit_at = |i: Option<usize>| {
        i.and_then(|i| chars.get(i))
            .is_some_and(char::is_ascii_digit)
    };
    let underscores_between_digits = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '_')
        .all(|(i, _)| digit_at(i.checked_sub(1)) && digit_at(Some(i + 1)));
    if !underscores_between_digits {
        return None;
    }
    let seconds: f64 = value.replace('_', "").parse().ok()?;
    if !seconds.is_finite() {
        return None;
    }
    let whole = seconds.floor();
    let micros = ((seconds - whole) * 1e6).round_ties_even();
    let whole = if micros >= 1e6 { whole + 1.0 } else { whole };
    // A value past `i64` saturates, far outside Python's years either way.
    python_datetime(whole as i64)
}

/// `date_format_mappings` (`i18n.py:177-215`) in its order, the order
/// `date_format_re`'s alternation tries them in (`:217`) — though no two
/// can match at one place.
const TOKENS: &[&str] = &[
    "%a", "%A", "%b", "%B", "%c", "%-d", "%d", "%-H", "%H", "%-I", "%I", "%-j", "%j", "%-m", "%m",
    "%-M", "%M", "%p", "%-S", "%S", "%U", "%w", "%-W", "%W", "%x", "%X", "%y", "%Y", "%Z", "%z",
    "%%",
];

/// `format_date(format, date=date, language='en')` (`i18n.py:263-313`):
/// `date_format_re.split(format)` — every token formatted through its
/// Babel pattern, everything between kept as it is (a `%` that starts no
/// token included).
///
/// `%U` and `%W` map onto Babel's `WW`, which Babel rejects (`Invalid
/// length for field: 'WW'`); Sphinx logs `Invalid Babel locale: 'en'.`,
/// retries in English, and the second `ValueError` aborts the build. Here
/// the token is kept as written.
pub(super) fn format_date(format: &str, date: DateTime<Utc>) -> String {
    let mut out = String::new();
    let mut rest = format;
    while let Some(c) = rest.chars().next() {
        match TOKENS.iter().find(|token| rest.starts_with(**token)) {
            Some(token) => {
                out.push_str(&english(token, date).unwrap_or_else(|| (*token).to_string()));
                rest = &rest[token.len()..];
            }
            None => {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    out
}

/// Babel's `en` abbreviated and wide day names, Monday first
/// (`date.weekday()` order).
const DAYS: [(&str, &str); 7] = [
    ("Mon", "Monday"),
    ("Tue", "Tuesday"),
    ("Wed", "Wednesday"),
    ("Thu", "Thursday"),
    ("Fri", "Friday"),
    ("Sat", "Saturday"),
    ("Sun", "Sunday"),
];

/// Babel's `en` abbreviated and wide month names.
const MONTHS: [(&str, &str); 12] = [
    ("Jan", "January"),
    ("Feb", "February"),
    ("Mar", "March"),
    ("Apr", "April"),
    ("May", "May"),
    ("Jun", "June"),
    ("Jul", "July"),
    ("Aug", "August"),
    ("Sep", "September"),
    ("Oct", "October"),
    ("Nov", "November"),
    ("Dec", "December"),
];

/// `Locale.parse('en').first_week_day` (Monday, `0`) and `min_week_days`
/// (`1`): `en` names no territory, so Babel takes the world's week rules.
const FIRST_WEEK_DAY: i64 = 0;
const MIN_WEEK_DAYS: i64 = 1;

/// One token through `babel_format_date` with its mapped pattern
/// (`i18n.py:220-260`) and Babel's `DateTimeFormat` (`babel/dates.py:
/// 1427-1750`) over the English data. `None` for the two patterns Babel
/// rejects.
fn english(token: &str, date: DateTime<Utc>) -> Option<String> {
    let weekday = date.weekday().num_days_from_monday() as usize;
    let month = date.month() as usize;
    Some(match token {
        "%a" => DAYS[weekday].0.to_string(),     // EEE
        "%A" => DAYS[weekday].1.to_string(),     // EEEE
        "%b" => MONTHS[month - 1].0.to_string(), // MMM
        "%B" => MONTHS[month - 1].1.to_string(), // MMMM
        "%c" => format!("{}, {}", medium_date(date), medium_time(date)), // medium
        "%-d" => date.day().to_string(),         // d
        "%d" => format!("{:02}", date.day()),    // dd
        "%-H" => date.hour().to_string(),        // H
        "%H" => format!("{:02}", date.hour()),   // HH
        "%-I" => hour12(date).to_string(),       // h
        "%I" => format!("{:02}", hour12(date)),  // hh
        "%-j" => date.ordinal().to_string(),     // D
        "%j" => format!("{:03}", date.ordinal()), // DDD
        "%-m" => month.to_string(),              // M
        "%m" => format!("{month:02}"),           // MM
        "%-M" => date.minute().to_string(),      // m
        "%M" => format!("{:02}", date.minute()), // mm
        "%p" => period(date).to_string(),        // a
        "%-S" => date.second().to_string(),      // s
        "%S" => format!("{:02}", date.second()), // ss
        // e: the local day of the week, 1 on `FIRST_WEEK_DAY`
        // (`format_weekday`, `dates.py:1582-1585`).
        "%w" => ((weekday as i64 + 7 - FIRST_WEEK_DAY).rem_euclid(7) + 1).to_string(),
        // W: the week of the month (`format_week`, `get_week_of_month`).
        "%-W" => week_number(i64::from(date.day()), weekday as i64).to_string(),
        "%x" => medium_date(date), // format_date medium
        "%X" => medium_time(date), // format_time medium
        // YY: the week-based year's last two digits (`format_year`,
        // `dates.py:1517-1528`).
        "%y" => {
            let year = format!("{:02}", week_based_year(date));
            year[year.len().saturating_sub(2)..].to_string()
        }
        "%Y" => format!("{:04}", date.year()), // yyyy
        // zzz / ZZZ of a UTC datetime: `local_time` is off, so every date
        // `format_date` sees is UTC.
        "%Z" => "UTC".to_string(),
        "%z" => "+0000".to_string(),
        "%%" => "%".to_string(),
        _ => return None, // %U, %W: `WW`
    })
}

/// `en`'s medium date pattern, `MMM d, y`.
fn medium_date(date: DateTime<Utc>) -> String {
    let (abbreviated, _) = MONTHS[date.month() as usize - 1];
    format!("{abbreviated} {}, {}", date.day(), date.year())
}

/// `en`'s medium time pattern, `h:mm:ss\u202fa` (a narrow no-break space
/// before the period, CLDR 42 on).
fn medium_time(date: DateTime<Utc>) -> String {
    format!(
        "{}:{:02}:{:02}\u{202f}{}",
        hour12(date),
        date.minute(),
        date.second(),
        period(date)
    )
}

/// `h`: the hour on a 12-hour clock, 12 for 0.
fn hour12(date: DateTime<Utc>) -> u32 {
    match date.hour() % 12 {
        0 => 12,
        hour => hour,
    }
}

/// `a`: `en`'s abbreviated am/pm period names.
fn period(date: DateTime<Utc>) -> &'static str {
    if date.hour() >= 12 {
        "PM"
    } else {
        "AM"
    }
}

/// `DateTimeFormat.get_week_number` (`dates.py:1728-1750`): the week of a
/// period (a month or a year) its `day_of_period`-th day falls in, `0` when
/// the period's first week is too short to count as its own.
fn week_number(day_of_period: i64, day_of_week: i64) -> i64 {
    let first_day = (day_of_week - FIRST_WEEK_DAY - day_of_period + 1).rem_euclid(7);
    let week = (day_of_period + first_day - 1).div_euclid(7);
    if 7 - first_day >= MIN_WEEK_DAYS {
        week + 1
    } else {
        week
    }
}

/// `DateTimeFormat.get_week_of_year` (`dates.py:1708-1722`).
fn week_of_year(date: DateTime<Utc>) -> i64 {
    let day_of_week = |d: NaiveDate| i64::from(d.weekday().num_days_from_monday());
    let week = week_number(
        i64::from(date.ordinal()),
        i64::from(date.weekday().num_days_from_monday()),
    );
    if week == 0 {
        NaiveDate::from_ymd_opt(date.year() - 1, 12, 31).map_or(week, |eve| {
            week_number(i64::from(eve.ordinal()), day_of_week(eve))
        })
    } else if week > 52 {
        match NaiveDate::from_ymd_opt(date.year() + 1, 1, 1) {
            Some(next) => {
                let weekday = day_of_week(next);
                if week_number(1, weekday) == 1
                    && 32 - (weekday - FIRST_WEEK_DAY).rem_euclid(7) <= i64::from(date.day())
                {
                    1
                } else {
                    week
                }
            }
            None => week,
        }
    } else {
        week
    }
}

/// `format_year`'s `Y` (`dates.py:1517-1528`): the calendar year, less one
/// for early-January days of the previous year's last week, plus one for
/// late-December days of the next year's first week.
fn week_based_year(date: DateTime<Utc>) -> i32 {
    let year = date.year();
    if date.month() == 1 && date.day() < 7 && week_of_year(date) >= 52 {
        year - 1
    } else if date.month() == 12 && date.day() > 25 && week_of_year(date) <= 2 {
        year + 1
    } else {
        year
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ts: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(ts, 0).unwrap()
    }

    /// Every token `date_format_mappings` maps (`i18n.py:177-215`) but the
    /// two whose Babel pattern raises, separated by literal `|`s.
    const EVERY_TOKEN: &str = "%a|%A|%b|%B|%c|%-d|%d|%-H|%H|%-I|%I|%-j|%j|%-m|%m|%-M|%M|%p|\
                               %-S|%S|%w|%-W|%x|%X|%y|%Y|%Z|%z|%%";

    /// Probed: `format_date(EVERY_TOKEN, date=datetime.fromtimestamp(ts,
    /// tz=UTC), language='en')` under Sphinx 9.1.0 / Babel 2.18.0, one date
    /// per row — the week-numbered fields (`%-W` week of month, `%w` local
    /// weekday, `%y` week-based year) across year ends, and 12-hour clock
    /// edges at midnight, noon and one past.
    #[test]
    fn every_token_formats_as_sphinx_formats_it_in_english() {
        #[rustfmt::skip]
        let rows: &[(i64, &str)] = &[
            (0, "Thu|Thursday|Jan|January|Jan 1, 1970, 12:00:00\u{202f}AM|1|01|0|00|12|12|1|001|1|01|0|00|AM|0|00|4|1|Jan 1, 1970|12:00:00\u{202f}AM|70|1970|UTC|+0000|%"), // 1970-01-01T00:00:00+00:00
            (1234567890, "Fri|Friday|Feb|February|Feb 13, 2009, 11:31:30\u{202f}PM|13|13|23|23|11|11|44|044|2|02|31|31|PM|30|30|5|3|Feb 13, 2009|11:31:30\u{202f}PM|09|2009|UTC|+0000|%"), // 2009-02-13T23:31:30+00:00
            (1759190400, "Tue|Tuesday|Sep|September|Sep 30, 2025, 12:00:00\u{202f}AM|30|30|0|00|12|12|273|273|9|09|0|00|AM|0|00|2|5|Sep 30, 2025|12:00:00\u{202f}AM|25|2025|UTC|+0000|%"), // 2025-09-30T00:00:00+00:00
            (1767225599, "Wed|Wednesday|Dec|December|Dec 31, 2025, 11:59:59\u{202f}PM|31|31|23|23|11|11|365|365|12|12|59|59|PM|59|59|3|5|Dec 31, 2025|11:59:59\u{202f}PM|26|2025|UTC|+0000|%"), // 2025-12-31T23:59:59+00:00
            (1704067200, "Mon|Monday|Jan|January|Jan 1, 2024, 12:00:00\u{202f}AM|1|01|0|00|12|12|1|001|1|01|0|00|AM|0|00|1|1|Jan 1, 2024|12:00:00\u{202f}AM|24|2024|UTC|+0000|%"), // 2024-01-01T00:00:00+00:00
            (1735603200, "Tue|Tuesday|Dec|December|Dec 31, 2024, 12:00:00\u{202f}AM|31|31|0|00|12|12|366|366|12|12|0|00|AM|0|00|2|6|Dec 31, 2024|12:00:00\u{202f}AM|25|2024|UTC|+0000|%"), // 2024-12-31T00:00:00+00:00
            (1609459200, "Fri|Friday|Jan|January|Jan 1, 2021, 12:00:00\u{202f}AM|1|01|0|00|12|12|1|001|1|01|0|00|AM|0|00|5|1|Jan 1, 2021|12:00:00\u{202f}AM|21|2021|UTC|+0000|%"), // 2021-01-01T00:00:00+00:00
            (1641038400, "Sat|Saturday|Jan|January|Jan 1, 2022, 12:00:00\u{202f}PM|1|01|12|12|12|12|1|001|1|01|0|00|PM|0|00|6|1|Jan 1, 2022|12:00:00\u{202f}PM|22|2022|UTC|+0000|%"), // 2022-01-01T12:00:00+00:00
            (946684800, "Sat|Saturday|Jan|January|Jan 1, 2000, 12:00:00\u{202f}AM|1|01|0|00|12|12|1|001|1|01|0|00|AM|0|00|6|1|Jan 1, 2000|12:00:00\u{202f}AM|00|2000|UTC|+0000|%"), // 2000-01-01T00:00:00+00:00
            (951782400, "Tue|Tuesday|Feb|February|Feb 29, 2000, 12:00:00\u{202f}AM|29|29|0|00|12|12|60|060|2|02|0|00|AM|0|00|2|5|Feb 29, 2000|12:00:00\u{202f}AM|00|2000|UTC|+0000|%"), // 2000-02-29T00:00:00+00:00
            (-86400, "Wed|Wednesday|Dec|December|Dec 31, 1969, 12:00:00\u{202f}AM|31|31|0|00|12|12|365|365|12|12|0|00|AM|0|00|3|5|Dec 31, 1969|12:00:00\u{202f}AM|70|1969|UTC|+0000|%"), // 1969-12-31T00:00:00+00:00
            (1356998400, "Tue|Tuesday|Jan|January|Jan 1, 2013, 12:00:00\u{202f}AM|1|01|0|00|12|12|1|001|1|01|0|00|AM|0|00|2|1|Jan 1, 2013|12:00:00\u{202f}AM|13|2013|UTC|+0000|%"), // 2013-01-01T00:00:00+00:00
            (1388448000, "Tue|Tuesday|Dec|December|Dec 31, 2013, 12:00:00\u{202f}AM|31|31|0|00|12|12|365|365|12|12|0|00|AM|0|00|2|6|Dec 31, 2013|12:00:00\u{202f}AM|14|2013|UTC|+0000|%"), // 2013-12-31T00:00:00+00:00
            (1420070399, "Wed|Wednesday|Dec|December|Dec 31, 2014, 11:59:59\u{202f}PM|31|31|23|23|11|11|365|365|12|12|59|59|PM|59|59|3|5|Dec 31, 2014|11:59:59\u{202f}PM|15|2014|UTC|+0000|%"), // 2014-12-31T23:59:59+00:00
            (1451606400, "Fri|Friday|Jan|January|Jan 1, 2016, 12:00:00\u{202f}AM|1|01|0|00|12|12|1|001|1|01|0|00|AM|0|00|5|1|Jan 1, 2016|12:00:00\u{202f}AM|16|2016|UTC|+0000|%"), // 2016-01-01T00:00:00+00:00
            (1483142400, "Sat|Saturday|Dec|December|Dec 31, 2016, 12:00:00\u{202f}AM|31|31|0|00|12|12|366|366|12|12|0|00|AM|0|00|6|5|Dec 31, 2016|12:00:00\u{202f}AM|17|2016|UTC|+0000|%"), // 2016-12-31T00:00:00+00:00
            (1483228800, "Sun|Sunday|Jan|January|Jan 1, 2017, 12:00:00\u{202f}AM|1|01|0|00|12|12|1|001|1|01|0|00|AM|0|00|7|1|Jan 1, 2017|12:00:00\u{202f}AM|17|2017|UTC|+0000|%"), // 2017-01-01T00:00:00+00:00
            (1514764800, "Mon|Monday|Jan|January|Jan 1, 2018, 12:00:00\u{202f}AM|1|01|0|00|12|12|1|001|1|01|0|00|AM|0|00|1|1|Jan 1, 2018|12:00:00\u{202f}AM|18|2018|UTC|+0000|%"), // 2018-01-01T00:00:00+00:00
            (1546214400, "Mon|Monday|Dec|December|Dec 31, 2018, 12:00:00\u{202f}AM|31|31|0|00|12|12|365|365|12|12|0|00|AM|0|00|1|6|Dec 31, 2018|12:00:00\u{202f}AM|19|2018|UTC|+0000|%"), // 2018-12-31T00:00:00+00:00
            (1577750400, "Tue|Tuesday|Dec|December|Dec 31, 2019, 12:00:00\u{202f}AM|31|31|0|00|12|12|365|365|12|12|0|00|AM|0|00|2|6|Dec 31, 2019|12:00:00\u{202f}AM|20|2019|UTC|+0000|%"), // 2019-12-31T00:00:00+00:00
            (1672444800, "Sat|Saturday|Dec|December|Dec 31, 2022, 12:00:00\u{202f}AM|31|31|0|00|12|12|365|365|12|12|0|00|AM|0|00|6|5|Dec 31, 2022|12:00:00\u{202f}AM|23|2022|UTC|+0000|%"), // 2022-12-31T00:00:00+00:00
            (1703980800, "Sun|Sunday|Dec|December|Dec 31, 2023, 12:00:00\u{202f}AM|31|31|0|00|12|12|365|365|12|12|0|00|AM|0|00|7|5|Dec 31, 2023|12:00:00\u{202f}AM|23|2023|UTC|+0000|%"), // 2023-12-31T00:00:00+00:00
            (1693526400, "Fri|Friday|Sep|September|Sep 1, 2023, 12:00:00\u{202f}AM|1|01|0|00|12|12|244|244|9|09|0|00|AM|0|00|5|1|Sep 1, 2023|12:00:00\u{202f}AM|23|2023|UTC|+0000|%"), // 2023-09-01T00:00:00+00:00
            (1696118400, "Sun|Sunday|Oct|October|Oct 1, 2023, 12:00:00\u{202f}AM|1|01|0|00|12|12|274|274|10|10|0|00|AM|0|00|7|1|Oct 1, 2023|12:00:00\u{202f}AM|23|2023|UTC|+0000|%"), // 2023-10-01T00:00:00+00:00
            (1701388800, "Fri|Friday|Dec|December|Dec 1, 2023, 12:00:00\u{202f}AM|1|01|0|00|12|12|335|335|12|12|0|00|AM|0|00|5|1|Dec 1, 2023|12:00:00\u{202f}AM|23|2023|UTC|+0000|%"), // 2023-12-01T00:00:00+00:00
            (1709251200, "Fri|Friday|Mar|March|Mar 1, 2024, 12:00:00\u{202f}AM|1|01|0|00|12|12|61|061|3|03|0|00|AM|0|00|5|1|Mar 1, 2024|12:00:00\u{202f}AM|24|2024|UTC|+0000|%"), // 2024-03-01T00:00:00+00:00
            (1719792000, "Mon|Monday|Jul|July|Jul 1, 2024, 12:00:00\u{202f}AM|1|01|0|00|12|12|183|183|7|07|0|00|AM|0|00|1|1|Jul 1, 2024|12:00:00\u{202f}AM|24|2024|UTC|+0000|%"), // 2024-07-01T00:00:00+00:00
            (1725148800, "Sun|Sunday|Sep|September|Sep 1, 2024, 12:00:00\u{202f}AM|1|01|0|00|12|12|245|245|9|09|0|00|AM|0|00|7|1|Sep 1, 2024|12:00:00\u{202f}AM|24|2024|UTC|+0000|%"), // 2024-09-01T00:00:00+00:00
            (1727740800, "Tue|Tuesday|Oct|October|Oct 1, 2024, 12:00:00\u{202f}AM|1|01|0|00|12|12|275|275|10|10|0|00|AM|0|00|2|1|Oct 1, 2024|12:00:00\u{202f}AM|24|2024|UTC|+0000|%"), // 2024-10-01T00:00:00+00:00
            (1730419200, "Fri|Friday|Nov|November|Nov 1, 2024, 12:00:00\u{202f}AM|1|01|0|00|12|12|306|306|11|11|0|00|AM|0|00|5|1|Nov 1, 2024|12:00:00\u{202f}AM|24|2024|UTC|+0000|%"), // 2024-11-01T00:00:00+00:00
            (1733058309, "Sun|Sunday|Dec|December|Dec 1, 2024, 1:05:09\u{202f}PM|1|01|13|13|1|01|336|336|12|12|5|05|PM|9|09|7|1|Dec 1, 2024|1:05:09\u{202f}PM|24|2024|UTC|+0000|%"), // 2024-12-01T13:05:09+00:00
            (1717243200, "Sat|Saturday|Jun|June|Jun 1, 2024, 12:00:00\u{202f}PM|1|01|12|12|12|12|153|153|6|06|0|00|PM|0|00|6|1|Jun 1, 2024|12:00:00\u{202f}PM|24|2024|UTC|+0000|%"), // 2024-06-01T12:00:00+00:00
            (1717243140, "Sat|Saturday|Jun|June|Jun 1, 2024, 11:59:00\u{202f}AM|1|01|11|11|11|11|153|153|6|06|59|59|AM|0|00|6|1|Jun 1, 2024|11:59:00\u{202f}AM|24|2024|UTC|+0000|%"), // 2024-06-01T11:59:00+00:00
            (1717200060, "Sat|Saturday|Jun|June|Jun 1, 2024, 12:01:00\u{202f}AM|1|01|0|00|12|12|153|153|6|06|1|01|AM|0|00|6|1|Jun 1, 2024|12:01:00\u{202f}AM|24|2024|UTC|+0000|%"), // 2024-06-01T00:01:00+00:00
        ];
        for (ts, expected) in rows {
            assert_eq!(format_date(EVERY_TOKEN, at(*ts)), *expected, "at {ts}");
        }
    }

    /// Text outside the mapped tokens passes through untouched, a `%` that
    /// starts no token included (`date_format_re.split`, `i18n.py:217,
    /// 285-311`); `%%` is itself a token, so `%%Y` is a literal `%Y`.
    /// Probed at 1234567890.
    #[test]
    fn text_outside_the_tokens_passes_through() {
        let date = at(1_234_567_890);
        for (format, expected) in [
            ("%b %d, %Y", "Feb 13, 2009"),
            ("'%Y'", "'2009'"),
            ("a'b", "a'b"),
            ("%%Y", "%Y"),
            ("%e", "%e"),
            ("%-y", "%-y"),
            ("%", "%"),
            ("x%", "x%"),
            ("%e%Y", "%e2009"),
            ("", ""),
        ] {
            assert_eq!(format_date(format, date), expected, "{format:?}");
        }
    }

    /// `datetime.fromtimestamp(float(SOURCE_DATE_EPOCH), tz=UTC)`
    /// (`i18n.py:273-275`): Python's `float()` (surrounding whitespace, an
    /// exponent, a fraction) and the instant's whole second, a negative
    /// fraction rounding down. Probed through `format_date('%Y-%m-%d
    /// %H:%M:%S', language='en')`.
    #[test]
    fn source_date_epoch_reads_as_python_reads_it() {
        for (value, expected) in [
            ("1234567890", "2009-02-13 23:31:30"),
            ("1234567890.9", "2009-02-13 23:31:30"),
            (" 1234567890 ", "2009-02-13 23:31:30"),
            ("1e9", "2001-09-09 01:46:40"),
            ("-1.5", "1969-12-31 23:59:58"),
            ("0", "1970-01-01 00:00:00"),
        ] {
            let date = from_epoch_value(value).unwrap_or_else(|| panic!("{value:?} unread"));
            assert_eq!(
                format_date("%Y-%m-%d %H:%M:%S", date),
                expected,
                "{value:?}"
            );
        }
    }

    /// Python's `datetime` holds the years 1 to 9999, and `fromtimestamp`
    /// reaches both ends (probed, Sphinx 9.1.0 on Python 3.12: these format
    /// as below); a `_` between digits is PEP 515's.
    #[test]
    fn source_date_epoch_reads_to_the_ends_of_pythons_years() {
        for (value, expected) in [
            ("253402300799", "9999-12-31 23:59:59"),
            ("-62135596800", "0001-01-01 00:00:00"),
            ("1_0", "1970-01-01 00:00:10"),
        ] {
            let date = from_epoch_value(value).unwrap_or_else(|| panic!("{value:?} unread"));
            assert_eq!(
                format_date("%Y-%m-%d %H:%M:%S", date),
                expected,
                "{value:?}"
            );
        }
    }

    /// Every value `format_date` raises on (probed, Sphinx 9.1.0 on Python
    /// 3.12) — `float()`'s `ValueError` for garbage, `fromtimestamp`'s
    /// `OverflowError` for an infinity and `ValueError` for NaN or a year
    /// outside 1-9999 (`1e12` is "year 33658 is out of range"; a fraction
    /// rounding past the last second or below the first leaves the range
    /// too) — aborts the Sphinx build; here the build date falls back to
    /// the clock. The same range bounds a pinned [`BuildDate::Epoch`].
    #[test]
    fn a_source_date_epoch_python_rejects_falls_back_to_the_clock() {
        let clock = || at(1_700_000_000);
        for value in [
            "garbage",
            "",
            "1__0",
            "_1",
            "inf",
            "-inf",
            "nan",
            "1e12",
            "-1e11",
            "253402300800",
            "253402300799.9999994",
            "-62135596801",
            "-62135596800.4",
        ] {
            assert_eq!(
                date_from(BuildDate::Environment, Some(value.into()), clock),
                clock(),
                "{value:?}"
            );
        }
        for seconds in [253_402_300_800, -62_135_596_801, i64::MAX, i64::MIN] {
            assert_eq!(
                date_from(BuildDate::Epoch(seconds), None, clock),
                clock(),
                "{seconds}"
            );
        }
    }

    /// `%U` and `%W` map onto Babel's `WW`, which Babel rejects: Sphinx
    /// logs `Invalid Babel locale: 'en'.` and the build aborts with
    /// `ValueError: Invalid length for field: 'WW'` (probed). Here the
    /// token stays as written, the rest formatted.
    #[test]
    fn the_week_of_year_tokens_stay_as_written() {
        assert_eq!(
            format_date("%U|%W|a%Ub|%Y", at(1_234_567_890)),
            "%U|%W|a%Ub|2009"
        );
    }

    /// The `date=None` branch (`i18n.py:270-280`): `SOURCE_DATE_EPOCH` when
    /// it is set, else now; a pinned [`BuildDate::Epoch`] reads neither.
    #[test]
    fn the_build_date_prefers_source_date_epoch_to_the_clock() {
        let clock = || at(1_700_000_000);
        assert_eq!(
            date_from(BuildDate::Environment, Some("1234567890".into()), clock),
            at(1_234_567_890)
        );
        assert_eq!(date_from(BuildDate::Environment, None, clock), clock());
        assert_eq!(
            date_from(
                BuildDate::Epoch(946_684_800),
                Some("1234567890".into()),
                clock
            ),
            at(946_684_800)
        );
    }
}
