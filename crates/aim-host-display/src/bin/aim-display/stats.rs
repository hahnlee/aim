//! Vsync accuracy and present cost, summarized periodically.

use std::sync::Mutex;
use std::time::Instant;

use crate::metal::Frame;
use crate::vsync::Tick;

#[derive(Default)]
struct Series {
    n: u64,
    sum: f64,
    sum_sq: f64,
    max: f64,
}

impl Series {
    fn add(&mut self, v: f64) {
        self.n += 1;
        self.sum += v;
        self.sum_sq += v * v;
        self.max = self.max.max(v);
    }

    fn mean(&self) -> f64 {
        self.sum / self.n.max(1) as f64
    }

    fn std_dev(&self) -> f64 {
        let m = self.mean();
        (self.sum_sq / self.n.max(1) as f64 - m * m).max(0.0).sqrt()
    }
}

#[derive(Default)]
struct Window {
    last_vsync: Option<i64>,
    /// Between consecutive vsync timestamps, in µs.
    interval: Series,
    period: i64,
    /// From a vsync to its callback, in µs.
    latency: Series,
    /// From starting the display link to its first callback, in µs.
    start: Series,
    present_cpu: Series,
    present_gpu: Series,
}

pub struct Stats {
    w: Mutex<Window>,
    since: Mutex<Instant>,
}

impl Stats {
    pub fn new() -> Stats {
        Stats {
            w: Mutex::new(Window::default()),
            since: Mutex::new(Instant::now()),
        }
    }

    pub fn vsync(&self, t: Tick) {
        let mut w = self.w.lock().unwrap();
        let last = w.last_vsync.replace(t.timestamp_ns);
        match (t.started_ns, last) {
            (Some(started), _) => w.start.add((t.now_ns - started) as f64 / 1e3),
            (None, Some(last)) => w.interval.add((t.timestamp_ns - last) as f64 / 1e3),
            (None, None) => {}
        }
        w.period = t.period_ns;
        w.latency.add((t.now_ns - t.timestamp_ns) as f64 / 1e3);
    }

    pub fn present(&self, f: Frame) {
        let mut w = self.w.lock().unwrap();
        w.present_cpu.add(f.cpu_ns as f64 / 1e3);
        w.present_gpu.add(f.gpu_ns as f64 / 1e3);
    }

    /// The summary since the last call, if anything happened.
    pub fn take(&self) -> Option<String> {
        let mut w = self.w.lock().unwrap();
        let mut since = self.since.lock().unwrap();
        let secs = since.elapsed().as_secs_f64();
        *since = Instant::now();
        if w.latency.n == 0 && w.present_cpu.n == 0 {
            return None;
        }
        let mut parts = Vec::new();
        if w.latency.n > 0 {
            parts.push(format!(
                "vsync {} in {secs:.1}s, period {:.1} us (interval mean {:.1} sd {:.1} max {:.1} us), callback {:.0} us after vsync (max {:.0})",
                w.latency.n,
                w.period as f64 / 1e3,
                w.interval.mean(),
                w.interval.std_dev(),
                w.interval.max,
                w.latency.mean(),
                w.latency.max,
            ));
        }
        if w.start.n > 0 {
            parts.push(format!(
                "link started {} times, first vsync {:.0} us after (max {:.0})",
                w.start.n,
                w.start.mean(),
                w.start.max,
            ));
        }
        if w.present_cpu.n > 0 {
            parts.push(format!(
                "present {} ({:.1}/s), {:.0} us to GPU done (max {:.0}), GPU {:.0} us",
                w.present_cpu.n,
                w.present_cpu.n as f64 / secs,
                w.present_cpu.mean(),
                w.present_cpu.max,
                w.present_gpu.mean(),
            ));
        }
        let last = w.last_vsync;
        *w = Window {
            last_vsync: last,
            ..Default::default()
        };
        Some(parts.join("; "))
    }
}
