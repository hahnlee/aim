//! Window mode for the app host (docs/windows.md): each activity's task
//! is a Mac window. The app host is the display server's task bridge,
//! which `aim-windows` is for the original's tasks: on the connection
//! `FN_WINDOWS` makes (`aim_hostcall::display::window`) it reports its
//! tasks and takes the server's requests. A task opens at freeform's
//! default bounds (`freeform`); its layer is a container at its bounds,
//! the parent of its windows' layers, as a task's surface is the parent
//! of its windows' in WindowManager.

use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use aim_host_display::wire;
use aim_hostcall::display::{Window as Record, Windows, mode, window};

use super::AppHost;
use super::config::Rect;
use super::surface::{self, Change, Layer};

/// A task: one activity's, in its own window.
pub struct Task {
    pub id: i32,
    /// The start of its app's process.
    pub start_seq: i64,
    pub uid: u32,
    pub package: String,
    /// Its activity, `package/class`.
    pub component: String,
    pub bounds: Rect,
    /// Its root activity's window layout's minimum size, -1 for none.
    min_size: (i32, i32),
    /// Its layer, made with its first window's.
    layer: Option<Layer>,
    /// The server's `LAUNCH` it answers with `DRAWN` once it is idle.
    launch: Option<String>,
}

/// The tasks, back to front, and the server's connection.
#[derive(Default)]
pub struct Tasks {
    /// Whether the display server shows a window per task, once it said.
    windows: OnceLock<bool>,
    bridge: Mutex<Option<UnixStream>>,
    tasks: Mutex<Vec<Task>>,
    next: Mutex<i32>,
}

impl Tasks {
    /// Whether the display server shows a window per task.
    pub fn window_mode(&self) -> bool {
        self.windows.get().copied().unwrap_or(false)
    }

    /// Sends `record` to the server.
    fn send(&self, record: &Record) {
        let mut bridge = self.bridge.lock().unwrap();
        if let Some(stream) = bridge.as_mut()
            && let Err(e) = stream.write_all(wire::bytes(record))
        {
            eprintln!("app host: task bridge: {e}");
            *bridge = None;
        }
    }

    fn send_text(&self, op: u32, task: i32, text: &str) {
        self.send(&Record::with_text(op, task, text));
    }

    fn send_bounds(&self, task: i32, bounds: Rect) {
        self.send(&Record { op: window::TASK, task, bounds: [bounds.left, bounds.top, bounds.right, bounds.bottom], ..Default::default() });
    }

    /// The latest task of `uid`: its id and bounds.
    pub fn of_uid(&self, uid: u32) -> Option<(i32, Rect)> {
        self.tasks.lock().unwrap().iter().rev().find(|t| t.uid == uid).map(|t| (t.id, t.bounds))
    }

    /// The bounds of task `id`.
    pub fn bounds(&self, id: i32) -> Option<Rect> {
        self.tasks.lock().unwrap().iter().find(|t| t.id == id).map(|t| t.bounds)
    }

    /// The bounds of the tasks.
    fn all_bounds(&self) -> Vec<Rect> {
        self.tasks.lock().unwrap().iter().map(|t| t.bounds).collect()
    }

    /// Runs `f` with the layer of task `id`, made by `make` if it has none.
    pub fn with_layer<T>(
        &self,
        id: i32,
        make: impl FnOnce(&Task) -> Result<Layer, String>,
        f: impl FnOnce(&Layer) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut tasks = self.tasks.lock().unwrap();
        let task = tasks.iter_mut().find(|t| t.id == id).ok_or_else(|| format!("no task {id}"))?;
        if task.layer.is_none() {
            task.layer = Some(make(task)?);
        }
        f(task.layer.as_ref().unwrap())
    }

