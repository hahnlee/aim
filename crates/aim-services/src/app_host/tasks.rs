//! The app host's tasks on the display, and window mode
//! (docs/windows.md): each task is a Mac window. A task's layer is a
//! container at its bounds, the parent of its windows' layers, as a task's
//! surface is the parent of its windows' in WindowManager, stacked in the
//! tasks' order (`activity_stack` decides it). On a fullscreen display a
//! task has the whole display; in window mode the app host is the display
//! server's task bridge, which `aim-windows` is for the original's tasks:
//! on the connection `FN_WINDOWS` makes (`aim_hostcall::display::window`)
//! it reports its tasks and takes the server's requests, and a task opens
//! at its saved launch bounds or freeform's default bounds (`freeform`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use aim_host_display::wire;
use aim_hostcall::display::{Window as Record, Windows, mode, window};

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use super::activity::Item;
use super::activity_stack::Command;
use super::config::{Configuration, Rect};
use super::launch_params::{Params, Persister};
use super::surface::{self, Change, Layer};
use super::{AppHost, exit_info};
use crate::package::info::ActivityInfo;

/// A task on the display: in window mode, a window.
pub struct Task {
    pub id: i32,
    /// Its root activity's uid.
    pub uid: u32,
    pub package: String,
    /// Its activity, `package/class`.
    pub component: String,
    real_activity: String,
    pub bounds: Rect,
    windowing_mode: i32,
    restore_bounds: Option<Rect>,
    /// Its root activity's window layout's minimum size, -1 for none.
    min_size: (i32, i32),
    /// Its layer, made with its first window's.
    layer: Option<Layer>,
    /// The server's `LAUNCH` it answers when its first real window draws.
    launch: Option<String>,
    window_layout_affinity: Option<String>,
    has_been_visible: bool,
    organized: bool,
}

impl Task {
    fn is_locked(&self) -> bool {
        false
    }
}

/// The tasks, back to front, and the server's connection.
#[derive(Default)]
pub struct Tasks {
    pub(super) app_switches: Mutex<super::app_switches::Policy>,
    pub(super) accessibility_uids: Mutex<Vec<i32>>,
    /// Whether the display server shows a window per task, once it said.
    pub(super) windows: OnceLock<bool>,
    bridge: Mutex<Option<UnixStream>>,
    tasks: Mutex<Vec<Task>>,
    next: Mutex<i32>,
    launch_params: Mutex<Option<Arc<Persister>>>,
}

impl Tasks {
    pub(super) fn launch_params(&self, data: &Path) -> Result<Arc<Persister>, Exception> {
        let mut owner = self.launch_params.lock().unwrap();
        if owner.is_none() {
            *owner = Some(Arc::new(
                Persister::new(data).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?,
            ));
        }
        Ok(owner.as_ref().unwrap().clone())
    }

    /// `ActivityTaskManagerInternal.isBaseOfLockedTask`: this task owner
    /// has no lock task mode, so none of its tasks is locked.
    pub(super) fn is_base_of_locked_task(&self, package: Option<&str>) -> bool {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .any(|task| Some(task.package.as_str()) == package && task.is_locked())
    }

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
        self.send(&Record {
            op: window::TASK,
            task,
            bounds: [bounds.left, bounds.top, bounds.right, bounds.bottom],
            ..Default::default()
        });
    }

    /// The front task of `uid`'s root activities.
    pub fn of_uid(&self, uid: u32) -> Option<i32> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|t| t.uid == uid)
            .map(|t| t.id)
    }

    /// The bounds of task `id`.
    pub fn bounds(&self, id: i32) -> Option<Rect> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.bounds)
    }

    /// The minimal size of task `id` (`mMinWidth`, `mMinHeight`).
    pub fn min_size(&self, id: i32) -> Option<(i32, i32)> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.min_size)
    }

    /// The bounds of task `id` in window mode, a freeform task's.
    pub fn freeform_bounds(&self, id: i32) -> Option<Rect> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id && t.windowing_mode == 5)
            .map(|t| t.bounds)
    }

    /// The bounds of the tasks.
    fn all_bounds(&self) -> Vec<Rect> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.bounds)
            .collect()
    }

    /// Runs `f` with the layer of task `id`, made by `make` if it has none.
    pub fn with_layer<T>(
        &self,
        id: i32,
        make: impl FnOnce(&Task) -> Result<Layer, String>,
        f: impl FnOnce(&Layer) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut tasks = self.tasks.lock().unwrap();
        let task = tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or_else(|| format!("no task {id}"))?;
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
                Some((
                    layer,
                    Change {
                        position: (!t.organized).then_some((b.left as f32, b.top as f32)),
                        z: Some(z as i32),
                        crop: (!t.organized).then_some([
                            0.0,
                            0.0,
                            b.width() as f32,
                            b.height() as f32,
                        ]),
                        ..Default::default()
                    },
                ))
            })
            .collect()
    }
}

