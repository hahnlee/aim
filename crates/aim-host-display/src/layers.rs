//! Per-task composition in window mode (`docs/layers.md`): which window
//! shows each layer of a frame, and the geometry it is drawn with.
//!
//! The composer HAL sends the layers SurfaceFlinger gave it, bottom first
//! ([`Layer`]). WindowManager crops every task to its bounds, so each of a
//! task's layers lies within them: [`Attribution`] gives a layer to the
//! task whose bounds contain it, settles layers within several tasks by
//! their neighbours in z, and leaves the rest to the desktop (not shown)
//! or to system panels.

use std::collections::HashMap;
use std::io;
use std::os::fd::BorrowedFd;

use aim_hostcall::display::{Layer, owner};

/// Left, top, right, bottom, in display pixels.
pub type Rect = [i32; 4];

/// How long a layer within no task waits for one to take it (a new task's
/// first frames come before its bounds) before a system panel shows it.
pub const GRACE_MS: i64 = 200;

/// The most layers and rectangles a frame may have.
const MAX_LAYERS: u32 = 4096;
const MAX_RECTS: u32 = 1 << 16;

/// A task with a window: its id and bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Task {
    pub id: i32,
    pub bounds: Rect,
}

/// What a layer was given before, and since when it has been seen.
#[derive(Clone, Copy, Debug)]
struct Seen {
    owner: u32,
    task: i32,
    since_ms: i64,
}

/// The owners of the layers of successive frames (one composer client).
#[derive(Default)]
pub struct Attribution {
    seen: HashMap<u64, Seen>,
}

fn contains(b: Rect, f: Rect) -> bool {
    f[0] >= b[0] && f[1] >= b[1] && f[2] <= b[2] && f[3] <= b[3]
}

fn is_empty(r: Rect) -> bool {
    r[2] <= r[0] || r[3] <= r[1]
}

impl Attribution {
    /// Set each layer's owner (and task). `tasks` are the tasks with
    /// windows, `display` the display's rectangle, `front` the focused
    /// task, `now_ms` a monotonic time. Returns when (in `now_ms`'s
    /// clock) a layer waiting for a task stops waiting, if one does: the
    /// frame should be attributed again then.
    pub fn attribute(
        &mut self,
        layers: &mut [Layer],
        tasks: &[Task],
        display: Rect,
        front: Option<i32>,
        now_ms: i64,
    ) -> Option<i64> {
        let n = layers.len();
        let within = |f: Rect| -> Vec<i32> {
            tasks
                .iter()
                .filter(|t| contains(t.bounds, f))
                .map(|t| t.id)
                .collect()
        };
        let candidates: Vec<Vec<i32>> = layers.iter().map(|l| within(l.frame)).collect();
        let mut owners: Vec<Option<(u32, i32)>> = vec![None; n];
        // The desktop: the layers at the bottom that cover the display.
        let mut first = 0;
        while first < n && contains(layers[first].frame, display) && candidates[first].is_empty() {
            owners[first] = Some((owner::NONE, 0));
            first += 1;
        }
        for i in first..n {
            let f = layers[i].frame;
            let c = &candidates[i];
            let exact: Vec<i32> = tasks
                .iter()
                .filter(|t| t.bounds == f)
                .map(|t| t.id)
                .collect();
            owners[i] = if is_empty(f) {
                Some((owner::NONE, 0))
            } else if exact.len() == 1 {
                Some((owner::TASK, exact[0]))
            } else if c.len() == 1 {
                Some((owner::TASK, c[0]))
            } else {
                None
            };
        }
        let task_of = |o: Option<(u32, i32)>| match o {
            Some((owner::TASK, t)) => Some(t),
            _ => None,
        };
        let resolved = owners.clone();
        let mut deadline: Option<i64> = None;
        for i in first..n {
            if owners[i].is_some() {
                continue;
            }
            let c = &candidates[i];
            let previous = self.seen.get(&layers[i].id).copied();
            owners[i] = Some(if !c.is_empty() {
                // Within several tasks: the nearest neighbour's above, else
                // below, else what it was, else the front one.
                let near = |j: usize| task_of(resolved[j]).filter(|t| c.contains(t));
                let t = (i + 1..n)
                    .find_map(near)
                    .or_else(|| (first..i).rev().find_map(near))
                    .or_else(|| {
                        previous
                            .filter(|p| p.owner == owner::TASK && c.contains(&p.task))
                            .map(|p| p.task)
                    })
                    .or_else(|| front.filter(|f| c.contains(f)))
                    .unwrap_or(c[0]);
                (owner::TASK, t)
            } else {
                match previous {
                    // Moved with its task before the new bounds came.
                    Some(p) if p.owner == owner::TASK && tasks.iter().any(|t| t.id == p.task) => {
                        (owner::TASK, p.task)
                    }
                    // Its task is gone: it closes with it.
                    Some(p) if p.owner == owner::TASK => (owner::NONE, 0),
                    Some(p) if p.owner == owner::SYSTEM => (owner::SYSTEM, 0),
                    _ => {
                        let since = previous.map_or(now_ms, |p| p.since_ms);
                        let ends = since + GRACE_MS;
                        if now_ms >= ends {
                            (owner::SYSTEM, 0)
                        } else {
                            deadline = Some(deadline.map_or(ends, |d| d.min(ends)));
                            (owner::NONE, 0)
                        }
                    }
                }
            });
        }
        let mut seen = HashMap::with_capacity(n);
        for (l, o) in layers.iter_mut().zip(owners) {
            let (owner, task) = o.unwrap_or((owner::NONE, 0));
            l.owner = owner;
            l.task = task;
            let since_ms = self.seen.get(&l.id).map_or(now_ms, |p| p.since_ms);
            seen.insert(
                l.id,
                Seen {
                    owner,
                    task,
                    since_ms,
                },
            );
        }
        self.seen = seen;
        deadline
    }