    /// The task `id`'s layer's place: its bounds, cut to them, in the
    /// tasks' order.
    fn placement(tasks: &[Task]) -> Vec<(&Layer, Change)> {
        tasks
            .iter()
            .enumerate()
            .filter_map(|(z, t)| {
                let layer = t.layer.as_ref()?;
                let b = t.bounds;
                Some((layer, Change {
                    position: Some((b.left as f32, b.top as f32)),
                    z: Some(z as i32),
                    crop: Some([0.0, 0.0, b.width() as f32, b.height() as f32]),
                }))
            })
            .collect()
    }
}

impl AppHost {
    /// Connects to the display server's task bridge at `socket`, in a
    /// thread of its own; in window mode it keeps the connection and
    /// takes the server's requests.
    pub fn connect_display(self: &Arc<Self>, socket: &Path) {
        let host = self.clone();
        let socket = socket.to_path_buf();
        std::thread::spawn(move || match bridge(&socket) {
            Ok((stream, windows)) => {
                let _ = host.tasks.windows.set(windows);
                if !windows {
                    return;
                }
                let Ok(reader) = stream.try_clone() else { return };
                *host.tasks.bridge.lock().unwrap() = Some(stream);
                host.take_requests(reader);
            }
            Err(e) => {
                let _ = host.tasks.windows.set(false);
                eprintln!("app host: task bridge: {e}");
            }
        });
    }

    /// A new task for the activity `class` of the process `start_seq`
    /// started for `uid`'s `package`: its bounds, in window mode.
    pub(super) fn new_task(&self, start_seq: i64, uid: u32, package: &str, class: &str, activity: super::freeform::Activity) -> Option<Rect> {
        if !self.tasks.window_mode() {
            return None;
        }
        let display = self.display().ok()?.0.clone();
        let area = Rect { left: 0, top: 0, right: display.width, bottom: display.height };
        let bounds = super::freeform::default_bounds(&activity, area, display.density_dpi(), false, &self.tasks.all_bounds());
        let id = {
            let mut next = self.tasks.next.lock().unwrap();
            *next += 1;
            *next
        };
        let component = format!("{package}/{class}");
        let launch = self.pending_launches.lock().unwrap().remove(package);
        self.tasks.tasks.lock().unwrap().push(Task {
            id,
            start_seq,
            uid,
            package: package.to_string(),
            component: component.clone(),
            bounds,
            min_size: activity.min_size,
            layer: None,
            launch,
        });
        self.tasks.send_bounds(id, bounds);
        self.tasks.send_text(window::PACKAGE, id, package);
        self.tasks.send_text(window::ACTIVITY, id, &component);
        self.tasks.send(&Record { op: window::FRONT, task: id, ..Default::default() });
        eprintln!("app host: task {id} of {component} at {bounds:?}");
        Some(bounds)
    }

    /// The layer of task `id`, the parent of its windows: made at its
    /// bounds, in front.
    pub(super) fn task_layer<T>(&self, id: i32, f: impl FnOnce(&Layer) -> Result<T, String>) -> Result<T, String> {
        let composer = self.composer().map_err(|e| format!("{e:?}"))?;
        let made = self.tasks.with_layer(
            id,
            |task| composer.create_surface(&self.process, &format!("Task={id} {}", task.component), surface::CONTAINER, None, &[]),
            f,
        )?;
        self.place_tasks()?;
        Ok(made)
    }

    /// Places every task's layer at its bounds, in the tasks' order.
    fn place_tasks(&self) -> Result<(), String> {
        let composer = self.composer().map_err(|e| format!("{e:?}"))?;
        let tasks = self.tasks.tasks.lock().unwrap();
        composer.apply(&Tasks::placement(&tasks))
    }

