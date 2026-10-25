//! The guest's tzdata (`/apex/com.android.tzdata`), as bionic and libcore
//! resolve zone names in it.
//! Calendar resolution: Copyright (C) The Android Open Source Project, Apache License 2.0.

/// Where the image has it.
pub const PATH: &str = "/apex/com.android.tzdata/etc/tz/tzdata";

/// Whether bionic's tzdata file (`ZoneInfoDb`: a 12-byte version, the
/// index, data and final offsets, then 52-byte entries of a 40-byte name
/// and three integers) has a zone called `name`.
pub fn has(tzdata: &[u8], name: &str) -> bool {
    zone(tzdata, name).is_some()
}

/// The TZif data of zone `name`: an entry's offset (from the data offset)
/// and length.
pub fn zone<'a>(tzdata: &'a [u8], name: &str) -> Option<&'a [u8]> {
    let be = |at: usize| -> Option<usize> {
        Some(u32::from_be_bytes(tzdata.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    if !tzdata.starts_with(b"tzdata") || name.len() >= 40 {
        return None;
    }
    let (index, data) = (be(12)?, be(16)?);
    let entries = tzdata.get(index..data.min(tzdata.len()))?;
    let entry = entries.as_chunks::<52>().0.iter().find(|entry| {
        let stored = &entry[..40];
        let end = stored.iter().position(|&b| b == 0).unwrap_or(40);
        &stored[..end] == name.as_bytes()
    })?;
    let word = |at: usize| u32::from_be_bytes(entry[at..at + 4].try_into().unwrap()) as usize;
    let start = data.checked_add(word(40))?;
    tzdata.get(start..start.checked_add(word(44))?)
}

/// A zone's rules (RFC 8536 TZif, its 64-bit data and footer): the
/// offset of each type, the transitions to them, and the POSIX TZ rule
/// after the last.
#[derive(Debug, PartialEq)]
pub struct Rules {
    transitions: Vec<(i64, i32)>,
    daylight: Vec<bool>,
    earliest_standard: i32,
    initial: i32,
    footer: Option<Posix>,
}

impl Rules {
    pub fn parse(tzif: &[u8]) -> Option<Rules> {
        let header = |at: usize| -> Option<(u8, [usize; 6])> {
            let h = tzif.get(at..at + 44)?;
            if &h[..4] != b"TZif" {
                return None;
            }
            let mut counts = [0; 6];
            for (i, c) in counts.iter_mut().enumerate() {
                *c = u32::from_be_bytes(h[20 + 4 * i..24 + 4 * i].try_into().ok()?) as usize;
            }
            Some((h[4], counts))
        };
        let (version, v1) = header(0)?;
        // Each block: times, type indices, types, designations, leap
        // seconds, standard/wall and UT/local indicators.
        let size = |c: [usize; 6], time: usize| {
            c[3] * time + c[3] + c[4] * 6 + c[5] + c[2] * (time + 4) + c[1] + c[0]
        };
        let (at, time, counts) = if version >= b'2' {
            let second = 44 + size(v1, 4);
            (second + 44, 8, header(second)?.1)
        } else {
            (44, 4, v1)
        };
        let [_, _, _, timecnt, typecnt, _] = counts;
        let data = tzif.get(at..at + size(counts, time))?;
        let types: Vec<i32> = (0..typecnt)
            .map(|i| {
                let o = timecnt * (time + 1) + 6 * i;
                i32::from_be_bytes(data[o..o + 4].try_into().unwrap())
            })
            .collect();
        let is_dst = |i: usize| data[timecnt * (time + 1) + 6 * i + 4] != 0;
        let daylight = (0..timecnt)
            .map(|i| {
                let kind = data[timecnt * time + i] as usize;
                (kind < typecnt).then(|| is_dst(kind))
            })
            .collect::<Option<Vec<_>>>()?;
        let transitions = (0..timecnt)
            .map(|i| {
                let t = &data[i * time..(i + 1) * time];
                let t = if time == 8 {
                    i64::from_be_bytes(t.try_into().unwrap())
                } else {
                    i32::from_be_bytes(t.try_into().unwrap()) as i64
                };
                Some((t, *types.get(data[timecnt * time + i] as usize)?))
            })
            .collect::<Option<Vec<_>>>()?;
        let footer = if version >= b'2' {
            let rest = tzif.get(at + size(counts, time)..)?;
            let text = std::str::from_utf8(rest.strip_prefix(b"\n")?).ok()?;
            let line = &text[..text.find('\n')?];
            if line.is_empty() {
                None
            } else {
                Some(Posix::parse(line)?)
            }
        } else {
            None
        };
        Some(Rules {
            transitions,
            daylight,
            earliest_standard: (0..typecnt)
                .find(|&i| !is_dst(i))
                .map(|i| types[i])
                .unwrap_or(*types.first()?),
            initial: *types.first()?,
            footer,
        })
    }

    /// Android's `GregorianCalendar` in `ZoneInfo`: normalize lenient fields,
    /// estimate UTC with the latest raw offset, then adjust historical raw/DST
    /// offsets, clearing DST in a gap. Unlike java.time, overlaps use standard
    /// time. `ZoneInfoData` uses TZif transitions, without the POSIX footer.
    pub fn calendar_offset(&self, fields: [i32; 6]) -> i32 {
        let [year, month, day, hour, minute, second] = fields;
        let month = month.wrapping_sub(1) as i64;
        let year = year as i64 + month.div_euclid(12);
        let month = month.rem_euclid(12) + 1;
        let clock = hour as i64 * 3600 + minute as i64 * 60 + second as i64;
        let day = day as i64 + clock.div_euclid(86_400);
        // ZIP entries are DOS times, from 1980: always Gregorian.
        let days = days_from_civil(year, month, day);
        let local = (days * 86_400 + clock.rem_euclid(86_400))
            .wrapping_mul(1000)
            .div_euclid(1000);
        let raw = self
            .transitions
            .iter()
            .zip(&self.daylight)
            .rfind(|(_, dst)| !**dst)
            .map_or(self.initial, |(t, _)| t.1);
        let parts = |utc: i64| {
            let end = self.transitions.partition_point(|t| t.0 <= utc);
            let total = self
                .transitions
                .get(end.wrapping_sub(1))
                .map_or(self.earliest_standard, |t| t.1);
            let dst = end != 0 && self.daylight[end - 1];
            let standard = if dst {
                self.transitions[..end]
                    .iter()
                    .zip(&self.daylight[..end])
                    .rfind(|(_, dst)| !**dst)
                    .map_or(self.earliest_standard, |(t, _)| t.1)
            } else {
                total
            };
            (standard, total - standard, dst)
        };
        let estimate = local - raw as i64;
        let (standard, mut dst, _) = parts(estimate);
        if dst != 0 && !parts(estimate - dst as i64).2 {
            dst = 0;
        }
        let (raw, dst, _) = parts(local - standard as i64 - dst as i64);
        raw + dst
    }

    /// The transitions (UTC seconds and the offset after) around year
    /// `year`: the zone's own, then its footer rule's beyond the last.
    fn transitions_near(&self, year: i64) -> Vec<(i64, i32)> {
        let mut out = self.transitions.clone();
        if let Some(rule) = &self.footer {
            let last = out.last().map_or(i64::MIN, |t| t.0);
            for y in year - 1..=year + 1 {
                out.extend(rule.transitions(y).into_iter().filter(|t| t.0 > last));
            }
            out.sort_by_key(|t| t.0);
        }
        out
    }

    /// The offset at UTC second `utc`.
    fn offset_at(&self, list: &[(i64, i32)], utc: i64) -> i32 {
        match list.iter().rposition(|t| t.0 <= utc) {
            Some(i) => list[i].1,
            None if self.transitions.is_empty() => {
                self.footer.as_ref().map_or(self.initial, |f| f.std)
            }
            None => self.initial,
        }
    }

    /// `ZonedDateTime.of(local, zone).toInstant()`: the UTC second of local
    /// second `local`; in a gap, as moved past it by the gap's length; in
    /// an overlap, at the earlier offset.
    pub fn local_to_utc(&self, local: i64) -> i64 {
        let list = self.transitions_near(local.div_euclid(86_400 * 365) + 1970);
        let mut previous = self.offset_at(&list, i64::MIN);
        let mut valid: Vec<i32> = Vec::new();
        for &(t, after) in &list {
            // The local times each side of the transition covers.
            if local < t + previous as i64 && local >= t + after as i64 {
                valid.push(previous);
                valid.push(after);
            } else if local >= t + previous as i64 && local < t + after as i64 {
                // A gap: the local time is moved forward by its length.
                return local - previous as i64;
            }
            previous = after;
        }
        if let Some(earlier) = valid.iter().max() {
            return local - *earlier as i64;
        }
        // Neither: the offset of the last transition the local time is past.
        let mut offset = self.offset_at(&list, i64::MIN);
        for &(t, after) in &list {
            if local >= t + after as i64 {
                offset = after;
            }
        }
        local - offset as i64
    }
}

/// A POSIX TZ rule (`std offset [dst [offset] ,start[/time],end[/time]]`)
/// in UTC offsets east of Greenwich.
#[derive(Debug, PartialEq)]
struct Posix {
    std: i32,
    dst: Option<(i32, Change, Change)>,
}

/// A rule's day (`Jn`, `n` or `Mm.w.d`) and local time of a change.
#[derive(Debug, PartialEq)]
enum Day {
    Julian(i64),
    Zero(i64),
    Month(i64, i64, i64),
}

#[derive(Debug, PartialEq)]
struct Change {
    day: Day,
    time: i64,
}

impl Posix {
    fn parse(s: &str) -> Option<Posix> {
        let mut rest = s;
        let name = |rest: &mut &str| -> Option<()> {
            let len = if let Some(quoted) = rest.strip_prefix('<') {
                quoted.find('>')? + 2
            } else {
                rest.find(|c: char| !c.is_ascii_alphabetic())
                    .unwrap_or(rest.len())
            };
            (len >= 3).then_some(())?;
            *rest = &rest[len..];
            Some(())
        };
        // `[+-]hh[:mm[:ss]]` west of Greenwich, as seconds.
        let time = |rest: &mut &str| -> Option<i64> {
            let (sign, body) = match rest.as_bytes().first()? {
                b'-' => (-1, &rest[1..]),
                b'+' => (1, &rest[1..]),
                _ => (1, &rest[..]),
            };
            let end = body
                .find(|c: char| !c.is_ascii_digit() && c != ':')
                .unwrap_or(body.len());
            let mut seconds = 0;
            let mut unit = 3600;
            for part in body[..end].split(':') {
                seconds += part.parse::<i64>().ok()? * unit;
                unit /= 60;
            }
            *rest = &body[end..];
            Some(sign * seconds)
        };
        name(&mut rest)?;
        let std = -time(&mut rest)?;
        if rest.is_empty() {
            return Some(Posix {
                std: std as i32,
                dst: None,
            });
        }
        name(&mut rest)?;
        let dst = if rest.starts_with(',') {
            std + 3600
        } else {
            -time(&mut rest)?
        };
        let change = |rest: &mut &str| -> Option<Change> {
            *rest = rest.strip_prefix(',')?;
            let day = if let Some(m) = rest.strip_prefix('M') {
                let end = m.find(['/', ',']).unwrap_or(m.len());
                let mut parts = m[..end].split('.').map(|p| p.parse::<i64>().ok());
                let day = Day::Month(parts.next()??, parts.next()??, parts.next()??);
                *rest = &m[end..];
                day
            } else {
                let julian = rest.starts_with('J');
                let body = if julian { &rest[1..] } else { &rest[..] };
                let end = body.find(['/', ',']).unwrap_or(body.len());
                let n = body[..end].parse().ok()?;
                *rest = &body[end..];
                if julian { Day::Julian(n) } else { Day::Zero(n) }
            };
            let time = match rest.strip_prefix('/') {
                Some(t) => {
                    *rest = t;
                    time(rest)?
                }
                None => 7200,
            };
            Some(Change { day, time })
        };
        let start = change(&mut rest)?;
        let end = change(&mut rest)?;
        rest.is_empty().then_some(())?;
        Some(Posix {
            std: std as i32,
            dst: Some((dst as i32, start, end)),
        })
    }

    /// Year `year`'s two changes, as UTC seconds and the offset after.
    fn transitions(&self, year: i64) -> Vec<(i64, i32)> {
        let Some((dst, start, end)) = &self.dst else {
            return Vec::new();
        };
        let at = |change: &Change, before: i32| {
            let day = match change.day {
                Day::Julian(n) => {
                    let leap = n >= 60 && is_leap(year);
                    days_from_civil(year, 1, 1) + n - 1 + leap as i64
                }
                Day::Zero(n) => days_from_civil(year, 1, 1) + n,
                Day::Month(m, w, d) => {
                    let first = days_from_civil(year, m, 1);
                    let weekday = (first + 4).rem_euclid(7);
                    let mut day = first + (d - weekday).rem_euclid(7) + (w - 1) * 7;
                    let next = if m == 12 {
                        days_from_civil(year + 1, 1, 1)
                    } else {
                        days_from_civil(year, m + 1, 1)
                    };
                    while day >= next {
                        day -= 7;
                    }
                    day
                }
            };
            day * 86_400 + change.time - before as i64
        };
        vec![(at(start, self.std), *dst), (at(end, *dst), self.std)]
    }
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tzdata(names: &[&str]) -> Vec<u8> {
        let mut out = b"tzdata2025a\0".to_vec();
        let index = 24u32;
        let data = index + 52 * names.len() as u32;
        for offset in [index, data, data] {
            out.extend(offset.to_be_bytes());
        }
        for name in names {
            let mut entry = [0u8; 52];
            entry[..name.len()].copy_from_slice(name.as_bytes());
            out.extend(entry);
        }
        out
    }

    #[test]
    fn tzdata_index_lookup() {
        let data = tzdata(&[
            "Africa/Abidjan",
            "Asia/Seoul",
            "America/Argentina/ComodRivadavia",
        ]);
        assert!(has(&data, "Asia/Seoul"));
        assert!(has(&data, "America/Argentina/ComodRivadavia"));
        assert!(!has(&data, "Asia/Seo"));
        assert!(!has(&data, "Mars/Olympus"));
        assert!(!has(b"nottzdata", "Asia/Seoul"));
        assert!(!has(&data[..30], "Asia/Seoul"));
    }

    fn local(y: i64, m: i64, d: i64, h: i64, min: i64) -> i64 {
        days_from_civil(y, m, d) * 86_400 + h * 3600 + min * 60
    }

    /// `ZonedDateTime.of`'s resolution in the image's own zones: a plain
    /// offset, a gap moved forward, an overlap at the earlier offset, and a
    /// date past the explicit transitions, from the POSIX footer.
    #[test]
    fn local_times_resolve_as_java_time_resolves_them() {
        let Some(root) = aim_paths::original_image_with(PATH.trim_start_matches('/')) else {
            return;
        };
        let tzdata = std::fs::read(root.join(PATH.trim_start_matches('/'))).unwrap();
        let rules = |zone: &str| Rules::parse(super::zone(&tzdata, zone).unwrap()).unwrap();
        let seoul = rules("Asia/Seoul");
        let at = local(2024, 6, 1, 12, 0);
        assert_eq!(seoul.local_to_utc(at), at - 9 * 3600);
        let york = rules("America/New_York");
        assert_eq!(
            york.local_to_utc(local(2024, 3, 10, 2, 30)),
            local(2024, 3, 10, 7, 30)
        );
        assert_eq!(
            york.local_to_utc(local(2024, 11, 3, 1, 30)),
            local(2024, 11, 3, 5, 30)
        );
        assert_eq!(
            york.local_to_utc(local(2100, 7, 1, 12, 0)),
            local(2100, 7, 1, 16, 0)
        );
        assert_eq!(
            york.local_to_utc(local(2100, 1, 1, 12, 0)),
            local(2100, 1, 1, 17, 0)
        );
        let utc = rules("UTC");
        assert_eq!(utc.local_to_utc(at), at);
    }

    #[test]
    fn posix_rules_parse_and_give_their_changes() {
        let rule = Posix::parse("EST5EDT,M3.2.0,M11.1.0").unwrap();
        assert_eq!(rule.std, -5 * 3600);
        // 2024: the second Sunday of March at 02:00 EST, the first of
        // November at 02:00 EDT.
        assert_eq!(
            rule.transitions(2024),
            vec![
                (local(2024, 3, 10, 7, 0), -4 * 3600),
                (local(2024, 11, 3, 6, 0), -5 * 3600)
            ]
        );
        let quoted = Posix::parse("<+0330>-3:30").unwrap();
        assert_eq!(quoted.std, 3 * 3600 + 1800);
        assert!(quoted.transitions(2024).is_empty());
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }
}