    /// Forget every layer (a new composer client).
    pub fn reset(&mut self) {
        self.seen.clear();
    }
}

/// The texture coordinates (0 to 1, the top row first) shown at a layer's
/// top left, top right, bottom left and bottom right corners: its source
/// `crop` of a `size` buffer under `transform` (flip horizontally 1,
/// vertically 2, then turn 90 degrees clockwise 4, as the composer's
/// `Transform`).
pub fn corners(crop: [f32; 4], transform: u32, size: (u32, u32)) -> [[f32; 2]; 4] {
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|(x, y)| {
        // Undo the turn, then the flips: a display point's place in the
        // crop.
        let (mut u, mut v) = if transform & 4 != 0 {
            (y, 1.0 - x)
        } else {
            (x, y)
        };
        if transform & 1 != 0 {
            u = 1.0 - u;
        }
        if transform & 2 != 0 {
            v = 1.0 - v;
        }
        [
            (crop[0] + u * (crop[2] - crop[0])) / w,
            (crop[1] + v * (crop[3] - crop[1])) / h,
        ]
    })
}

/// Rectangles that cover the union of `rects` once each, in bands from the
/// top.
pub fn disjoint(rects: &[Rect]) -> Vec<Rect> {
    let rects: Vec<Rect> = rects.iter().copied().filter(|&r| !is_empty(r)).collect();
    let mut ys: Vec<i32> = rects.iter().flat_map(|r| [r[1], r[3]]).collect();
    ys.sort_unstable();
    ys.dedup();
    let mut out = Vec::new();
    for band in ys.windows(2) {
        let (top, bottom) = (band[0], band[1]);
        let mut spans: Vec<(i32, i32)> = rects
            .iter()
            .filter(|r| r[1] <= top && r[3] >= bottom)
            .map(|r| (r[0], r[2]))
            .collect();
        spans.sort_unstable();
        let mut current: Option<(i32, i32)> = None;
        for (l, r) in spans {
            current = match current {
                Some((cl, cr)) if l <= cr => Some((cl, cr.max(r))),
                Some((cl, cr)) => {
                    out.push([cl, top, cr, bottom]);
                    Some((l, r))
                }
                None => Some((l, r)),
            };
        }
        if let Some((l, r)) = current {
            out.push([l, top, r, bottom]);
        }
    }
    out
}

