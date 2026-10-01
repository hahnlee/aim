//! Where each launcher activity's window last was (#635), as a desktop
//! remembers an app's window: a new task of it opens there.
//!
//! The platform records a task's bounds per component only on a freeform
//! display area (`LaunchParamsPersister`), and desktop windowing keeps the
//! display area fullscreen. The bridge keeps them itself, one line per
//! activity, `left top right bottom package/class`, in display pixels.

use std::collections::BTreeMap;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Places(BTreeMap<String, [i32; 4]>);

impl Places {
    /// The places in `text`; a line that is not one is skipped.
    pub fn parse(text: &str) -> Places {
        let place = |line: &str| {
            let mut fields = line.splitn(5, ' ');
            let mut bounds = [0; 4];
            for b in &mut bounds {
                *b = fields.next()?.parse().ok()?;
            }
            let activity = fields.next().filter(|a| a.contains('/'))?;
            Some((activity.to_string(), bounds))
        };
        Places(text.lines().filter_map(place).collect())
    }

    pub fn text(&self) -> String {
        let line = |(a, [l, t, r, b]): (&String, &[i32; 4])| format!("{l} {t} {r} {b} {a}\n");
        self.0.iter().map(line).collect()
    }

    pub fn get(&self, activity: &str) -> Option<[i32; 4]> {
        self.0.get(activity).copied()
    }

    /// Remember `bounds` for `activity`; whether that changed anything.
    pub fn set(&mut self, activity: &str, bounds: [i32; 4]) -> bool {
        self.0.insert(activity.to_string(), bounds) != Some(bounds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut p = Places::default();
        assert!(p.set("com.android.settings/.Settings", [10, 20, 834, 1484]));
        assert!(p.set("a.b/c.D Name", [-5, 0, 100, 200]));
        assert!(!p.set("a.b/c.D Name", [-5, 0, 100, 200]));
        let text = p.text();
        assert_eq!(
            text,
            "-5 0 100 200 a.b/c.D Name\n10 20 834 1484 com.android.settings/.Settings\n"
        );
        assert_eq!(Places::parse(&text), p);
    }

    #[test]
    fn skips_bad_lines() {
        let p = Places::parse("1 2 3 4 a/b\n1 2 x 4 c/d\n1 2 3 e/f\n1 2 3 4 nopackage\n\n");
        assert_eq!(p.get("a/b"), Some([1, 2, 3, 4]));
        assert_eq!(p.text(), "1 2 3 4 a/b\n");
    }
}