    /// An activity of `uid` is idle: the launch its task answers drew its
    /// first frame.
    pub(super) fn task_idle(&self, uid: u32) {
        let launch = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            tasks.iter_mut().rev().find(|t| t.uid == uid).and_then(|t| t.launch.take().map(|l| (t.id, l)))
        };
        if let Some((id, launch)) = launch {
            self.tasks.send_text(window::DRAWN, id, &launch);
        }
    }

    /// The process of `start_seq` died: its tasks are gone.
    pub(super) fn tasks_died(&self, start_seq: i64) {
        let gone: Vec<i32> = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let gone = tasks.iter().filter(|t| t.start_seq == start_seq).map(|t| t.id).collect();
            tasks.retain(|t| t.start_seq != start_seq);
            gone
        };
        for id in gone {
            self.tasks.send(&Record { op: window::REMOVED, task: id, ..Default::default() });
        }
    }

    /// Takes the server's requests until it closes the connection.
    fn take_requests(&self, mut stream: UnixStream) {
        loop {
            let mut record = Record::default();
            // SAFETY: `Record` is plain old data; every byte pattern is one.
            let buf = unsafe { std::slice::from_raw_parts_mut((&mut record as *mut Record).cast::<u8>(), size_of::<Record>()) };
            if stream.read_exact(buf).is_err() {
                *self.tasks.bridge.lock().unwrap() = None;
                return;
            }
            if let Err(e) = self.request(&record) {
                eprintln!("app host: task bridge request {}: {e}", record.op);
            }
        }
    }

    fn request(&self, r: &Record) -> Result<(), String> {
        if r.op == window::LAUNCH {
            eprintln!("app host: launch of {}", r.text());
        }
        match r.op {
            window::LAUNCH => self.launch(r.text()),
            window::FOCUS => self.focus_task(r.task),
            window::CLOSE => self.close_task(r.task),
            window::SET_BOUNDS => {
                // `resizeTask`.
                let [left, top, right, bottom] = r.bounds;
                self.move_task(r.task, Rect { left, top, right, bottom })
            }
            op => Err(format!("unknown request {op}")),
        }
    }

    /// `LAUNCH` of `package/class`: its package's task comes to the front,
    /// else the activity starts in a new one.
    fn launch(&self, component: &str) -> Result<(), String> {
        let (package, class) = component.split_once('/').ok_or_else(|| format!("{component}: not a component"))?;
        let running = self.tasks.tasks.lock().unwrap().iter().find(|t| t.package == package).map(|t| t.id);
        if let Some(id) = running {
            self.focus_task(id)?;
            self.tasks.send_text(window::DRAWN, id, component);
            return Ok(());
        }
        self.pending_launches.lock().unwrap().insert(package.to_string(), component.to_string());
        let class = if class.starts_with('.') { format!("{package}{class}") } else { class.to_string() };
        if let Err(e) = self.start_process(package, &class) {
            self.pending_launches.lock().unwrap().remove(package);
            // The server's splash waits for the launch's end.
            self.tasks.send_text(window::DRAWN, 0, component);
            return Err(format!("{e:?}"));
        }
        Ok(())
    }

    /// `FOCUS`: the task comes to the front, and its latest window takes
    /// the input focus.
    fn focus_task(&self, id: i32) -> Result<(), String> {
        let uid = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let at = tasks.iter().position(|t| t.id == id).ok_or_else(|| format!("no task {id}"))?;
            let task = tasks.remove(at);
            let uid = task.uid;
            tasks.push(task);
            uid
        };
        self.place_tasks()?;
        self.focus_window_of(uid)?;
        self.tasks.send(&Record { op: window::FRONT, task: id, ..Default::default() });
        Ok(())
    }

    /// `CLOSE`: removes the task as `removeTask` does, killing its
    /// process (`killProcessesForRemovedTask`: the process has no other
    /// task here); its death removes the task.
    fn close_task(&self, id: i32) -> Result<(), String> {
        let start_seq = self.tasks.tasks.lock().unwrap().iter().find(|t| t.id == id).map(|t| t.start_seq).ok_or_else(|| format!("no task {id}"))?;
        eprintln!("app host: removing task {id}");
        self.kill_process(start_seq)
    }

    /// `SET_BOUNDS` (`resizeTask`): the task takes `bounds`, at least its
    /// minimal size; its activities and windows are told, as
    /// `ActivityRecord.updateReportedConfigurationAndSend` and
    /// `WindowState.reportResized` tell them. The server takes the bounds
    /// the task got.
    fn move_task(&self, id: i32, bounds: Rect) -> Result<(), String> {
        let min_dp = self
            .inputs()
            .ok()
            .and_then(|i| i.packages.framework_dimension_dp("default_minimal_size_resizable_task"))
            .ok_or("no default_minimal_size_resizable_task")? as i32;
        let density = self.display().map_err(|e| format!("{e:?}"))?.0.density_dpi() as f32 / 160.0;
        let (uid, start_seq, bounds) = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let task = tasks.iter_mut().find(|t| t.id == id).ok_or_else(|| format!("no task {id}"))?;
            let default_min = (min_dp as f32 * density) as i32;
            let min = (if task.min_size.0 < 0 { default_min } else { task.min_size.0 }, if task.min_size.1 < 0 { default_min } else { task.min_size.1 });
            task.bounds = minimal_dimensions(bounds, task.bounds, min);
            (task.uid, task.start_seq, task.bounds)
        };
        self.place_tasks()?;
        self.move_windows_of(uid, bounds)?;
        let global = self.global_config(uid).ok_or("the task's app is not bound")?;
        let (mut items, relaunching) = self.reconfigure_activities(start_seq, &global, bounds);
        // A relaunching activity's windows are not told (`reportResized`).
        if !relaunching {
            items.extend(self.resize_windows(uid, &global, bounds)?);
        }
        if !items.is_empty() {
            self.schedule(start_seq, items)?;
        }
        self.tasks.send_bounds(id, bounds);
        Ok(())
    }
}