/// The bounding boxes of the groups of overlapping `rects`; boxes that
/// would overlap are one group.
pub fn groups(rects: &[Rect]) -> Vec<Rect> {
    let overlap = |a: Rect, b: Rect| a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
    let mut boxes: Vec<Rect> = rects.iter().copied().filter(|&r| !is_empty(r)).collect();
    loop {
        let mut merged = false;
        let mut i = 0;
        while i < boxes.len() {
            let mut j = i + 1;
            while j < boxes.len() {
                if overlap(boxes[i], boxes[j]) {
                    let b = boxes.swap_remove(j);
                    let a = &mut boxes[i];
                    *a = [
                        a[0].min(b[0]),
                        a[1].min(b[1]),
                        a[2].max(b[2]),
                        a[3].max(b[3]),
                    ];
                    merged = true;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
        if !merged {
            return boxes;
        }
    }
}

/// A frame's layers and their visible regions' rectangles as bytes: the
/// two counts, then the arrays.
pub fn encode(layers: &[Layer], rects: &[Rect]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + size_of_val(layers) + size_of_val(rects));
    out.extend_from_slice(&(layers.len() as u32).to_ne_bytes());
    out.extend_from_slice(&(rects.len() as u32).to_ne_bytes());
    // SAFETY: plain-old-data arrays viewed as bytes.
    unsafe {
        out.extend_from_slice(std::slice::from_raw_parts(
            layers.as_ptr().cast(),
            size_of_val(layers),
        ));
        out.extend_from_slice(std::slice::from_raw_parts(
            rects.as_ptr().cast(),
            size_of_val(rects),
        ));
    }
    out
}

/// Read what [`encode`] wrote from `sock`.
pub fn read(sock: BorrowedFd) -> io::Result<(Vec<Layer>, Vec<Rect>)> {
    let mut counts = [0u8; 8];
    if !crate::wire::recv(sock, &mut counts, &mut Vec::new())? {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    let count = u32::from_ne_bytes(counts[..4].try_into().unwrap());
    let rect_count = u32::from_ne_bytes(counts[4..].try_into().unwrap());
    if count > MAX_LAYERS || rect_count > MAX_RECTS {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut layers = vec![Layer::default(); count as usize];
    let mut rects = vec![[0i32; 4]; rect_count as usize];
    // SAFETY: plain-old-data arrays; every byte pattern is a value.
    let (lb, rb) = unsafe {
        (
            std::slice::from_raw_parts_mut(layers.as_mut_ptr().cast(), size_of_val(&layers[..])),
            std::slice::from_raw_parts_mut(rects.as_mut_ptr().cast(), size_of_val(&rects[..])),
        )
    };
    for buf in [lb, rb] {
        if !buf.is_empty() && !crate::wire::recv(sock, buf, &mut Vec::new())? {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
    }
    Ok((layers, rects))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_hostcall::display::layer;

    const DISPLAY: Rect = [0, 0, 3000, 2000];

    fn at(id: u64, frame: Rect) -> Layer {
        Layer {
            id,
            kind: layer::BUFFER,
            frame,
            alpha: 1.0,
            ..Default::default()
        }
    }

    fn owners(layers: &[Layer]) -> Vec<(u32, i32)> {
        layers.iter().map(|l| (l.owner, l.task)).collect()
    }

    const A: Task = Task {
        id: 9,
        bounds: [100, 100, 1100, 1100],
    };
    const B: Task = Task {
        id: 11,
        bounds: [600, 300, 1600, 1300],
    };

    #[test]
    fn overlapping_tasks_keep_their_layers() {
        // The home, A's window and a popup of A in the overlap, B's
        // SurfaceView in the overlap below B's window, B's window.
        let mut layers = [
            at(1, DISPLAY),
            at(2, A.bounds),
            at(3, [700, 400, 900, 600]),
            at(4, [650, 350, 1000, 700]),
            at(5, B.bounds),
        ];
        Attribution::default().attribute(&mut layers, &[A, B], DISPLAY, Some(11), 0);
        assert_eq!(
            owners(&layers),
            [
                (owner::NONE, 0),
                (owner::TASK, 9),
                // Between A's and B's within both: B's, which on screen
                // shows it where Android does.
                (owner::TASK, 11),
                (owner::TASK, 11),
                (owner::TASK, 11),
            ]
        );
    }

    #[test]
    fn a_task_inside_another_keeps_its_layers() {
        let inner = Task {
            id: 12,
            bounds: [300, 300, 700, 900],
        };
        // A's window, the inner task's window and its menu, then a popup
        // of A over both when A is on top.
        let mut layers = [
            at(1, A.bounds),
            at(2, inner.bounds),
            at(3, [400, 400, 600, 500]),
        ];
        Attribution::default().attribute(&mut layers, &[A, inner], DISPLAY, None, 0);
        assert_eq!(
            owners(&layers),
            [(owner::TASK, 9), (owner::TASK, 12), (owner::TASK, 12)]
        );
        let mut layers = [
            at(2, inner.bounds),
            at(1, A.bounds),
            at(4, [400, 400, 600, 500]),
        ];
        Attribution::default().attribute(&mut layers, &[A, inner], DISPLAY, None, 0);
        assert_eq!(
            owners(&layers),
            [(owner::TASK, 12), (owner::TASK, 9), (owner::TASK, 9)]
        );
    }

    #[test]
    fn layers_outside_every_task() {
        let mut a = Attribution::default();
        let toast = [1200, 1700, 1800, 1800];
        let mut layers = [at(1, A.bounds), at(2, toast)];
        // A new one waits for a task first.
        assert_eq!(
            a.attribute(&mut layers, &[A], DISPLAY, None, 1000),
            Some(1200)
        );
        assert_eq!(owners(&layers)[1], (owner::NONE, 0));
        assert_eq!(
            a.attribute(&mut layers, &[A], DISPLAY, None, 1100),
            Some(1200)
        );
        assert_eq!(a.attribute(&mut layers, &[A], DISPLAY, None, 1200), None);
        assert_eq!(owners(&layers)[1], (owner::SYSTEM, 0));
        // A task's layer that moved before its new bounds came keeps its
        // task; once the task is gone it shows nowhere.
        layers[0].frame = [150, 100, 1150, 1100];
        a.attribute(&mut layers, &[A], DISPLAY, None, 1300);
        assert_eq!(owners(&layers)[0], (owner::TASK, 9));
        a.attribute(&mut layers, &[], DISPLAY, None, 1400);
        assert_eq!(owners(&layers)[0], (owner::NONE, 0));
        // A new task's window takes its task once the bounds come.
        let mut layers = [at(7, B.bounds)];
        a.attribute(&mut layers, &[A], DISPLAY, None, 2000);
        assert_eq!(owners(&layers)[0], (owner::NONE, 0));
        a.attribute(&mut layers, &[A, B], DISPLAY, None, 2100);
        assert_eq!(owners(&layers)[0], (owner::TASK, 11));
    }

    #[test]
    fn corners_follow_the_transform() {
        let crop = [0.0, 0.0, 100.0, 50.0];
        let size = (100, 50);
        assert_eq!(
            corners(crop, 0, size),
            [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]
        );
        // Flipped horizontally: the top left shows the top right.
        assert_eq!(corners(crop, 1, size)[0], [1.0, 0.0]);
        // Turned 90 degrees clockwise: the top left shows the bottom left,
        // the top right the top left.
        let r = corners(crop, 4, size);
        assert_eq!((r[0], r[1], r[3]), ([0.0, 1.0], [0.0, 0.0], [1.0, 0.0]));
        // 180 degrees and 270 degrees.
        assert_eq!(corners(crop, 3, size)[0], [1.0, 1.0]);
        assert_eq!(corners(crop, 7, size)[0], [1.0, 0.0]);
        // A crop is a part of the buffer.
        assert_eq!(corners([50.0, 25.0, 100.0, 50.0], 0, size)[0], [0.5, 0.5]);
    }

    #[test]
    fn disjoint_covers_once() {
        assert_eq!(
            disjoint(&[[0, 0, 10, 10], [5, 5, 15, 15]]),
            [[0, 0, 10, 5], [0, 5, 15, 10], [5, 10, 15, 15]]
        );
        assert_eq!(disjoint(&[[0, 0, 4, 4], [4, 0, 8, 4]]), [[0, 0, 8, 4]]);
        assert_eq!(
            disjoint(&[[0, 0, 2, 2], [5, 0, 7, 2]]),
            [[0, 0, 2, 2], [5, 0, 7, 2]]
        );
        assert!(disjoint(&[[3, 3, 3, 9]]).is_empty());
    }

    #[test]
    fn groups_merge_overlaps() {
        let g = groups(&[[0, 0, 10, 10], [5, 5, 20, 20], [100, 100, 110, 110]]);
        assert_eq!(g, [[0, 0, 20, 20], [100, 100, 110, 110]]);
        // Two boxes that come to overlap once merged are one group.
        let g = groups(&[[0, 0, 10, 10], [20, 0, 30, 10], [5, 5, 25, 6]]);
        assert_eq!(g, [[0, 0, 30, 10]]);
    }

    #[test]
    fn frames_round_trip() {
        use std::os::fd::AsFd;
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let layers = [at(3, A.bounds), at(4, B.bounds)];
        let rects = [[1, 2, 3, 4]];
        crate::wire::send(a.as_fd(), &encode(&layers, &rects), None).unwrap();
        let (l, r) = read(b.as_fd()).unwrap();
        assert_eq!((&l[..], &r[..]), (&layers[..], &rects[..]));
    }
}
