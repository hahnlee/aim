//! Parsers for what the guest's tools print: `am start -W`, `dumpsys
//! gfxinfo`, the events log (`logcat -b events -v epoch`), the fixtures'
//! latency lines and host `ps`.

use std::collections::BTreeMap;

/// `am start -W`: the launch state and `TotalTime` (ms), the time to the
/// activity's first frame as ActivityTaskManager measures it.
#[derive(Debug, PartialEq)]
pub struct Launch {
    pub state: String,
    pub total_ms: f64,
}

pub fn am_start(out: &str) -> Option<Launch> {
    let mut state = None;
    let mut total = None;
    for line in out.lines() {
        if let Some(v) = line.strip_prefix("LaunchState: ") {
            state = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("TotalTime: ") {
            total = v.trim().parse().ok();
        }
    }
    if !out.lines().any(|l| l.trim() == "Status: ok") {
        return None;
    }
    Some(Launch {
        state: state.unwrap_or_default(),
        total_ms: total?,
    })
}

/// The HWUI frame statistics of `dumpsys gfxinfo PACKAGE` (its first
/// process): frames, janky share and frame-time percentiles.
#[derive(Debug, Default, PartialEq)]
pub struct Frames {
    pub total: f64,
    pub janky_pct: f64,
    pub p50_ms: f64,
    pub p90_ms: f64,
    pub p99_ms: f64,
}

pub fn gfxinfo(out: &str) -> Option<Frames> {
    let mut f = Frames::default();
    let mut seen = 0;
    let first = |slot: &mut f64, seen: &mut u32, bit: u32, v: Option<f64>| {
        if *seen & bit == 0
            && let Some(v) = v
        {
            *slot = v;
            *seen |= bit;
        }
    };
    for line in out.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("Total frames rendered: ") {
            first(&mut f.total, &mut seen, 1, v.trim().parse().ok());
        } else if let Some(v) = line.strip_prefix("Janky frames: ") {
            // "12 (8.33%)"
            let pct = v
                .split_once('(')
                .and_then(|(_, p)| p.trim_end_matches([')', '%']).parse().ok());
            first(&mut f.janky_pct, &mut seen, 2, pct);
        } else if let Some(v) = line.strip_prefix("50th percentile: ") {
            first(&mut f.p50_ms, &mut seen, 4, ms(v));
        } else if let Some(v) = line.strip_prefix("90th percentile: ") {
            first(&mut f.p90_ms, &mut seen, 8, ms(v));
        } else if let Some(v) = line.strip_prefix("99th percentile: ") {
            first(&mut f.p99_ms, &mut seen, 16, ms(v));
        }
    }
    // No frames: the percentiles are the histogram's empty defaults.
    (seen & 1 != 0 && f.total > 0.0).then_some(f)
}

fn ms(v: &str) -> Option<f64> {
    v.trim().strip_suffix("ms")?.parse().ok()
}