/// `Task.adjustForMinimalTaskDimensions`: `bounds` at least `min` wide and
/// high, grown away from the edge that stayed where it was in `previous`.
fn minimal_dimensions(mut bounds: Rect, previous: Rect, min: (i32, i32)) -> Rect {
    if bounds.width() < min.0 {
        if bounds.right == previous.right {
            bounds.left = bounds.right - min.0;
        } else {
            bounds.right = bounds.left + min.0;
        }
    }
    if bounds.height() < min.1 {
        if bounds.bottom == previous.bottom {
            bounds.top = bounds.bottom - min.1;
        } else {
            bounds.bottom = bounds.top + min.1;
        }
    }
    bounds
}

/// A connection to the task bridge of the display server at `socket`, and
/// whether it shows a window per task.
fn bridge(socket: &Path) -> Result<(UnixStream, bool), String> {
    let link = UnixStream::connect(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    let hello = wire::Request { op: wire::OP_WINDOWS, id: wire::VERSION, ..Default::default() };
    wire::send(link.as_fd(), wire::bytes(&hello), None).map_err(|e| e.to_string())?;
    let answer = wire::recv_record::<Windows>(link.as_fd())
        .map_err(|e| e.to_string())?
        .ok_or("the display server closed the connection")?;
    Ok((link, answer.mode == mode::WINDOWS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_keeps_its_minimal_size() {
        let previous = Rect { left: 100, top: 100, right: 900, bottom: 1500 };
        // Shrunk from the left: the right edge stays, the left one goes back.
        let from_left = Rect { left: 800, top: 100, right: 900, bottom: 1500 };
        assert_eq!(minimal_dimensions(from_left, previous, (440, 440)), Rect { left: 460, top: 100, right: 900, bottom: 1500 });
        // Shrunk from the bottom: the bottom edge grows back.
        let from_bottom = Rect { left: 100, top: 100, right: 900, bottom: 200 };
        assert_eq!(minimal_dimensions(from_bottom, previous, (440, 440)), Rect { left: 100, top: 100, right: 900, bottom: 540 });
        assert_eq!(minimal_dimensions(previous, previous, (440, 440)), previous);
    }
}
