//! PackageUsage and PackageStateUnserialized, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use std::{collections::BTreeMap, path::Path};

pub const REASONS: usize = 8;
const HEADER: &[u8] = b"PACKAGE_USAGE__VERSION_1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usage {
    historical: bool,
    times: BTreeMap<String, [i64; REASONS]>,
}

impl Usage {
    pub fn new<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            historical: true,
            times: names
                .into_iter()
                .map(|name| (name.into(), [0; REASONS]))
                .collect(),
        }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.times.keys().map(String::as_str)
    }

    pub fn historical_available(&self) -> bool {
        self.historical
    }
    pub fn times(&self, name: &str) -> Option<&[i64; REASONS]> {
        self.times.get(name)
    }

    /// The original ignores an invalid reason or an unknown package.
    pub fn notify(&mut self, name: &str, reason: i32, time: i64) {
        if let Some(times) = self.times.get_mut(name) {
            if let Some(slot) = usize::try_from(reason).ok().and_then(|r| times.get_mut(r)) {
                *slot = time;
            }
        }
    }

    pub fn latest(&self, name: &str) -> Option<i64> {
        self.times(name)
            .map(|times| times.iter().copied().fold(0, i64::max))
    }

    pub fn latest_foreground(&self, name: &str) -> Option<i64> {
        // NOTIFY_PACKAGE_USE_ACTIVITY and NOTIFY_PACKAGE_USE_FOREGROUND_SERVICE.
        self.times(name).map(|times| 0.max(times[0]).max(times[2]))
    }

    /// AtomicFile.openRead prefers its legacy backup. A missing file alone
    /// marks historical usage unavailable; parse failures keep the applied prefix.
    pub fn read(&mut self, path: &Path) -> Result<(), String> {
        let backup = super::super::sibling(path, ".bak");
        let path = if backup.exists() {
            backup.as_path()
        } else {
            path
        };
        match super::super::bytes(path)? {
            None => {
                self.historical = false;
                Ok(())
            }
            Some(bytes) => self
                .apply(&bytes)
                .map_err(|error| format!("{}: {error}", path.display())),
        }
    }

    pub fn apply(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut rows = bytes.split_inclusive(|b| *b == b'\n');
        let Some(first) = rows.next() else {
            return Ok(());
        };
        let v1 = line(first)? == HEADER;
        let rows = (!v1).then_some(first).into_iter().chain(rows);
        for row in rows {
            let row = line(row)?;
            // Java String.split(" ") retains internal empty fields and drops trailing ones.
            let mut tokens: Vec<_> = row.split(|b| *b == b' ').collect();
            while tokens.len() > 1 && tokens.last() == Some(&b"".as_slice()) {
                tokens.pop();
            }
            if tokens.len() != if v1 { REASONS + 1 } else { 2 } {
                return Err("invalid package usage row width".into());
            }
            let name: String = tokens[0].iter().map(|b| char::from(*b)).collect();
            let Some(times) = self.times.get_mut(&name) else {
                continue;
            };
            if v1 {
                for (slot, token) in times.iter_mut().zip(&tokens[1..]) {
                    *slot = number(token)?;
                }
            } else {
                times.fill(number(tokens[1])?);
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = String::from("PACKAGE_USAGE__VERSION_1\n");
        for (name, times) in &self.times {
            if times.iter().copied().fold(0, i64::max) == 0 {
                continue;
            }
            out.push_str(name);
            for time in times {
                out.push(' ');
                out.push_str(&time.to_string());
            }
            out.push('\n');
        }
        out.into_bytes()
    }
}

fn line(row: &[u8]) -> Result<&[u8], String> {
    row.strip_suffix(b"\n")
        .ok_or_else(|| "unexpected EOF in package usage".into())
}

fn number(token: &[u8]) -> Result<i64, String> {
    // Long.parseLong accepts signs, but no whitespace or ASCII separators.
    let token = std::str::from_utf8(token).map_err(|_| "invalid usage timestamp")?;
    if token.is_empty()
        || !token
            .trim_start_matches(['+', '-'])
            .bytes()
            .all(|b| b.is_ascii_digit())
    {
        return Err("invalid usage timestamp".into());
    }
    token.parse().map_err(|_| "invalid usage timestamp".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_file_and_legacy_atomic_backup_preserve_original_availability() {
        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let directory =
            Directory(std::env::temp_dir().join(format!("aim-usage-{}", std::process::id())));
        std::fs::create_dir(&directory.0).unwrap();
        let path = directory.0.join("package-usage.list");
        let mut missing = Usage::new(["a"]);
        missing.read(&path).unwrap();
        assert!(!missing.historical_available());
        assert_eq!(missing.times("a"), Some(&[0; REASONS]));
        std::fs::write(&path, b"a 11\n").unwrap();
        std::fs::write(super::super::super::sibling(&path, ".bak"), b"a 22\n").unwrap();
        let mut backup = Usage::new(["a"]);
        backup.read(&path).unwrap();
        assert_eq!(backup.times("a"), Some(&[22; REASONS]));
        assert!(backup.historical_available());
        // Reading never changes either original input file.
        assert_eq!(std::fs::read(&path).unwrap(), b"a 11\n");
        std::fs::write(super::super::super::sibling(&path, ".bak"), b"a 33\nbroken").unwrap();
        assert!(backup.read(&path).is_err());
        assert_eq!(backup.times("a"), Some(&[33; REASONS]));
        assert!(backup.historical_available());
    }

    #[test]
    fn versions_prefix_failures_and_unknown_packages_follow_original() {
        let mut usage = Usage::new(["a", "b"]);
        usage.apply(b"a +7\nb -4\nunknown invalid\n").unwrap();
        assert_eq!(usage.times("a"), Some(&[7; REASONS]));
        assert_eq!(usage.latest("b"), Some(0));
        usage.apply(b"PACKAGE_USAGE__VERSION_1\na 1 2 3 4 5 6 7 8   \nunknown bad bad bad bad bad bad bad bad\n").unwrap();
        assert_eq!(usage.latest("a"), Some(8));
        assert_eq!(usage.latest_foreground("a"), Some(3));
        assert!(
            usage
                .apply(b"PACKAGE_USAGE__VERSION_1\na 9 10 invalid 4 5 6 7 8\n")
                .is_err()
        );
        assert_eq!(usage.times("a"), Some(&[9, 10, 3, 4, 5, 6, 7, 8]));
        let before = usage.clone();
        assert!(usage.apply(b"a 12").is_err());
        assert_eq!(usage, before);
        assert!(usage.apply(b"a  12\n").is_err());
        assert!(usage.apply(b"a 9223372036854775808\n").is_err());
        assert!(usage.apply(b"a 12\r\n").is_err());
        usage.apply(b"").unwrap();
        usage.notify("a", -1, 33);
        usage.notify("a", 8, 33);
        usage.notify("absent", 0, 33);
        assert_eq!(usage, before);
        usage.notify("a", 2, 44);
        let encoded = usage.encode();
        let mut read = Usage::new(["a", "b"]);
        read.apply(&encoded).unwrap();
        assert_eq!(read.times("a"), usage.times("a"));
        assert_eq!(read.times("b"), Some(&[0; REASONS]));
        assert!(usage.historical_available());
    }
}