/// The first wall-clock time (seconds since the epoch) of each tag in an
/// events log printed with `-v epoch`:
/// `  1790605163.948 57708 57708 I boot_progress_start: 517638124`.
pub fn events(out: &str) -> BTreeMap<String, f64> {
    let mut first = BTreeMap::new();
    for line in out.lines() {
        let mut fields = line.split_whitespace();
        let (Some(time), Some(_pid), Some(_tid), Some(_level), Some(tag)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let (Ok(time), Some(tag)) = (time.parse::<f64>(), tag.strip_suffix(':')) else {
            continue;
        };
        first.entry(tag.to_string()).or_insert(time);
    }
    first
}

/// Whether the middle of a 32-bit BMP (aim-display's capture), the rows
/// between 20 % and 80 % of its height, is one color: a page area with
/// nothing drawn in it. Not a 32-bit BMP: not blank.
pub fn bmp_middle_blank(bmp: &[u8]) -> bool {
    let field = |at: usize| -> Option<i32> {
        Some(i32::from_le_bytes(bmp.get(at..at + 4)?.try_into().ok()?))
    };
    let (Some(offset), Some(width), Some(height)) = (field(10), field(18), field(22)) else {
        return false;
    };
    if bmp.get(28..30) != Some(&[32, 0][..]) || width <= 0 {
        return false;
    }
    let (offset, width, height) = (
        offset as usize,
        width as usize,
        height.unsigned_abs() as usize,
    );
    let row = width * 4;
    let Some(pixels) = bmp.get(offset + height / 5 * row..offset + height * 4 / 5 * row) else {
        return false;
    };
    let first = &pixels[..4];
    pixels.chunks_exact(4).all(|p| p[..3] == first[..3])
}

/// Chrome's "Child process died" lines (a renderer or GPU process) at or
/// after `since` (seconds since the epoch) in a log printed with `-v epoch`.
pub fn child_deaths(log: &str, since: f64) -> usize {
    log.lines()
        .filter(|l| l.contains("Child process died"))
        .filter_map(|l| l.split_whitespace().next()?.parse::<f64>().ok())
        .filter(|&t| t >= since)
        .count()
}

/// A fixture's latency line: `binder ping: n=2000 p50=41.2 p90=50.0
/// p99=80.1 mean=43.0 us` gives {n, p50, p90, p99, mean}.
pub fn latency(out: &str, prefix: &str) -> Option<BTreeMap<String, f64>> {
    let line = out.lines().find(|l| l.starts_with(prefix))?;
    let values: BTreeMap<String, f64> = line[prefix.len()..]
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .filter_map(|(k, v)| Some((k.to_string(), v.parse().ok()?)))
        .collect();
    values.contains_key("p50").then_some(values)
}

/// The host processes of one boot.
#[derive(Debug, Default, PartialEq)]
pub struct Processes {
    pub count: usize,
    pub rss_kib: u64,
    /// CPU time used so far by the processes alive now.
    pub cpu_s: f64,
}

/// Host processes of one boot from `ps -axww -o pid=,rss=,time=,command=`:
/// every linux-run whose command line names the boot's path map (services,
/// their fork children and exec'd programs all carry it).
pub fn guest_processes(ps: &str, path_map: &str) -> Processes {
    let needle = format!("--path-map {path_map}");
    let mut p = Processes::default();
    for line in ps.lines() {
        let mut fields = line.split_whitespace();
        let (Some(_pid), Some(kib), Some(time), Some(command)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if !command.ends_with("/linux-run") || !line.contains(&needle) {
            continue;
        }
        p.count += 1;
        p.rss_kib += kib.parse::<u64>().unwrap_or(0);
        p.cpu_s += cpu_time(time).unwrap_or(0.0);
    }
    p
}

/// `ps`'s cumulative CPU time: `[[D-]H:]M:SS.ss`.
fn cpu_time(v: &str) -> Option<f64> {
    let (days, rest) = match v.split_once('-') {
        Some((d, rest)) => (d.parse::<f64>().ok()?, rest),
        None => (0.0, v),
    };
    let mut total = 0.0;
    for part in rest.split(':') {
        total = total * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn am_start_cold_and_failed() {
        let out = "Starting: Intent { cmp=com.android.calculator2/.Calculator }\n\
                   Status: ok\nLaunchState: COLD\n\
                   Activity: com.android.calculator2/.Calculator\n\
                   TotalTime: 2229\nWaitTime: 2239\nComplete\n";
        assert_eq!(
            am_start(out),
            Some(Launch {
                state: "COLD".into(),
                total_ms: 2229.0
            })
        );
        let hot = "Warning: Activity not started, its current task has been brought to the front\n\
                   Status: ok\nLaunchState: UNKNOWN (0)\nActivity: a/.B\nWaitTime: 406\nComplete\n";
        assert_eq!(am_start(hot), None);
        let timeout = "Starting: Intent { cmp=a/.B }\nStatus: timeout\nComplete\n";
        assert_eq!(am_start(timeout), None);
    }

    #[test]
    fn gfxinfo_takes_the_first_process() {
        let out = "\
Applications Graphics Acceleration Info:
Uptime: 1000 Realtime: 1000

** Graphics info for pid 4242 [org.chromium.chrome] **

Stats since: 5120000000ns
Total frames rendered: 120
Janky frames: 10 (8.33%)
Janky frames (legacy): 14 (11.67%)
50th percentile: 7ms
90th percentile: 13ms
95th percentile: 17ms
99th percentile: 32ms
Number Missed Vsync: 3

** Graphics info for pid 4300 [org.chromium.chrome:sandboxed] **
Total frames rendered: 5
Janky frames: 5 (100.00%)
50th percentile: 150ms
";
        assert_eq!(
            gfxinfo(out),
            Some(Frames {
                total: 120.0,
                janky_pct: 8.33,
                p50_ms: 7.0,
                p90_ms: 13.0,
                p99_ms: 32.0
            })
        );
        assert_eq!(gfxinfo("No process found for: x\n"), None);
        let empty = "Total frames rendered: 0\nJanky frames: 0 (0.00%)\n50th percentile: 4950ms\n";
        assert_eq!(gfxinfo(empty), None);
    }

    #[test]
    fn events_first_time_of_each_tag() {
        let out = "\
--------- beginning of events
         1790605163.948 57708 57708 I boot_progress_start: 517638124
         1790605166.549 57780 57780 I boot_progress_system_run: 517640724
         1790605186.097 57780 16399 I boot_progress_enable_screen: 517660273
         1790605190.000 57780 16399 I boot_progress_enable_screen: 1
         1790605191.000 57780 16399 I am_proc_start: [0,1,2,com.x,activity,{com.x/.A}]
";
        let e = events(out);
        assert_eq!(e["boot_progress_start"], 1790605163.948);
        assert_eq!(e["boot_progress_system_run"], 1790605166.549);
        assert_eq!(e["boot_progress_enable_screen"], 1790605186.097);
        assert_eq!(e["am_proc_start"], 1790605191.0);
        assert_eq!(e.len(), 4);
    }

    fn bmp(width: i32, height: i32, pixel: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        let mut out = b"BM".to_vec();
        let size = 54 + (width * height * 4) as u32;
        out.extend(size.to_le_bytes());
        out.extend([0; 4]);
        out.extend(54u32.to_le_bytes());
        out.extend(40u32.to_le_bytes());
        out.extend(width.to_le_bytes());
        out.extend((-height).to_le_bytes());
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend([0; 24]);
        for y in 0..height as usize {
            for x in 0..width as usize {
                out.extend(pixel(x, y));
            }
        }
        out
    }

    #[test]
    fn bmp_middle_blank_ignores_the_bars() {
        // Toolbars in the top and bottom tenth, white between them.
        let bars = |_: usize, y: usize| {
            if !(1..9).contains(&y) {
                [9, 9, 9, 255]
            } else {
                [255; 4]
            }
        };
        assert!(bmp_middle_blank(&bmp(4, 10, bars)));
        let text = |x: usize, y: usize| {
            if (x, y) == (2, 5) {
                [0, 0, 0, 255]
            } else {
                bars(x, y)
            }
        };
        assert!(!bmp_middle_blank(&bmp(4, 10, text)));
        assert!(!bmp_middle_blank(b"BM"));
    }

    #[test]
    fn child_deaths_since() {
        let log = "\
--------- beginning of main
  1790608790.504 80985 15449 W chromium: [WARNING:chrome/browser/android/compositor/compositor_view.cc:394] Child process died (type=6) pid=81490)
  1790608793.494 81516 49162 W chromium: [WARNING:content/app/android/content_main_android.cc:82] android_setCpu already initialized
  1790608793.614 80985 15449 W chromium: [WARNING:chrome/browser/android/compositor/compositor_view.cc:394] Child process died (type=6) pid=81516)
";
        assert_eq!(child_deaths(log, 0.0), 2);
        assert_eq!(child_deaths(log, 1790608791.0), 1);
        assert_eq!(child_deaths(log, 1790608800.0), 0);
    }

    #[test]
    fn latency_line() {
        let out = "noise\nbinder ping: n=2000 p50=41.2 p90=50.0 p99=80.1 mean=43.0 us\n";
        let v = latency(out, "binder ping:").unwrap();
        assert_eq!(v["n"], 2000.0);
        assert_eq!(v["p50"], 41.2);
        assert_eq!(v["p99"], 80.1);
        assert_eq!(v["mean"], 43.0);
        assert!(latency(out, "fork:").is_none());
    }

    #[test]
    fn guest_processes_of_one_boot() {
        let ps = "\
  101  2048   0:00.10 /t/release/guest-init --image /i --data /d --run
  102 10240   1:02.50 /t/release/linux-run --root /i --path-map /d/run/path-map --identity x /system/bin/logd
  103 20480   0:00.25 /t/release/linux-run --path-map /d/run/path-map --fork-child 3,4
  104 40960   0:09.00 /t/release/linux-run --root /i --path-map /other/run/path-map /system/bin/logd
  105   512   0:00.00 /bin/zsh -c linux-run --path-map /d/run/path-map
";
        assert_eq!(
            guest_processes(ps, "/d/run/path-map"),
            Processes {
                count: 2,
                rss_kib: 30720,
                cpu_s: 62.75
            }
        );
    }

    #[test]
    fn cpu_time_forms() {
        assert_eq!(cpu_time("0:01.50"), Some(1.5));
        assert_eq!(cpu_time("75:00.00"), Some(4500.0));
        assert_eq!(cpu_time("1:00:00.00"), Some(3600.0));
        assert_eq!(cpu_time("1-00:00:01"), Some(86_401.0));
        assert_eq!(cpu_time("x"), None);
    }
}
