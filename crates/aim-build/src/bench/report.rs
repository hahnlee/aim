//! The bench's results: one flat map of metrics per boot ("run"), their
//! median, min and max over the runs, the stdout table, the JSON file and
//! `--compare`.
//!
//! A metric's name ends in its unit (`_s`, `_ms`, `_us`, `_mb`, `_pct`,
//! `_count`); every metric is lower-is-better except `_count`.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub type Run = BTreeMap<String, f64>;

#[derive(Debug, PartialEq)]
pub struct Stat {
    pub median: f64,
    pub min: f64,
    pub max: f64,
    pub n: usize,
}

pub fn stat(values: &[f64]) -> Option<Stat> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    let median = if v.len() % 2 == 1 {
        v[mid]
    } else {
        (v[mid - 1] + v[mid]) / 2.0
    };
    Some(Stat {
        median,
        min: v[0],
        max: v[v.len() - 1],
        n: v.len(),
    })
}

/// Each metric's statistics over the runs that have it.
pub fn summarize(runs: &[Run]) -> BTreeMap<String, Stat> {
    let mut values: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for run in runs {
        for (k, v) in run {
            values.entry(k).or_default().push(*v);
        }
    }
    values
        .into_iter()
        .filter_map(|(k, v)| Some((k.to_string(), stat(&v)?)))
        .collect()
}

pub fn to_json(meta: Map<String, Value>, runs: &[Run]) -> Value {
    let summary: Map<String, Value> = summarize(runs)
        .into_iter()
        .map(|(k, s)| {
            (
                k,
                json!({"median": s.median, "min": s.min, "max": s.max, "n": s.n}),
            )
        })
        .collect();
    let mut out = meta;
    out.insert("runs".into(), json!(runs));
    out.insert("summary".into(), Value::Object(summary));
    Value::Object(out)
}

/// The medians of a bench JSON file.
pub fn medians(report: &Value) -> Result<BTreeMap<String, f64>, String> {
    let summary = report["summary"]
        .as_object()
        .ok_or("not a bench report (no summary)")?;
    Ok(summary
        .iter()
        .filter_map(|(k, v)| Some((k.clone(), v["median"].as_f64()?)))
        .collect())
}

fn number(v: f64) -> String {
    if v.abs() >= 100.0 || v.fract() == 0.0 {
        format!("{v:.0}")
    } else if v.abs() >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

pub fn table(runs: &[Run]) -> String {
    let summary = summarize(runs);
    let width = summary.keys().map(String::len).max().unwrap_or(6).max(6);
    let mut out = format!(
        "{:<width$}  {:>9}  {:>9}  {:>9}  {:>2}\n",
        "metric", "median", "min", "max", "n"
    );
    for (k, s) in &summary {
        out += &format!(
            "{k:<width$}  {:>9}  {:>9}  {:>9}  {:>2}\n",
            number(s.median),
            number(s.min),
            number(s.max),
            s.n
        );
    }
    out
}

/// A's and B's medians side by side, with B - A and its share of A.
pub fn compare(a: &BTreeMap<String, f64>, b: &BTreeMap<String, f64>) -> String {
    let keys: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    let width = keys.iter().map(|k| k.len()).max().unwrap_or(6).max(6);
    let mut out = format!(
        "{:<width$}  {:>9}  {:>9}  {:>9}  {:>8}\n",
        "metric", "A", "B", "B-A", "change"
    );
    let cell = |v: Option<&f64>| v.map_or("-".to_string(), |v| number(*v));
    for k in keys {
        let (va, vb) = (a.get(k), b.get(k));
        let (delta, pct) = match (va, vb) {
            (Some(x), Some(y)) => (
                number(y - x),
                if *x != 0.0 {
                    format!("{:+.1}%", (y - x) / x * 100.0)
                } else {
                    "-".into()
                },
            ),
            _ => ("-".into(), "-".into()),
        };
        out += &format!(
            "{k:<width$}  {:>9}  {:>9}  {:>9}  {:>8}\n",
            cell(va),
            cell(vb),
            delta,
            pct
        );
    }
    out
}

/// `YYYYMMDD-HHMMSS` (UTC) of a Unix time, for the report's file name.
pub fn timestamp(unix: u64) -> String {
    let (days, secs) = (unix / 86_400, unix % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(pairs: &[(&str, f64)]) -> Run {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn stat_odd_even_and_empty() {
        assert_eq!(
            stat(&[3.0, 1.0, 2.0]),
            Some(Stat {
                median: 2.0,
                min: 1.0,
                max: 3.0,
                n: 3
            })
        );
        assert_eq!(stat(&[4.0, 1.0]).unwrap().median, 2.5);
        assert_eq!(stat(&[]), None);
    }

    #[test]
    fn json_round_trip_through_medians() {
        let runs = [
            run(&[("boot.completed_s", 25.0), ("binder.ping_p50_us", 40.0)]),
            run(&[("boot.completed_s", 27.0)]),
            run(&[("boot.completed_s", 26.0), ("binder.ping_p50_us", 50.0)]),
        ];
        let mut meta = Map::new();
        meta.insert("sha".into(), json!("abc"));
        let report = to_json(meta, &runs);
        let text = serde_json::to_string_pretty(&report).unwrap();
        let back: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back["sha"], "abc");
        assert_eq!(back["runs"].as_array().unwrap().len(), 3);
        assert_eq!(back["summary"]["boot.completed_s"]["min"], 25.0);
        assert_eq!(back["summary"]["binder.ping_p50_us"]["n"], 2);
        let m = medians(&back).unwrap();
        assert_eq!(m["boot.completed_s"], 26.0);
        assert_eq!(m["binder.ping_p50_us"], 45.0);
        assert!(medians(&json!({"runs": []})).is_err());
    }

    #[test]
    fn compare_prints_deltas_and_missing() {
        let a = run(&[("boot.completed_s", 25.0), ("gone_ms", 1.0)]);
        let b = run(&[("boot.completed_s", 20.0), ("new_ms", 2.0)]);
        let out = compare(&a, &b);
        let line = out.lines().find(|l| l.starts_with("boot.")).unwrap();
        assert_eq!(
            line.split_whitespace().collect::<Vec<_>>(),
            ["boot.completed_s", "25", "20", "-5", "-20.0%"]
        );
        assert!(
            out.lines()
                .any(|l| l.starts_with("gone_ms") && l.contains(" - "))
        );
        assert!(out.lines().any(|l| l.starts_with("new_ms")));
    }

    #[test]
    fn table_has_one_row_per_metric() {
        let out = table(&[run(&[("a_ms", 1.5), ("b_s", 120.0)])]);
        assert_eq!(out.lines().count(), 3);
        assert!(out.contains("1.50"));
    }

    #[test]
    fn timestamp_is_utc_civil_time() {
        assert_eq!(timestamp(0), "19700101-000000");
        // 2026-09-28 14:19:23 UTC
        assert_eq!(timestamp(1_790_605_163), "20260928-141923");
        assert_eq!(timestamp(951_782_400), "20000229-000000");
    }
}
