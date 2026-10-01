//! `java.time.LocalTime` as the service keeps its custom schedule: a time
//! of day in microseconds (`toNanoOfDay() / 1000`), printed and parsed as
//! `toString` and `LocalTime.parse` (ISO_LOCAL_TIME) do, and the Mac's
//! local clock.

pub const MICROS_PER_DAY: i64 = 86_400_000_000;

/// `LocalTime.toString`: `HH:mm`, then `:ss` and a fraction of 3, 6 or 9
/// digits as they are needed.
pub fn format(micros: i64) -> String {
    let nanos = micros * 1000;
    let (h, m) = (micros / 3_600_000_000, micros / 60_000_000 % 60);
    let (s, n) = (micros / 1_000_000 % 60, nanos % 1_000_000_000);
    let mut out = format!("{h:02}:{m:02}");
    if s > 0 || n > 0 {
        out += &format!(":{s:02}");
        if n % 1_000_000 == 0 {
            if n > 0 {
                out += &format!(".{:03}", n / 1_000_000);
            }
        } else if n % 1000 == 0 {
            out += &format!(".{:06}", n / 1000);
        } else {
            out += &format!(".{n:09}");
        }
    }
    out
}

/// `LocalTime.parse(text)` in microseconds of the day, or the exception it
/// throws as `toString` prints it.
pub fn parse(text: Option<&str>) -> Result<i64, String> {
    let Some(text) = text else {
        return Err("java.lang.NullPointerException: text".into());
    };
    let error = |detail: String| {
        format!(
            "java.time.format.DateTimeParseException: Text '{text}' could not be parsed{detail}"
        )
    };
    let b = text.as_bytes();
    let two = |at: usize| -> Option<i64> {
        let d = b.get(at..at + 2)?;
        d.iter()
            .all(u8::is_ascii_digit)
            .then(|| i64::from(d[0] - b'0') * 10 + i64::from(d[1] - b'0'))
    };
    let at = |i: usize| error(format!(" at index {i}"));
    let hour = two(0).ok_or_else(|| at(0))?;
    if b.get(2) != Some(&b':') {
        return Err(at(2));
    }
    let minute = two(3).ok_or_else(|| at(3))?;
    let (mut second, mut nano, mut end) = (0, 0, 5);
    if b.get(5) == Some(&b':') {
        second = two(6).ok_or_else(|| at(6))?;
        end = 8;
        if b.get(8) == Some(&b'.') {
            let digits = b[9..]
                .iter()
                .take(9)
                .take_while(|c| c.is_ascii_digit())
                .count();
            let fraction = &text[9..9 + digits];
            nano = format!("{fraction:0<9}").parse::<i64>().unwrap_or(0);
            end = 9 + digits;
        }
    }
    if end < b.len() {
        return Err(error(format!(", unparsed text found at index {end}")));
    }
    for (value, field, max) in [
        (hour, "HourOfDay", 23),
        (minute, "MinuteOfHour", 59),
        (second, "SecondOfMinute", 59),
    ] {
        if value > max {
            return Err(error(format!(
                ": Invalid value for {field} (valid values 0 - {max}): {value}"
            )));
        }
    }
    Ok((hour * 3600 + minute * 60 + second) * 1_000_000 + nano / 1000)
}

/// The Mac's local time now: microseconds of the day, and the epoch
/// microsecond it is.
pub fn now() -> (i64, i64) {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as i64);
    let secs = epoch.div_euclid(1_000_000) as libc::time_t;
    // SAFETY: localtime_r fills the zeroed struct from a valid time.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    };
    let of_day = (i64::from(tm.tm_hour) * 3600 + i64::from(tm.tm_min) * 60 + i64::from(tm.tm_sec))
        * 1_000_000
        + epoch.rem_euclid(1_000_000);
    (of_day, epoch)
}

/// `getDateTimeAfter`: the epoch microsecond of the next `time` of day at
/// or after now (today's, or tomorrow's once it passed), in the Mac's
/// local time.
pub fn next(time: i64) -> i64 {
    let (of_day, epoch) = now();
    let mut delta = time - of_day;
    if delta < 0 {
        delta += MICROS_PER_DAY;
    }
    epoch + delta
}

/// `TimeUtils.isTimeBetween(reference, start, end)`.
pub fn is_between(reference: i64, start: i64, end: i64) -> bool {
    let outside = (reference < start && reference > end)
        || (reference < end && reference < start && start < end)
        || (reference > end && reference > start && start < end);
    !outside
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000_000;

    #[test]
    fn prints_as_local_time() {
        assert_eq!(format(22 * H), "22:00");
        assert_eq!(format(6 * H + 30 * 60_000_000), "06:30");
        assert_eq!(format(H + 5_000_000), "01:00:05");
        assert_eq!(format(H + 5_250_000), "01:00:05.250");
        assert_eq!(format(1), "00:00:00.000001");
    }

    #[test]
    fn parses_as_local_time() {
        assert_eq!(parse(Some("22:00")), Ok(22 * H));
        assert_eq!(parse(Some("01:00:05.25")), Ok(H + 5_250_000));
        assert_eq!(
            parse(Some("noon")).unwrap_err(),
            "java.time.format.DateTimeParseException: Text 'noon' could not be parsed at index 0"
        );
        assert_eq!(
            parse(Some("25:00")).unwrap_err(),
            "java.time.format.DateTimeParseException: Text '25:00' could not be parsed: \
             Invalid value for HourOfDay (valid values 0 - 23): 25"
        );
        assert_eq!(
            parse(Some("10:00x")).unwrap_err(),
            "java.time.format.DateTimeParseException: Text '10:00x' could not be parsed, \
             unparsed text found at index 5"
        );
        assert_eq!(
            parse(None).unwrap_err(),
            "java.lang.NullPointerException: text"
        );
    }

    #[test]
    fn schedules_wrap_midnight() {
        // 22:00 to 06:00: night at 23:00 and 05:00, not at noon.
        assert!(is_between(23 * H, 22 * H, 6 * H));
        assert!(is_between(5 * H, 22 * H, 6 * H));
        assert!(!is_between(12 * H, 22 * H, 6 * H));
        // 08:00 to 17:00.
        assert!(is_between(12 * H, 8 * H, 17 * H));
        assert!(!is_between(20 * H, 8 * H, 17 * H));
        assert!(!is_between(7 * H, 8 * H, 17 * H));
    }
}