impl AppHost {
    /// `ActivityTaskManagerService.Lifecycle.onUserUnlocked` →
    /// `ActivityTaskSupervisor.onUserUnlocked`: load the user's launch params.
    pub(super) fn activity_task_user_unlocked(&self, user: i32) -> Result<(), Exception> {
        let capture = self.package_scan_capture()?;
        self.tasks
            .launch_params(self.data()?)?
            .unlock_user(user, capture.state().packages.keys().cloned().collect())
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))
    }

    /// Connects to the display server's task bridge at `socket`, in a
    /// thread of its own; in window mode it keeps the connection and
    /// takes the server's requests.
    pub fn connect_display(self: &Arc<Self>, socket: &Path) {
        let _ = self.display_socket.set(socket.to_path_buf());
        self.windows.transitions.lock().unwrap().socket = Some(socket.to_path_buf());
        let host = self.clone();
        let socket = socket.to_path_buf();
        std::thread::spawn(move || match bridge(&socket) {
            Ok((stream, windows)) => {
                let _ = host.tasks.windows.set(windows);
                host.activities.stack.lock().unwrap().freeform = windows;
                if !windows {
                    return;
                }
                let Ok(reader) = stream.try_clone() else {
                    return;
                };
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
    /// saved or default bounds. It answers the package's pending launch, else the
    /// launch `source`'s task has not answered yet.
    #[cfg(test)]
    pub(super) fn new_task(
        &self,
        uid: u32,
        package: &str,
        class: &str,
        info: &ActivityInfo,
        source: Option<i32>,
    ) -> Result<i32, Exception> {
        self.new_task_with_options(uid, package, class, info, source, Default::default())
    }

    pub(super) fn new_task_with_options(
        &self,
        uid: u32,
        package: &str,
        class: &str,
        info: &ActivityInfo,
        source: Option<i32>,
        options: super::activity_options::LaunchOptions,
    ) -> Result<i32, Exception> {
        let display = self.display()?.0.clone();
        let area = Rect {
            left: 0,
            top: 0,
            right: display.width,
            bottom: display.height,
        };
        let windows = self.tasks.window_mode();
        let windowing_mode = options.windowing_mode(windows);
        let activity = super::freeform::Activity::from(info);
        let real_activity = super::activity_start::real_component(info);
        let bounds = if windowing_mode == 5 {
            let name = crate::package::preferred::unflatten(&real_activity)
                .ok_or_else(|| Exception::illegal_argument("invalid task component"))?;
            let saved = self
                .tasks
                .launch_params(self.data()?)?
                .get((uid / super::PER_USER_RANGE) as i32, &name)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
            let others = self.tasks.all_bounds();
            // Display and mode are already selected here. Only reuse params
            // for this display and a mode the freeform task owner supports.
            match saved.filter(|p| {
                p.display_unique_id.as_deref() == Some(format!("local:{}", display.id).as_str())
                    && matches!(p.windowing_mode, 0 | 5)
                    && p.bounds.width() > 0
                    && p.bounds.height() > 0
            }) {
                Some(p) => super::freeform::restored_bounds(area, &others, p.bounds),
                None => super::freeform::default_bounds(
                    &activity,
                    area,
                    display.density_dpi(),
                    false,
                    &others,
                ),
            }
        } else {
            area
        };
        let bounds = options
            .bounds
            .filter(|_| windowing_mode == 5)
            .map_or(bounds, |b| super::freeform::restored_bounds(area, &[], b));
        let id = {
            let mut next = self.tasks.next.lock().unwrap();
            *next += 1;
            *next
        };
        let component = format!("{package}/{class}");
        let launch = if windows {
            self.pending_launches.lock().unwrap().remove(package)
        } else {
            None
        };
        let mut tasks = self.tasks.tasks.lock().unwrap();
        let launch = launch.or_else(|| {
            tasks
                .iter_mut()
                .find(|t| Some(t.id) == source)
                .and_then(|t| t.launch.take())
        });
        tasks.push(Task {
            id,
            uid,
            package: package.to_string(),
            component: component.clone(),
            real_activity,
            bounds,
            windowing_mode,
            restore_bounds: (windowing_mode == 5).then_some(bounds).or(options.bounds),
            min_size: activity.min_size,
            layer: None,
            launch,
            window_layout_affinity: info.window_layout.as_ref().and_then(|l| l.affinity.clone()),
            has_been_visible: false,
            organized: false,
        });
        drop(tasks);
        self.activities
            .stack
            .lock()
            .unwrap()
            .set_task_windowing_mode(id, windowing_mode);
        if windows {
            self.tasks.send_bounds(id, bounds);
            self.tasks.send_text(window::PACKAGE, id, package);
            self.tasks.send_text(window::ACTIVITY, id, &component);
        }
        eprintln!("app host: task {id} of {component} at {bounds:?}");
        Ok(id)
    }

    pub(super) fn apply_task_launch_options(
        &self,
        id: i32,
        options: super::activity_options::LaunchOptions,
        info: &ActivityInfo,
    ) -> Result<(), String> {
        if options.mode == 0 && options.bounds.is_none() {
            return Ok(());
        }
        let display = self.display().map_err(|e| e.message)?;
        let area = Rect {
            left: 0,
            top: 0,
            right: display.0.width,
            bottom: display.0.height,
        };
        let (bounds, mode) = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let task = tasks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| format!("no task {id}"))?;
            if options.mode != 0 {
                task.windowing_mode = options.windowing_mode(self.tasks.window_mode());
            }
            let bounds = if task.windowing_mode == 5 {
                let bounds = options.bounds.or(task.restore_bounds).unwrap_or_else(|| {
                    super::freeform::default_bounds(
                        &super::freeform::Activity::from(info),
                        area,
                        display.0.density_dpi(),
                        false,
                        &[],
                    )
                });
                super::freeform::restored_bounds(area, &[], bounds)
            } else {
                if options.bounds.is_some() {
                    task.restore_bounds = options.bounds;
                }
                area
            };
            (bounds, task.windowing_mode)
        };
        self.activities
            .stack
            .lock()
            .unwrap()
            .set_task_windowing_mode(id, mode);
        self.move_task(id, bounds)
    }

    /// Task `id` is in front and focused: its layer goes over the others,
    /// its windows in front of the others' with the latest focused, and
    /// the server is told.
    pub(super) fn task_to_front(&self, id: i32) -> Result<(), String> {
        {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let at = tasks
                .iter()
                .position(|t| t.id == id)
                .ok_or_else(|| format!("no task {id}"))?;
            let task = tasks.remove(at);
            tasks.push(task);
        }
        self.place_tasks()?;
        self.raise_windows_of(id)?;
        self.tasks.send(&Record {
            op: window::FRONT,
            task: id,
            ..Default::default()
        });
        Ok(())
    }

    pub(super) fn task_to_back(&self, id: i32) -> Result<(), String> {
        {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let at = tasks
                .iter()
                .position(|t| t.id == id)
                .ok_or_else(|| format!("no task {id}"))?;
            let task = tasks.remove(at);
            tasks.insert(0, task);
        }
        self.place_tasks()?;
        self.tasks.send(&Record {
            op: window::MOVED_TO_BACK,
            task: id,
            ..Default::default()
        });
        Ok(())
    }

    /// The layer of task `id`, the parent of its windows: made at its
    /// bounds, in front.
    pub(super) fn task_layer<T>(
        &self,
        id: i32,
        f: impl FnOnce(&Layer) -> Result<T, String>,
    ) -> Result<T, String> {
        let composer = self.composer().map_err(|e| format!("{e:?}"))?;
        let made = self.tasks.with_layer(
            id,
            |task| {
                self.with_display_layer(|parent| {
                    composer.create_surface(
                        &self.process,
                        &format!("Task={id} {}", task.component),
                        surface::CONTAINER,
                        Some(parent),
                        &[],
                    )
                })
            },
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

    /// An activity of task `id` is idle: persist its visible launch bounds.
    pub(super) fn task_idle(&self, id: i32) {
        let mut tasks = self.tasks.tasks.lock().unwrap();
        let Some(task) = tasks.iter_mut().find(|t| t.id == id) else {
            return;
        };
        task.has_been_visible = true;
        if let Err(e) = self.save_task_launch_params(task) {
            eprintln!("app host: task {id} launch params: {e}");
        }
        drop(tasks);
        self.notify_task_organizers();
    }

    /// `ActivityRecord.onFirstWindowDrawn` / `removeStartingWindow`:
    /// a real app window drew, independently of the activity's queue idling.
    pub(super) fn task_drawn(&self, id: i32) {
        let launch = self
            .tasks
            .tasks
            .lock()
            .unwrap()
            .iter_mut()
            .find(|t| t.id == id)
            .and_then(|t| {
                t.has_been_visible = true;
                t.launch.take()
            });
        self.notify_task_organizers();
        if let Some(launch) = launch {
            eprintln!("app host: task {id} first window drawn for {launch}");
            self.tasks.send_text(window::DRAWN, id, &launch);
        }
    }

    /// Task `id` has no activities left: it goes (`Task.removeIfPossible`),
    /// its layer with it; its processes stay.
    pub(super) fn remove_task(&self, id: i32) {
        let removed = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            tasks
                .iter()
                .position(|t| t.id == id)
                .map(|at| tasks.remove(at))
        };
        self.activities.task_tokens.lock().unwrap().remove(&id);
        if let Some(task) = removed {
            self.remove_layers(task.layer.into_iter().collect());
            self.tasks.send(&Record {
                op: window::REMOVED,
                task: id,
                ..Default::default()
            });
        }
        self.notify_task_organizers();
    }

    /// Takes the server's requests until it closes the connection.
    fn take_requests(self: &Arc<Self>, mut stream: UnixStream) {
        loop {
            let mut record = Record::default();
            // SAFETY: `Record` is plain old data; every byte pattern is one.
            let buf = unsafe {
                std::slice::from_raw_parts_mut(
                    (&mut record as *mut Record).cast::<u8>(),
                    size_of::<Record>(),
                )
            };
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
                self.move_task(
                    r.task,
                    Rect {
                        left,
                        top,
                        right,
                        bottom,
                    },
                )
            }
            op => Err(format!("unknown request {op}")),
        }
    }

    /// `LAUNCH` of `package/class`: the task a launcher's start would find
    /// for it (`findTask`) comes to the front, else the activity starts in
    /// a new one.
    fn launch(self: &Arc<Self>, component: &str) -> Result<(), String> {
        let (package, class) = component
            .split_once('/')
            .ok_or_else(|| format!("{component}: not a component"))?;
        let class = if class.starts_with('.') {
            format!("{package}{class}")
        } else {
            class.to_string()
        };
        let info = self
            .activity_info_of(package, &class)
            .map_err(|e| format!("{e:?}"))?;
        let user = info.info.application_info.uid as u32 / super::PER_USER_RANGE;
        let real = super::activity_start::real_component(&info);
        let found = {
            let stack = self.activities.stack.lock().unwrap();
            stack
                .find_task(&real, info.task_affinity.as_deref(), user, None)
                .and_then(|t| stack.get(t))
                .map(|a| a.task)
        };
        if let Some(id) = found {
            self.focus_task(id)?;
            self.tasks.send_text(window::DRAWN, id, component);
            return Ok(());
        }
        self.pending_launches
            .lock()
            .unwrap()
            .insert(package.to_string(), component.to_string());
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
        let (package, uid) = self
            .tasks
            .tasks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .map(|t| (t.package.clone(), t.uid))
            .ok_or_else(|| format!("no task {id}"))?;
        eprintln!("app host: removing task {id}");
        let starts = self.starts_of_package(&package, uid / super::PER_USER_RANGE);
        let elsewhere = self
            .activities
            .stack
            .lock()
            .unwrap()
            .activities
            .iter()
            .any(|a| a.task != id && starts.contains(&a.start_seq));
        let commands = self.activities.stack.lock().unwrap().remove(id);
        self.run(commands);
        if !elsewhere {
            for start_seq in starts {
                self.kill_process(
                    start_seq,
                    exit_info::REASON_USER_REQUESTED,
                    exit_info::SUBREASON_REMOVE_TASK,
                    "remove task",
                )?;
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
            .framework()
            .ok()
            .and_then(|i| i.framework_dimension_dp("default_minimal_size_resizable_task"))
            .ok_or("no default_minimal_size_resizable_task")? as i32;
        let display = self.display().map_err(|e| format!("{e:?}"))?;
        let density = display.0.density_dpi() as f32 / 160.0;
        let bounds = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let task = tasks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| format!("no task {id}"))?;
            let default_min = (min_dp as f32 * density) as i32;
            let min = (
                if task.min_size.0 < 0 {
                    default_min
                } else {
                    task.min_size.0
                },
                if task.min_size.1 < 0 {
                    default_min
                } else {
                    task.min_size.1
                },
            );
            task.bounds = if task.windowing_mode == 5 {
                minimal_dimensions(bounds, task.bounds, min)
            } else {
                Rect {
                    left: 0,
                    top: 0,
                    right: display.0.width,
                    bottom: display.0.height,
                }
            };
            if task.windowing_mode == 5 {
                task.restore_bounds = Some(task.bounds);
            }
            self.save_task_launch_params(task)?;
            task.bounds
        };
        self.place_tasks()?;
        self.move_windows_of(id, bounds)?;
        let uids: BTreeSet<u32> = self
            .activities
            .records
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.task == id)
            .map(|r| r.uid)
            .collect();
        let globals: HashMap<u32, Configuration> = uids
            .into_iter()
            .filter_map(|uid| Some((uid, self.global_config(uid)?)))
            .collect();
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
        self.organizer_tasks_changed().map_err(|e| e.message)?;
        Ok(())
    }

    /// Task.saveLaunchingStateIfNeeded: visible leaf tasks in a freeform
    /// display area. This owner currently has only freeform tasks there.
    fn save_task_launch_params(&self, task: &Task) -> Result<(), String> {
        if task.windowing_mode != 5 || !task.has_been_visible {
            return Ok(());
        }
        let display = self.display().map_err(|e| e.message)?;
        let name = crate::package::preferred::unflatten(&task.real_activity)
            .ok_or_else(|| format!("invalid task component {}", task.real_activity))?;
        self.tasks
            .launch_params(self.data().map_err(|e| e.message)?)
            .map_err(|e| e.message)?
            .save(
                (task.uid / super::PER_USER_RANGE) as i32,
                &name,
                Params {
                    display_unique_id: Some(format!("local:{}", display.0.id)),
                    windowing_mode: 5, // WINDOWING_MODE_FREEFORM
                    bounds: task.bounds,
                    window_layout_affinity: task.window_layout_affinity.clone(),
                },
            )
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
    let hello = wire::Request {
        op: wire::OP_WINDOWS,
        id: wire::VERSION,
        ..Default::default()
    };
    wire::send(link.as_fd(), wire::bytes(&hello), None).map_err(|e| e.to_string())?;
    let answer = wire::recv_record::<Windows>(link.as_fd())
        .map_err(|e| e.to_string())?
        .ok_or("the display server closed the connection")?;
    Ok((link, answer.mode == mode::WINDOWS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::local::{Call, Reply, Service};
    use aim_binder_host::parcel::{Binder, Parcel, Reader};

    #[derive(Default)]
    struct SurfaceTree {
        handles: Vec<Binder>,
        next: usize,
        children: BTreeSet<i32>,
        removed: Vec<i32>,
        parents: BTreeMap<i32, Option<i32>>,
        fail_create: Option<usize>,
        fail_apply: bool,
    }
    struct Surfaces(Arc<Mutex<SurfaceTree>>);
    impl Service for Surfaces {
        fn descriptor(&self) -> &str {
            "android.ui.ISurfaceComposer"
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            let mut tree = self.0.lock().unwrap();
            let r = &mut call.data;
            if call.code == 100 {
                while r.remaining() > 0 {
                    tree.handles.push(r.read_binder()?.unwrap());
                }
            } else if call.code == 1 {
                r.enforce_interface("android.gui.ISurfaceComposerClient")?;
                let name = r.read_string16()?;
                assert_eq!(r.read_i32()?, surface::CONTAINER);
                if tree.fail_create == Some(tree.next) {
                    tree.fail_create = None;
                    return Err(aim_binder_host::parcel::BAD_VALUE);
                }
                let id = tree.next as i32;
                let parent = r
                    .read_binder()?
                    .map(|b| tree.handles.iter().position(|h| *h == b).unwrap() as i32);
                tree.parents.insert(id, parent);
                if parent == Some(0) {
                    tree.children.insert(id);
                }
                let mut p = Parcel::new();
                p.write_no_exception();
                p.write_i32(1);
                p.write_i32(0);
                p.write_binder(Some(tree.handles[tree.next]));
                p.write_i32(id);
                p.write_string16(name.as_deref());
                p.write_i32(0);
                tree.next += 1;
                return Ok(p);
            } else {
                assert_eq!(call.code, 8);
                if std::mem::take(&mut tree.fail_apply) {
                    return Err(aim_binder_host::parcel::BAD_VALUE);
                }
                r.enforce_interface("android.ui.ISurfaceComposer")?;
                let start = r.position();
                r.skip(r.remaining())?;
                let (bytes, objects) = r.since(start);
                for &at in &objects[1..] {
                    // apply token, then layer handles
                    let mut r = Reader::new(bytes, &objects);
                    r.set_position(at as usize);
                    r.read_binder()?;
                    let id = r.read_i32()?;
                    let what = r.read_i64()?;
                    if id < 0 || id as usize >= tree.next {
                        continue;
                    } // nested client objects
                    if what & 0x8000 != 0 {
                        r.skip(56)?;
                        assert!(!r.read_bool()?);
                        let parent = if r.read_bool()? {
                            r.read_binder()?; // native SurfaceControl client
                            let handle = r.read_binder()?.unwrap();
                            let parent = r.read_i32()?;
                            assert_eq!(handle, tree.handles[parent as usize]);
                            assert!(r.read_string16()?.is_some());
                            r.skip(16)?; // hint, dimensions, format
                            Some(parent)
                        } else {
                            None
                        };
                        tree.parents.insert(id, parent);
                        tree.children.remove(&id);
                        if parent == Some(0) {
                            tree.children.insert(id);
                        }
                        if parent.is_none() {
                            tree.removed.push(id);
                        }
                    }
                }
            }
            Ok(Parcel::new())
        }
    }

    fn removal_under_display(cycles: usize, app_task: bool) {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let tree = Arc::new(Mutex::new(SurfaceTree::default()));
            let endpoint = export(Arc::new(Surfaces(tree.clone())));
            let handles: Vec<_> = (0..=cycles)
                .map(|_| export(Arc::new(super::super::activity::Token)))
                .collect();
            let mut init = Parcel::new();
            for handle in &handles {
                init.write_binder(Some(handle.binder()));
            }
            endpoint.transact(100, &init, false).unwrap();
            assert!(
                host.composer
                    .set(surface::Composer::for_test(&host.process, endpoint.handle,))
                    .is_ok()
            );
            let (bridge, mut records) = UnixStream::pair().unwrap();
            records
                .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            *host.tasks.bridge.lock().unwrap() = Some(bridge);
            for id in 1..=cycles as i32 {
                host.tasks.animation_test_task(id, Rect::default());
                host.task_layer(id, |_| Ok(())).unwrap();
                assert_eq!(tree.lock().unwrap().children, BTreeSet::from([id]));
                if app_task {
                    use aim_service_aidl::android_app_iapptask as iat;
                    super::super::task_info::tests::add_app_task(host, id);
                    let mut request = Parcel::new();
                    iat::FinishAndRemoveTask {}.write(&mut request);
                    for missing in [false, true] {
                        let p = super::super::task_info::tests::invoke_app_task(
                            host,
                            id,
                            10100,
                            iat::FINISH_AND_REMOVE_TASK,
                            &request,
                        );
                        assert_eq!(p.reader().read_exception().unwrap().is_err(), missing);
                    }
                    assert_eq!(
                        host.activities.stack.lock().unwrap().recent_tasks().count(),
                        0
                    );
                    assert!(host.activities.stack.lock().unwrap().task(id).is_none());
                } else {
                    host.remove_task(id);
                    host.remove_task(id); // an already removed task sends no transaction
                }
                let mut removed = vec![0; size_of::<Record>()];
                records.read_exact(&mut removed).unwrap();
                assert_eq!(
                    u32::from_ne_bytes(removed[..4].try_into().unwrap()),
                    window::REMOVED
                );
                assert_eq!(i32::from_ne_bytes(removed[4..8].try_into().unwrap()), id);
                assert!(host.tasks.bounds(id).is_none());
                assert!(tree.lock().unwrap().children.is_empty());
                assert_eq!(host.with_display_layer(|root| Ok(root.id)).unwrap(), 0);
            }
            assert_eq!(
                tree.lock().unwrap().removed,
                (1..=cycles as i32).collect::<Vec<_>>()
            );
            records.set_nonblocking(true).unwrap();
            assert_eq!(
                records.read(&mut [0]).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        });
    }

    #[test]
    fn removal_detaches_task_from_retained_display() {
        removal_under_display(1, false);
    }

    #[test]
    fn repeated_close_reopen_leaves_no_retired_containers() {
        removal_under_display(3, false);
    }

    #[test]
    fn app_task_removal_detaches_surface_and_releases_native_task_once() {
        removal_under_display(3, true);
    }

    #[test]
    fn organizer_migration_moves_window_children_and_cleans_failed_staging() {
        for fail_create in [true, false] {
            crate::system::with_discovery_binders(|host, _, _, export| {
                let tree = Arc::new(Mutex::new(SurfaceTree::default()));
                let endpoint = export(Arc::new(Surfaces(tree.clone())));
                let held: Vec<_> = (0..12)
                    .map(|_| export(Arc::new(super::super::activity::Token)))
                    .collect();
                let mut init = Parcel::new();
                for h in &held {
                    init.write_binder(Some(h.binder()));
                }
                endpoint.transact(100, &init, false).unwrap();
                assert!(
                    host.composer
                        .set(surface::Composer::for_test(&host.process, endpoint.handle))
                        .is_ok()
                );
                let organizer = Some(endpoint.binder());
                host.register_task_organizer(organizer).unwrap();
                let mut old = Vec::new();
                for id in [7, 8] {
                    host.tasks.animation_test_task(
                        id,
                        Rect {
                            left: 10,
                            top: 20,
                            right: 110,
                            bottom: 220,
                        },
                    );
                    old.push(
                        host.task_layer(id, |l| {
                            Ok(Layer::animation_test_layer(
                                host.process.strong(match l.handle {
                                    Binder::Handle(h) => h,
                                    _ => unreachable!(),
                                }),
                                l.id,
                            ))
                        })
                        .unwrap(),
                    );
                    host.animation_test_window(
                        host.process.strong(endpoint.handle),
                        1,
                        id,
                        id as u32,
                    );
                    let child = host
                        .task_layer(id, |parent| {
                            host.composer().unwrap().create_surface(
                                &host.process,
                                "real window container",
                                surface::CONTAINER,
                                Some(parent),
                                &[],
                            )
                        })
                        .unwrap();
                    host.organizer_test_window_layer(id as u32, child);
                }
                host.set_tasks_organized(true);
                if fail_create {
                    tree.lock().unwrap().fail_create = Some(6);
                } else {
                    tree.lock().unwrap().fail_apply = true;
                }
                assert!(host.unregister_task_organizer(organizer, None).is_err());
                assert_eq!(tree.lock().unwrap().parents[&2], Some(1));
                assert_eq!(tree.lock().unwrap().parents[&4], Some(3));
                assert!(
                    tree.lock()
                        .unwrap()
                        .parents
                        .iter()
                        .filter(|(id, _)| **id >= 5)
                        .all(|(_, p)| p.is_none())
                );
                for (id, layer) in [7, 8].into_iter().zip(&old) {
                    assert_eq!(host.task_layer(id, |l| Ok(l.id)).unwrap(), layer.id);
                }
                host.unregister_task_organizer(organizer, None).unwrap();
                let expected = tree.lock().unwrap().parents.clone();
                for layer in &old {
                    host.composer()
                        .unwrap()
                        .apply(&[(
                            layer,
                            Change {
                                position: Some((-1000., -1000.)),
                                alpha: Some(0.),
                                remove: true,
                                ..Default::default()
                            },
                        )])
                        .unwrap();
                }
                let parents = tree.lock().unwrap().parents.clone();
                for (child, task) in [(2, 7), (4, 8)] {
                    let new = host.task_layer(task, |l| Ok(l.id)).unwrap();
                    assert_eq!(parents[&child], Some(new));
                    assert_eq!(parents[&child], expected[&child]);
                    assert_eq!(parents[&new], Some(0));
                }
            });
        }
    }

    fn launch_host(ready: &AppHost, windows: bool) -> Arc<AppHost> {
        let host = AppHost::new(ready.process.clone());
        let data = ready.data().unwrap();
        super::super::configure_test_host(&host, data, data, data, Box::new(|_| None));
        host.display_metrics
            .set(Ok(super::super::display::Metrics::of(
                &super::super::display::Display::phone_for_test(),
            )))
            .unwrap();
        host.display
            .set(Arc::new((
                super::super::display::Display::phone_for_test(),
                super::super::display_service::Resources {
                    name: "test display".into(),
                    density_dpi: 320,
                    brightness: (10, 255, 102),
                    brightness_dim: 0.05,
                },
            )))
            .ok()
            .unwrap();
        host.tasks.windows.set(windows).unwrap();
        host
    }

    #[test]
    fn idle_does_not_complete_a_launch_without_a_drawn_window() {
        crate::system::with_discovery_session(|ready, _, _| {
            let host = launch_host(ready, false);
            let (bridge, mut receive) = UnixStream::pair().unwrap();
            receive.set_nonblocking(true).unwrap();
            host.tasks.launch_test_task(1, Rect::default(), bridge);
            host.task_idle(1);
            let mut byte = [0];
            assert_eq!(
                receive.read(&mut byte).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        });
    }

    #[test]
    fn launch_options_choose_task_geometry_and_validate_displays() {
        use super::super::kept::RawBundle;
        use aim_binder_host::parcel::{EX_SECURITY, Parcel};
        crate::system::with_discovery_binders(|ready, _, _, export| {
            let host = launch_host(ready, true);
            let info = ready.activity_info_of("p", "p.Main").unwrap();
            assert!(
                host.framework
                    .set(
                        super::super::resources::Framework::load(
                            &aim_paths::original_image(),
                            320,
                            "en-US"
                        )
                        .expect("framework resources for native task resizing")
                    )
                    .is_ok()
            );
            let endpoint = export(Arc::new(Surfaces(Default::default())));
            assert!(
                host.composer
                    .set(surface::Composer::for_test(&host.process, endpoint.handle))
                    .is_ok()
            );
            let bounds = Rect {
                left: 80,
                top: 100,
                right: 880,
                bottom: 1500,
            };
            let mut rect = Parcel::new();
            for n in [bounds.left, bounds.top, bounds.right, bounds.bottom] {
                rect.write_i32(n);
            }
            for (requested, mode) in [(5, 5), (1, 1), (2, 1), (6, 1)] {
                let bundle = RawBundle::empty()
                    .with_ints(&[
                        ("android.activity.windowingMode", requested),
                        ("android.activity.launchDisplayId", 0),
                    ])
                    .unwrap()
                    .with_parcelable(
                        "android:activity.launchBounds",
                        "android.graphics.Rect",
                        &rect,
                    )
                    .unwrap();
                let options = host.activity_launch_options(Some(&bundle), 2000).unwrap();
                let mut request = super::super::activity_start::StartRequest::launcher("p.Main");
                request.launch_options = options;
                let token = host
                    .start_in_process(
                        0,
                        10100,
                        "p",
                        info.clone(),
                        request,
                        &Default::default(),
                        None,
                    )
                    .unwrap();
                let id = host
                    .activities
                    .stack
                    .lock()
                    .unwrap()
                    .get(token)
                    .unwrap()
                    .task;
                if mode == 5 {
                    assert_eq!(host.tasks.freeform_bounds(id), Some(bounds));
                } else {
                    assert!(host.tasks.freeform_bounds(id).is_none());
                    let display = host.display().unwrap();
                    assert_eq!(
                        host.tasks.bounds(id),
                        Some(Rect {
                            left: 0,
                            top: 0,
                            right: display.0.width,
                            bottom: display.0.height,
                        })
                    );
                }
                assert_eq!(
                    host.tasks
                        .tasks
                        .lock()
                        .unwrap()
                        .last()
                        .unwrap()
                        .windowing_mode,
                    mode
                );
            }
            let id = host.new_task(10100, "p", "p.Main", &info, None).unwrap();
            let before = host.tasks.bounds(id).unwrap();
            for mode in [1, 5] {
                host.apply_task_launch_options(
                    id,
                    super::super::activity_options::LaunchOptions { mode, bounds: None },
                    &info,
                )
                .unwrap();
                assert_eq!(
                    host.tasks.freeform_bounds(id),
                    (mode == 5).then_some(before)
                );
            }
            for display in [-2, 1, 123] {
                let bundle = RawBundle::empty()
                    .with_ints(&[("android.activity.launchDisplayId", display)])
                    .unwrap();
                assert_eq!(
                    host.activity_launch_options(Some(&bundle), 2000)
                        .err()
                        .unwrap()
                        .code,
                    EX_SECURITY
                );
            }
            assert!(host.activity_launch_options(None, 2000).is_ok());
        });
    }

    #[test]
    fn a_launch_after_restart_restores_saved_task_bounds() {
        crate::system::with_discovery_session(|ready, _, _| {
            let mut info = ready.activity_info_of("p", "p.Main").unwrap();
            // An alias uses realActivity for both saving and reading.
            info.target_activity = Some("p.Real".into());
            let saved = Rect {
                left: 47,
                top: 83,
                right: 847,
                bottom: 1483,
            };
            {
                let host = launch_host(ready, true);
                std::fs::create_dir_all(host.data().unwrap().join("system_ce/0")).unwrap();
                let id = host.new_task(10100, "p", "p.Main", &info, None).unwrap();
                assert_ne!(host.tasks.bounds(id), Some(saved));
                host.tasks.tasks.lock().unwrap()[0].bounds = saved;
                host.task_idle(id);
            } // Drop drains the real writer; the next host starts with empty memory.
            let host = launch_host(ready, true);
            host.tasks
                .launch_params(host.data().unwrap())
                .unwrap()
                .unlock_user(0, BTreeSet::from(["p".into()]))
                .unwrap();
            // A fully resolved record keeps its size despite a new orientation.
            info.screen_orientation = 0;
            let id = host.new_task(10100, "p", "p.Main", &info, None).unwrap();
            assert_eq!(host.tasks.freeform_bounds(id), Some(saved));
            // PHASE_BOUNDS still avoids another task's corners.
            let next = host.new_task(10100, "p", "p.Main", &info, None).unwrap();
            let shifted = host.tasks.bounds(next).unwrap();
            assert_ne!(shifted, saved);
            assert_eq!(
                (shifted.width(), shifted.height()),
                (saved.width(), saved.height())
            );
        });
    }

    #[test]
    fn a_launch_ignores_a_record_for_a_missing_display() {
        crate::system::with_discovery_session(|ready, _, _| {
            let info = ready.activity_info_of("p", "p.Main").unwrap();
            let component = crate::package::preferred::unflatten("p/p.Main").unwrap();
            let saved = Rect {
                left: 47,
                top: 83,
                right: 847,
                bottom: 1483,
            };
            for (display, mode) in [("local:99", 5), ("local:0", 2), ("local:0", 1)] {
                {
                    let host = launch_host(ready, true);
                    std::fs::create_dir_all(host.data().unwrap().join("system_ce/0")).unwrap();
                    host.tasks
                        .launch_params(host.data().unwrap())
                        .unwrap()
                        .save(
                            0,
                            &component,
                            Params {
                                display_unique_id: Some(display.into()),
                                windowing_mode: mode,
                                bounds: saved,
                                ..Default::default()
                            },
                        )
                        .unwrap();
                }
                let host = launch_host(ready, true);
                host.tasks
                    .launch_params(host.data().unwrap())
                    .unwrap()
                    .unlock_user(0, BTreeSet::from(["p".into()]))
                    .unwrap();
                let id = host.new_task(10100, "p", "p.Main", &info, None).unwrap();
                let display = host.display().unwrap();
                let expected = super::super::freeform::default_bounds(
                    &super::super::freeform::Activity::from(&info),
                    Rect {
                        left: 0,
                        top: 0,
                        right: display.0.width,
                        bottom: display.0.height,
                    },
                    display.0.density_dpi(),
                    false,
                    &[],
                );
                assert_eq!(host.tasks.bounds(id), Some(expected), "mode {mode}");
                assert_ne!(host.tasks.bounds(id), Some(saved));
            }
        });
    }

    #[test]
    fn a_task_keeps_its_minimal_size() {
        let previous = Rect {
            left: 100,
            top: 100,
            right: 900,
            bottom: 1500,
        };
        // Shrunk from the left: the right edge stays, the left one goes back.
        let from_left = Rect {
            left: 800,
            top: 100,
            right: 900,
            bottom: 1500,
        };
        assert_eq!(
            minimal_dimensions(from_left, previous, (440, 440)),
            Rect {
                left: 460,
                top: 100,
                right: 900,
                bottom: 1500
            }
        );
        // Shrunk from the bottom: the bottom edge grows back.
        let from_bottom = Rect {
            left: 100,
            top: 100,
            right: 900,
            bottom: 200,
        };
        assert_eq!(
            minimal_dimensions(from_bottom, previous, (440, 440)),
            Rect {
                left: 100,
                top: 100,
                right: 900,
                bottom: 540
            }
        );
        assert_eq!(minimal_dimensions(previous, previous, (440, 440)), previous);
    }
}

#[cfg(test)]
impl Tasks {
    pub(super) fn launch_test_task(&self, id: i32, bounds: Rect, bridge: UnixStream) {
        self.animation_test_task(id, bounds);
        self.tasks.lock().unwrap().last_mut().unwrap().launch = Some("p/p.Main".into());
        *self.bridge.lock().unwrap() = Some(bridge);
    }

    pub(super) fn animation_test_task(&self, id: i32, bounds: Rect) {
        let windowing_mode = if self.window_mode() { 5 } else { 1 };
        self.tasks.lock().unwrap().push(Task {
            id,
            uid: 10100,
            package: "p".into(),
            component: "p/p.Main".into(),
            real_activity: "p/p.Main".into(),
            bounds,
            windowing_mode,
            restore_bounds: (windowing_mode == 5).then_some(bounds),
            min_size: (-1, -1),
            layer: None,
            launch: None,
            window_layout_affinity: None,
            has_been_visible: true,
            organized: false,
        });
    }
}

impl AppHost {
    pub(super) fn organizer_surfaces(
        &self,
    ) -> Result<Vec<(i32, bool, super::task_organizer::Leash)>, Exception> {
        let tasks = self.tasks.tasks.lock().unwrap();
        if !tasks.iter().any(|t| t.layer.is_some()) {
            return Ok(Vec::new());
        }
        let client = self.composer()?.binder();
        Ok(tasks
            .iter()
            .rev()
            .filter_map(|task| {
                Some((
                    task.id,
                    task.has_been_visible,
                    super::task_organizer::Leash::new(self, client, task.layer.as_ref()?),
                ))
            })
            .collect())
    }
    pub(super) fn set_tasks_organized(&self, organized: bool) {
        for task in self.tasks.tasks.lock().unwrap().iter_mut() {
            task.organized = organized;
        }
    }
}
impl Tasks {
    pub(super) fn migrate_organized_layers(
        &self,
        migrate: impl FnOnce(&[(i32, &Layer, Rect, i32)]) -> Result<Vec<Layer>, String>,
    ) -> Result<(), String> {
        let mut tasks = self.tasks.lock().unwrap();
        let current: Vec<_> = tasks
            .iter()
            .enumerate()
            .filter_map(|(z, t)| {
                Some((
                    t.id,
                    t.layer.as_ref().filter(|_| t.organized)?,
                    t.bounds,
                    z as i32,
                ))
            })
            .collect();
        let replacements = migrate(&current)?;
        assert_eq!(current.len(), replacements.len());
        for (task, layer) in tasks
            .iter_mut()
            .filter(|t| t.organized && t.layer.is_some())
            .zip(replacements)
        {
            task.layer = Some(layer);
        }
        Ok(())
    }
}

#[cfg(test)]
impl Tasks {
    pub(super) fn organizer_test_layer(&self, id: i32, layer: Layer) {
        self.tasks
            .lock()
            .unwrap()
            .iter_mut()
            .find(|t| t.id == id)
            .unwrap()
            .layer = Some(layer);
    }
}
