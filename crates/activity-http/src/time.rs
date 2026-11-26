//! RFC 3339 to seconds since the Unix epoch, for the one timestamp the activity answer carries.

/// `2026-11-26T01:02:03Z`, `...03.5+01:00`: whole seconds since the Unix epoch, the fraction dropped. `None` for anything
/// else (a date with no time, a lower-case `t`, a month 13).
pub(crate) fn parse_rfc3339(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if !text.is_ascii() || b.len() < 20 {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> {
        let s = text.get(from..to)?;
        if s.bytes().all(|c| c.is_ascii_digit()) {
            s.parse().ok()
        } else {
            None
        }
    };
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (h, mi, s) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    let mut at = 19;
    if b[at] == b'.' {
        at += 1;
        let start = at;
        while at < b.len() && b[at].is_ascii_digit() {
            at += 1;
        }
        if at == start {
            return None;
        }
    }
    let offset = match b.get(at..)? {
        [b'Z'] => 0,
        [sign @ (b'+' | b'-'), ..] if b.len() == at + 6 && b[at + 3] == b':' => {
            let (oh, om) = (num(at + 1, at + 3)?, num(at + 4, at + 6)?);
            if oh > 23 || om > 59 {
                return None;
            }
            let secs = oh * 3600 + om * 60;
            if *sign == b'+' { secs } else { -secs }
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s - offset)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::parse_rfc3339;

    #[test]
    fn known_instants() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2000-02-29T12:00:00Z"), Some(951_825_600));
        assert_eq!(
            parse_rfc3339("2026-11-26T01:00:00.123456Z"),
            Some(1_795_654_800)
        );
        assert_eq!(
            parse_rfc3339("2026-11-26T02:00:00+01:00"),
            Some(1_795_654_800)
        );
        assert_eq!(
            parse_rfc3339("2026-11-25T20:00:00-05:00"),
            Some(1_795_654_800)
        );
        assert_eq!(parse_rfc3339("1969-12-31T23:59:59Z"), Some(-1));
    }

    #[test]
    fn refuses_what_is_not_a_timestamp() {
        for bad in [
            "",
            "tomorrow",
            "2026-11-26",
            "2026-11-26t01:00:00Z",
            "2026-13-26T01:00:00Z",
            "2026-11-26T25:00:00Z",
            "2026-11-26T01:00:00",
            "2026-11-26T01:00:00.Z",
            "2026-11-26T01:00:00+0100",
            "2026-11-26T01:00:00Zjunk",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }
}
