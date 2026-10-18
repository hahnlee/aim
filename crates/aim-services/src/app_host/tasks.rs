//! The app host's tasks on the display, and window mode
//! (docs/windows.md): each task is a Mac window. A task's layer is a
//! container at its bounds, the parent of its windows' layers, as a task's
//! surface is the parent of its windows' in WindowManager, stacked in the
//! tasks' order (`activity_stack` decides it). On a fullscreen display a
//! task has the whole display; in window mode the app host is the display
//! server's task bridge, which `aim-windows` is for the original's tasks:
//! on the connection `FN_WINDOWS` makes (`aim_hostcall::display::window`)
//! it reports its tasks and takes the server's requests, and a task opens
//! at freeform's default bounds (`freeform`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use aim_host_display::wire;
use aim_hostcall::display::{Window as Record, Windows, mode, window};

use aim_binder_host::parcel::Exception;

use super::activity::Item;
use super::activity_stack::Command;
use super::config::{Configuration, Rect};
use super::{AppHost, exit_info};
use super::surface::{self, Change, Layer};

/// A task on the display: in window mode, a window.
pub struct Task {
    pub id: i32,
    /// Its root activity's uid.
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

    /// The front task of `uid`'s root activities.
    pub fn of_uid(&self, uid: u32) -> Option<i32> {
        self.tasks.lock().unwrap().iter().rev().find(|t| t.uid == uid).map(|t| t.id)
    }

    /// The bounds of task `id`.
    pub fn bounds(&self, id: i32) -> Option<Rect> {
        self.tasks.lock().unwrap().iter().find(|t| t.id == id).map(|t| t.bounds)
    }

    /// The minimal size of task `id` (`mMinWidth`, `mMinHeight`).
    pub fn min_size(&self, id: i32) -> Option<(i32, i32)> {
        self.tasks.lock().unwrap().iter().find(|t| t.id == id).map(|t| t.min_size)
    }

    /// The bounds of task `id` in window mode, a freeform task's.
    pub fn freeform_bounds(&self, id: i32) -> Option<Rect> {
        self.bounds(id).filter(|_| self.window_mode())
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
                    ..Default::default()
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
                host.activities.stack.lock().unwrap().freeform = windows;
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

    /// A new task, in front, for the activity `class` of `uid`'s
    /// `package`: its id. It has the display, or in window mode freeform's
    /// default bounds.
    pub(super) fn new_task(&self, uid: u32, package: &str, class: &str, activity: super::freeform::Activity) -> Result<i32, Exception> {
        let display = self.display()?.0.clone();
        let area = Rect { left: 0, top: 0, right: display.width, bottom: display.height };
        let windows = self.tasks.window_mode();
        let bounds = if windows { super::freeform::default_bounds(&activity, area, display.density_dpi(), false, &self.tasks.all_bounds()) } else { area };
        let id = {
            let mut next = self.tasks.next.lock().unwrap();
            *next += 1;
            *next
        };
        let component = format!("{package}/{class}");
        let launch = if windows { self.pending_launches.lock().unwrap().remove(package) } else { None };
        self.tasks.tasks.lock().unwrap().push(Task {
            id,
            uid,
            package: package.to_string(),
            component: component.clone(),
            bounds,
            min_size: activity.min_size,
            layer: None,
            launch,
        });
        if windows {
            self.tasks.send_bounds(id, bounds);
            self.tasks.send_text(window::PACKAGE, id, package);
            self.tasks.send_text(window::ACTIVITY, id, &component);
        }
        eprintln!("app host: task {id} of {component} at {bounds:?}");
        Ok(id)
    }

    /// Task `id` is in front and focused: its layer goes over the others,
    /// its windows in front of the others' with the latest focused, and
    /// the server is told.
    pub(super) fn task_to_front(&self, id: i32) -> Result<(), String> {
        {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let at = tasks.iter().position(|t| t.id == id).ok_or_else(|| format!("no task {id}"))?;
            let task = tasks.remove(at);
            tasks.push(task);
        }
        self.place_tasks()?;
        self.raise_windows_of(id)?;
        self.tasks.send(&Record { op: window::FRONT, task: id, ..Default::default() });
        Ok(())
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

    /// An activity of task `id` is idle: the launch the task answers drew
    /// its first frame.
    pub(super) fn task_idle(&self, id: i32) {
        let launch = self.tasks.tasks.lock().unwrap().iter_mut().find(|t| t.id == id).and_then(|t| t.launch.take());
        if let Some(launch) = launch {
            self.tasks.send_text(window::DRAWN, id, &launch);
        }
    }

    /// Task `id` has no activities left: it goes (`Task.removeIfPossible`),
    /// its layer with it; its processes stay.
    pub(super) fn remove_task(&self, id: i32) {
        let removed = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let before = tasks.len();
            tasks.retain(|t| t.id != id);
            tasks.len() != before
        };
        self.activities.task_tokens.lock().unwrap().remove(&id);
        if removed {
            self.tasks.send(&Record { op: window::REMOVED, task: id, ..Default::default() });
        }
    }

    /// Takes the server's requests until it closes the connection.
    fn take_requests(self: &Arc<Self>, mut stream: UnixStream) {
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

    fn request(self: &Arc<Self>, r: &Record) -> Result<(), String> {
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

    /// `LAUNCH` of `package/class`: the task a launcher's start would find
    /// for it (`findTask`) comes to the front, else the activity starts in
    /// a new one.
    fn launch(self: &Arc<Self>, component: &str) -> Result<(), String> {
        let (package, class) = component.split_once('/').ok_or_else(|| format!("{component}: not a component"))?;
        let class = if class.starts_with('.') { format!("{package}{class}") } else { class.to_string() };
        let info = self.activity_info_of(package, &class).map_err(|e| format!("{e:?}"))?;
        let user = info.info.application_info.uid as u32 / super::PER_USER_RANGE;
        let real = super::activity_start::real_component(&info);
        let found = {
            let stack = self.activities.stack.lock().unwrap();
            stack.find_task(&real, info.task_affinity.as_deref(), user, None).and_then(|t| stack.get(t)).map(|a| a.task)
        };
        if let Some(id) = found {
            self.focus_task(id)?;
            self.tasks.send_text(window::DRAWN, id, component);
            return Ok(());
        }
        self.pending_launches.lock().unwrap().insert(package.to_string(), component.to_string());
        if let Err(e) = self.start_launcher(package, &class, info) {
            self.pending_launches.lock().unwrap().remove(package);
            // The server's splash waits for the launch's end.
            self.tasks.send_text(window::DRAWN, 0, component);
            return Err(format!("{e:?}"));
        }
        Ok(())
    }

    /// `FOCUS`: the task comes to the front (`moveTaskToFront`), and its
    /// latest window takes the input focus.
    fn focus_task(self: &Arc<Self>, id: i32) -> Result<(), String> {
        let commands = self.activities.stack.lock().unwrap().bring_to_front(id);
        if !commands.contains(&Command::Front(id)) {
            self.task_to_front(id)?;
        }
        self.run(commands);
        Ok(())
    }

    /// `CLOSE`: removes the task as `removeTask` does: its activities
    /// finish, and the processes of its package are killed
    /// (`killTaskProcessesIfPossible`) unless one has an activity in
    /// another task.
    fn close_task(self: &Arc<Self>, id: i32) -> Result<(), String> {
        let (package, uid) = self.tasks.tasks.lock().unwrap().iter().find(|t| t.id == id).map(|t| (t.package.clone(), t.uid)).ok_or_else(|| format!("no task {id}"))?;
        eprintln!("app host: removing task {id}");
        let starts = self.starts_of_package(&package, uid / super::PER_USER_RANGE);
        let elsewhere = self.activities.stack.lock().unwrap().activities.iter().any(|a| a.task != id && starts.contains(&a.start_seq));
        let commands = self.activities.stack.lock().unwrap().remove(id);
        self.run(commands);
        if !elsewhere {
            for start_seq in starts {
                self.kill_process(start_seq, exit_info::REASON_USER_REQUESTED, exit_info::SUBREASON_REMOVE_TASK, "remove task")?;
            }
        }
        Ok(())
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
        let bounds = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let task = tasks.iter_mut().find(|t| t.id == id).ok_or_else(|| format!("no task {id}"))?;
            let default_min = (min_dp as f32 * density) as i32;
            let min = (if task.min_size.0 < 0 { default_min } else { task.min_size.0 }, if task.min_size.1 < 0 { default_min } else { task.min_size.1 });
            task.bounds = minimal_dimensions(bounds, task.bounds, min);
            task.bounds
        };
        self.place_tasks()?;
        self.move_windows_of(id, bounds)?;
        let uids: BTreeSet<u32> = self.activities.records.lock().unwrap().values().filter(|r| r.task == id).map(|r| r.uid).collect();
        let globals: HashMap<u32, Configuration> = uids.into_iter().filter_map(|uid| Some((uid, self.global_config(uid)?))).collect();
        let (mut items, relaunching) = self.reconfigure_activities(id, &globals, bounds);
        // A relaunching activity's windows are not told (`reportResized`).
        if !relaunching {
            items.extend(self.resize_windows(id, &globals, bounds)?);
        }
        let mut by_start: BTreeMap<i64, Vec<Box<dyn Item>>> = BTreeMap::new();
        for (start_seq, item) in items {
            by_start.entry(start_seq).or_default().push(item);
        }
        for (start_seq, items) in by_start {
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
