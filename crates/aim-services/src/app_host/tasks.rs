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
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use aim_host_display::wire;
use aim_hostcall::display::{Window as Record, Windows, mode, window};

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception};

use super::activity::{ACTIVITY_TYPE_HOME, Item};
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
    /// Its requested override windowing mode, 0 inheriting its parent's.
    requested_mode: i32,
    /// The organizer root task it is in (`getParent().asTask()`).
    parent: Option<i32>,
    /// Its requested override bounds, `None` if empty (inheriting its parent's).
    requested_bounds: Option<Rect>,
    /// `mLastNonFullscreenBounds`.
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
    /// `mCreatedByOrganizer`: an organizer's root task, of no activity
    /// and no Mac window of its own.
    root: Option<Root>,
}

/// An organizer root task's own state (`Task.Builder` in
/// `TaskOrganizerController.createRootTask`).
struct Root {
    /// `lastActiveTime`, set when it was made.
    last_active: i64,
    /// `mLaunchCookie`, held while the root task is.
    cookie: Option<(Binder, Option<Strong>)>,
}

/// What `TaskInfo` reports of an organizer root task.
pub(super) struct RootTask {
    pub windowing_mode: i32,
    pub bounds: Rect,
    pub last_active: i64,
    pub cookie: Option<Binder>,
}

impl Task {
    fn is_locked(&self) -> bool {
        false
    }
}

/// `Tasks::home` once the root home task has gone (`INVALID_TASK_ID`).
const HOME_GONE: i32 = -1;

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
    /// The root home task's id: 0 before the home first starts, `HOME_GONE`
    /// from its task's removal until the home starts again.
    home: AtomicI32,
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

    /// Sends `record` to the server, unless it is the root home task's:
    /// that is behind the desktop, never a Mac window.
    fn send(&self, record: &Record) {
        if record.task != 0 && record.task == self.home.load(Ordering::Relaxed) {
            return;
        }
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

    /// Sends task `task`'s `bounds` and windowing `mode`: its window's
    /// place, or the Mac's full screen.
    fn send_task(&self, task: i32, bounds: Rect, mode: i32) {
        self.send(&Record {
            op: window::TASK,
            task,
            bounds: [bounds.left, bounds.top, bounds.right, bounds.bottom],
            windowing_mode: mode,
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

    /// The bounds and windowing mode of task `id`.
    fn place(&self, id: i32) -> Option<(Rect, i32)> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .map(|t| (t.bounds, t.windowing_mode))
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

    /// The windowing mode and bounds of task `id` if its configuration is
    /// of its own bounds: a freeform task's in window mode, or one's in an
    /// organizer root task.
    pub fn own_bounds(&self, id: i32) -> Option<(i32, Rect)> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id && (t.windowing_mode == 5 || t.parent.is_some()))
            .map(|t| (t.windowing_mode, t.bounds))
    }

    /// The bounds of the tasks of a Mac window: not the organizer root
    /// tasks nor the root home task, behind the desktop.
    fn all_bounds(&self) -> Vec<Rect> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.root.is_none() && t.id != self.home.load(Ordering::Relaxed))
            .map(|t| t.bounds)
            .collect()
    }

    /// The organizer root task `id`.
    pub(super) fn root(&self, id: i32) -> Option<RootTask> {
        let tasks = self.tasks.lock().unwrap();
        let task = tasks.iter().find(|t| t.id == id)?;
        let root = task.root.as_ref()?;
        Some(RootTask {
            windowing_mode: task.windowing_mode,
            bounds: task.bounds,
            last_active: root.last_active,
            cookie: root.cookie.as_ref().map(|c| c.0),
        })
    }

    /// The root tasks, top to bottom (`DisplayContent.forAllRootTasks`).
    pub(super) fn root_task_ids(&self) -> Vec<i32> {
        self.children(None)
    }

    /// The tasks of the organizer root task `parent`, else the display
    /// area's, top to bottom.
    pub(super) fn children(&self, parent: Option<i32>) -> Vec<i32> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|t| t.parent == parent)
            .map(|t| t.id)
            .collect()
    }

    /// The organizer root task task `id` is in.
    pub(super) fn parent(&self, id: i32) -> Option<i32> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)?
            .parent
    }

    /// The windowing mode of task `id` (`getWindowingMode`).
    pub(super) fn windowing_mode(&self, id: i32) -> Option<i32> {
        self.tasks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.windowing_mode)
    }

    /// Moves task `id` over or under the other tasks, without telling the
    /// server: only for a task of no Mac window.
    fn restack(&self, id: i32, to_top: bool) {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(at) = tasks.iter().position(|t| t.id == id) {
            let task = tasks.remove(at);
            let at = if to_top { tasks.len() } else { 0 };
            tasks.insert(at, task);
        }
    }

    /// Runs `f` with the layer of task `id`, made by `make` if it has none,
    /// given its organizer root task's layer if it is in one.
    pub fn with_layer<T>(
        &self,
        id: i32,
        make: impl FnOnce(&Task, Option<&Layer>) -> Result<Layer, String>,
        f: impl FnOnce(&Layer) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut tasks = self.tasks.lock().unwrap();
        let at = tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| format!("no task {id}"))?;
        if tasks[at].layer.is_none() {
            let parent = tasks[at]
                .parent
                .and_then(|p| tasks.iter().find(|t| t.id == p)?.layer.as_ref());
            let layer = make(&tasks[at], parent)?;
            tasks[at].layer = Some(layer);
        }
        f(tasks[at].layer.as_ref().unwrap())
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
        // `startHomeActivity`'s root home task (`WINDOWING_MODE_UNDEFINED`:
        // the display area's) takes neither launch params nor a launch.
        let home = options.activity_type == ACTIVITY_TYPE_HOME;
        let activity = super::freeform::Activity::from(info);
        let real_activity = super::activity_start::real_component(info);
        // TaskLaunchParamsModifier: without launch bounds, params persisted for
        // this display in a freeform display area (window mode) give the mode and
        // the last non-fullscreen bounds. Only a mode this owner supports is reused.
        let saved = if windows && !home && options.bounds.is_none() {
            let name = crate::package::preferred::unflatten(&real_activity)
                .ok_or_else(|| Exception::illegal_argument("invalid task component"))?;
            self.tasks
                .launch_params(self.data()?)?
                .get((uid / super::PER_USER_RANGE) as i32, &name)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?
                .filter(|p| {
                    p.display_unique_id.as_deref() == Some(format!("local:{}", display.id).as_str())
                        && matches!(p.windowing_mode, 0 | 1 | 5)
                })
        } else {
            None
        };
        let windowing_mode = match &saved {
            _ if home => 0,
            Some(p) if p.windowing_mode != 0 => p.windowing_mode,
            _ => options.windowing_mode(windows),
        };
        let saved_bounds = saved
            .map(|p| p.bounds)
            .filter(|b| b.width() > 0 && b.height() > 0);
        let bounds = if windowing_mode == 5 {
            let others = self.tasks.all_bounds();
            match saved_bounds {
                Some(b) => super::freeform::restored_bounds(area, &others, b),
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
        if home {
            self.tasks.home.store(id, Ordering::Relaxed);
        }
        let component = format!("{package}/{class}");
        let launch = if windows && !home {
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
            requested_mode: windowing_mode,
            parent: None,
            requested_bounds: (windowing_mode == 5).then_some(bounds),
            restore_bounds: (windowing_mode == 5)
                .then_some(bounds)
                .or(options.bounds)
                .or(saved_bounds),
            min_size: activity.min_size,
            layer: None,
            launch,
            window_layout_affinity: info.window_layout.as_ref().and_then(|l| l.affinity.clone()),
            has_been_visible: false,
            organized: false,
            root: None,
        });
        // The root home task goes behind the desktop, under the others.
        if home && windows {
            tasks.rotate_right(1);
        }
        drop(tasks);
        // Its surface is made with it (`WindowContainer.setParent`), as it
        // may never have a window; an app task's is made with its first.
        if home {
            self.task_layer(id, |_| Ok(()))
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        }
        {
            let mut stack = self.activities.stack.lock().unwrap();
            stack.set_task_windowing_mode(id, windowing_mode);
            if home {
                stack.home = Some(id);
            }
        }
        if windows {
            self.tasks.send_task(id, bounds, windowing_mode);
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
                task.requested_mode = task.windowing_mode;
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
            |task, root| {
                let make = |parent: &Layer| {
                    composer.create_surface(
                        &self.process,
                        &format!("Task={id} {}", task.component),
                        surface::CONTAINER,
                        Some(parent),
                        &[],
                    )
                };
                match root {
                    Some(root) => make(root),
                    None => self.with_display_layer(make),
                }
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
    /// its layer with it; its processes stay. Once the root home task has
    /// gone, the home starts again when no task is left to focus
    /// (`resumeFocusedTasksTopActivities` to `resumeHomeActivity`), as the
    /// one resolved may have finished for another (a setup wizard once
    /// provisioned).
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
            if task.root.is_none() {
                self.tasks.send(&Record {
                    op: window::REMOVED,
                    task: id,
                    ..Default::default()
                });
            }
        }
        self.notify_task_organizers();
        let _ =
            self.tasks
                .home
                .compare_exchange(id, HOME_GONE, Ordering::Relaxed, Ordering::Relaxed);
        if self.home_to_restart()
            && let Some(host) = self.me.upgrade()
            && let Err(e) = host.start_home()
        {
            eprintln!("app host: home: {}", e.message);
        }
    }

    /// Whether the home is to start again: its root task has gone and no
    /// task is left to focus.
    fn home_to_restart(&self) -> bool {
        self.tasks.home.load(Ordering::Relaxed) == HOME_GONE
            && self.activities.stack.lock().unwrap().focused().is_none()
    }

    /// `TaskOrganizerController.createRootTask` on the display: a root
    /// task in windowing `mode` at the display's bounds, at the bottom
    /// (`Task.Builder`'s default position), its layer a container of the
    /// display's. It has no Mac window: each app task keeps its own.
    pub(super) fn create_root_task(
        &self,
        mode: i32,
        cookie: Option<Binder>,
    ) -> Result<i32, String> {
        let display = self.display().map_err(|e| e.message)?;
        let bounds = Rect {
            left: 0,
            top: 0,
            right: display.0.width,
            bottom: display.0.height,
        };
        let composer = self.composer().map_err(|e| e.message)?;
        let id = {
            let mut next = self.tasks.next.lock().unwrap();
            *next += 1;
            *next
        };
        let layer = self.with_display_layer(|parent| {
            composer.create_surface(
                &self.process,
                &format!("Task={id}"),
                surface::CONTAINER,
                Some(parent),
                &[],
            )
        })?;
        let held = match cookie {
            Some(Binder::Handle(handle)) => Some(self.process.strong(handle)),
            _ => None,
        };
        self.tasks.tasks.lock().unwrap().insert(
            0,
            Task {
                id,
                uid: 0,
                package: String::new(),
                component: String::new(),
                real_activity: String::new(),
                bounds,
                windowing_mode: mode,
                requested_mode: mode,
                parent: None,
                requested_bounds: None,
                restore_bounds: None,
                min_size: (-1, -1),
                layer: Some(layer),
                launch: None,
                window_layout_affinity: None,
                has_been_visible: false,
                organized: false,
                root: Some(Root {
                    last_active: super::alarm_service::elapsed_ms(),
                    cookie: cookie.map(|c| (c, held)),
                }),
            },
        );
        eprintln!("app host: root task {id} in windowing mode {mode}");
        self.place_tasks()?;
        Ok(id)
    }

    /// `TaskOrganizerState.dispose`: the organizer root tasks go with
    /// their organizer (`removeImmediately`); their ids.
    pub(super) fn remove_root_tasks(&self) -> Vec<i32> {
        let roots: Vec<Task> = self
            .tasks
            .tasks
            .lock()
            .unwrap()
            .extract_if(.., |t| t.root.is_some())
            .collect();
        let mut tokens = self.activities.task_tokens.lock().unwrap();
        for root in &roots {
            tokens.remove(&root.id);
        }
        drop(tokens);
        let ids = roots.iter().map(|r| r.id).collect();
        self.remove_layers(roots.into_iter().filter_map(|r| r.layer).collect());
        ids
    }

    /// `Task.reparent` of task `id` into the organizer root task `parent`,
    /// else onto the display area, over or under its new parent's tasks:
    /// its layer becomes its new parent's child and its windowing mode and
    /// bounds resolve in its new parent. It keeps its own Mac window.
    pub(super) fn reparent_task(
        self: &Arc<Self>,
        id: i32,
        parent: Option<i32>,
        to_top: bool,
    ) -> Result<(), String> {
        let composer = self.composer().map_err(|e| e.message)?;
        {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let at = tasks
                .iter()
                .position(|t| t.id == id)
                .ok_or_else(|| format!("no task {id}"))?;
            let mut task = tasks.remove(at);
            task.parent = parent;
            let at = match parent {
                Some(root) => {
                    let base = tasks
                        .iter()
                        .position(|t| t.id == root)
                        .ok_or_else(|| format!("no root task {root}"))?;
                    let above = tasks.iter().rposition(|t| t.parent == parent);
                    1 + if to_top { above.unwrap_or(base) } else { base }
                }
                None if to_top => tasks.len(),
                None => 0,
            };
            let root = parent.and_then(|p| tasks.iter().find(|t| t.id == p)?.layer.as_ref());
            if let Some(layer) = &task.layer {
                let reparent = |parent: &Layer| {
                    composer.apply(&[(
                        layer,
                        Change {
                            parent: Some(parent.control(composer.binder())),
                            ..Default::default()
                        },
                    )])
                };
                match root {
                    Some(root) => reparent(root),
                    None => self.with_display_layer(reparent),
                }?;
            }
            tasks.insert(at, task);
        }
        self.place_tasks()?;
        let change = self.task_change(id, None, None)?;
        self.apply_task_change(id, change)
    }

    /// Requested `bounds` of the organizer root task `root` (its parent's,
    /// the display's, if `None`), which its tasks resolve theirs in.
    pub(super) fn set_root_bounds(
        self: &Arc<Self>,
        root: i32,
        bounds: Option<Rect>,
    ) -> Result<(), String> {
        let display = self.display().map_err(|e| e.message)?;
        {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let task = tasks
                .iter_mut()
                .find(|t| t.id == root && t.root.is_some())
                .ok_or_else(|| format!("no root task {root}"))?;
            task.bounds = bounds.unwrap_or(Rect {
                left: 0,
                top: 0,
                right: display.0.width,
                bottom: display.0.height,
            });
        }
        for child in self.tasks.children(Some(root)) {
            let change = self.task_change(child, None, None)?;
            self.apply_task_change(child, change)?;
        }
        Ok(())
    }

    /// `positionChildAt` of the organizer root task `root` over or under
    /// the display's tasks: its tasks come with it in their order, the top
    /// one focused if it goes on top.
    pub(super) fn reorder_root_task(
        self: &Arc<Self>,
        root: i32,
        to_top: bool,
    ) -> Result<(), String> {
        let children = self.tasks.children(Some(root));
        if to_top {
            self.tasks.restack(root, true);
            for child in children.iter().rev() {
                self.focus_task(*child)?;
            }
        } else {
            for child in &children {
                self.reorder_task_to_back(*child)?;
            }
            self.tasks.restack(root, false);
        }
        self.place_tasks()
    }

    /// `Task.remove` of the organizer root tasks `roots`: their tasks leave
    /// them, their surfaces with the roots', and their activities finish.
    pub(super) fn remove_root_children(self: &Arc<Self>, roots: &[i32]) {
        let children: Vec<i32> = self
            .tasks
            .tasks
            .lock()
            .unwrap()
            .iter_mut()
            .filter(|t| t.parent.is_some_and(|p| roots.contains(&p)))
            .map(|t| {
                t.parent = None;
                t.id
            })
            .collect();
        for child in children {
            let commands = self.activities.stack.lock().unwrap().remove(child);
            self.run(commands);
        }
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
            window::SET_WINDOWING_MODE => {
                let set = self.set_task_windowing_mode(r.task, r.windowing_mode);
                // The task's record answers a refusal too.
                if set.is_err()
                    && let Some((bounds, mode)) = self.tasks.place(r.task)
                {
                    self.tasks.send_task(r.task, bounds, mode);
                }
                set
            }
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

    /// `SET_WINDOWING_MODE`: the user put task `id`'s window into or out of
    /// the Mac's full screen, which asks for fullscreen or freeform as
    /// `WindowContainerTransaction.setWindowingMode` does. A task in an
    /// organizer root task takes the root task's place, not the screen.
    fn set_task_windowing_mode(self: &Arc<Self>, id: i32, mode: i32) -> Result<(), String> {
        if !matches!(mode, 1 | 5) {
            return Err(format!("windowing mode {mode}"));
        }
        if self.tasks.root(id).is_some() || self.tasks.parent(id).is_some() {
            return Err(format!("task {id} is organized in a root task"));
        }
        let bounds = if mode == 5 {
            self.desktop_bounds(id)?.map(Some)
        } else {
            None
        };
        let change = self.task_change(id, Some(mode), bounds)?;
        self.apply_task_change(id, change)
    }

    /// The bounds task `id` takes on leaving the Mac's full screen when it
    /// has neither requested nor last non-fullscreen bounds (launched
    /// fullscreen): a new freeform task's, as the shell gives a task it
    /// moves to the desktop, rather than the display area's, whose window
    /// would have its title bar above the screen.
    fn desktop_bounds(&self, id: i32) -> Result<Option<Rect>, String> {
        let (component, others) = {
            let tasks = self.tasks.tasks.lock().unwrap();
            let t = tasks
                .iter()
                .find(|t| t.id == id)
                .ok_or_else(|| format!("no task {id}"))?;
            if t.requested_bounds.is_some() || t.restore_bounds.is_some() {
                return Ok(None);
            }
            let others: Vec<Rect> = tasks
                .iter()
                .filter(|o| o.root.is_none() && o.id != id)
                .map(|o| o.bounds)
                .collect();
            (t.component.clone(), others)
        };
        let (package, class) = component
            .split_once('/')
            .ok_or_else(|| format!("task {id} component {component}"))?;
        let info = self
            .activity_info_of(package, class)
            .map_err(|e| e.message)?;
        let display = self.display().map_err(|e| e.message)?;
        let area = Rect {
            left: 0,
            top: 0,
            right: display.0.width,
            bottom: display.0.height,
        };
        Ok(Some(super::freeform::default_bounds(
            &super::freeform::Activity::from(&info),
            area,
            display.0.density_dpi(),
            false,
            &others,
        )))
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
                .find_task(
                    &real,
                    info.task_affinity.as_deref(),
                    user,
                    None,
                    super::activity::ACTIVITY_TYPE_STANDARD,
                )
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
    pub(super) fn focus_task(self: &Arc<Self>, id: i32) -> Result<(), String> {
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
    /// the task got. In windowing `mode` if given, else in its own.
    pub(super) fn task_move_bounds(
        &self,
        id: i32,
        mode: Option<i32>,
        bounds: Rect,
    ) -> Result<Rect, String> {
        let min_dp = self
            .framework()
            .ok()
            .and_then(|i| i.framework_dimension_dp("default_minimal_size_resizable_task"))
            .ok_or("no default_minimal_size_resizable_task")? as i32;
        let display = self.display().map_err(|e| format!("{e:?}"))?;
        let density = display.0.density_dpi() as f32 / 160.0;
        let bounds = {
            let tasks = self.tasks.tasks.lock().unwrap();
            let task = tasks
                .iter()
                .find(|t| t.id == id)
                .ok_or_else(|| format!("no task {id}"))?;
            // Task.resolveLeafTaskOnlyOverrideConfigs: fullscreen fills its
            // parent; a multi-window task keeps its requested bounds.
            match mode.unwrap_or(task.windowing_mode) {
                5 => {}
                6 => return Ok(bounds),
                _ => {
                    return Ok(parent_of(&tasks, task).map_or(
                        Rect {
                            left: 0,
                            top: 0,
                            right: display.0.width,
                            bottom: display.0.height,
                        },
                        |p| p.1,
                    ));
                }
            }
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
            for (start, end, old_end, minimum) in [
                (bounds.left, bounds.right, task.bounds.right, min.0),
                (bounds.top, bounds.bottom, task.bounds.bottom, min.1),
            ] {
                let size = end.checked_sub(start).ok_or("task bounds overflow")?;
                if size <= 0 {
                    return Err("empty task bounds".into());
                }
                if size < minimum {
                    if end == old_end {
                        end.checked_sub(minimum)
                    } else {
                        start.checked_add(minimum)
                    }
                    .ok_or("minimum task bounds overflow")?;
                }
            }
            minimal_dimensions(bounds, task.bounds, min)
        };
        Ok(bounds)
    }

    pub(super) fn move_task(&self, id: i32, bounds: Rect) -> Result<(), String> {
        self.move_task_requesting(id, bounds, None)
    }

    /// `move_task`, with the task's requested override bounds `requested` if
    /// given, else its bounds if it is freeform. Like
    /// `Task.onConfigurationChangedInner`, a freeform task with requested
    /// bounds records its bounds as its last non-fullscreen ones.
    fn move_task_requesting(
        &self,
        id: i32,
        bounds: Rect,
        requested: Option<Option<Rect>>,
    ) -> Result<(), String> {
        let bounds = self.task_move_bounds(id, None, bounds)?;
        let mode = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let task = tasks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| format!("no task {id}"))?;
            task.bounds = bounds;
            match requested {
                Some(r) => task.requested_bounds = r,
                None if matches!(task.windowing_mode, 5 | 6) => {
                    task.requested_bounds = Some(bounds)
                }
                None => {}
            }
            if task.windowing_mode == 5 && task.requested_bounds.is_some() {
                task.restore_bounds = Some(bounds);
            }
            self.save_task_launch_params(task)?;
            task.windowing_mode
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
        // Each process following one of them is told first.
        let processes = self.process_configuration_changes();
        // A relaunching activity's windows are not told (`reportResized`).
        if !relaunching {
            items.extend(self.resize_windows(id, &globals, bounds)?);
        }
        let mut by_start: BTreeMap<i64, Vec<Box<dyn Item>>> = BTreeMap::new();
        for (start_seq, item) in processes.into_iter().chain(items) {
            by_start.entry(start_seq).or_default().push(item);
        }
        for (start_seq, items) in by_start {
            self.schedule(start_seq, items)?;
        }
        self.tasks.send_task(id, bounds, mode);
        self.organizer_tasks_changed().map_err(|e| e.message)?;
        Ok(())
    }

    /// A WCT `Change` of task `id`, admitted without mutating it, as
    /// `WindowOrganizerController.applyChanges` would apply it: its requested
    /// windowing mode (`mode`), then its requested bounds (`bounds`,
    /// `Some(None)` to inherit its parent's), which it keeps in any mode and
    /// which resolve in freeform and multi-window only. Both resolve in its
    /// parent: its organizer root task, else the display area.
    pub(super) fn task_change(
        &self,
        id: i32,
        mode: Option<i32>,
        bounds: Option<Option<Rect>>,
    ) -> Result<TaskChange, String> {
        let display = self.display().map_err(|e| e.message)?;
        let area = Rect {
            left: 0,
            top: 0,
            right: display.0.width,
            bottom: display.0.height,
        };
        let (current, requested_mode, requested, restore, parent) = {
            let tasks = self.tasks.tasks.lock().unwrap();
            let t = tasks
                .iter()
                .find(|t| t.id == id)
                .ok_or_else(|| format!("no task {id}"))?;
            (
                t.windowing_mode,
                t.requested_mode,
                t.requested_bounds,
                t.restore_bounds,
                parent_of(&tasks, t),
            )
        };
        let requested_mode = mode.unwrap_or(requested_mode);
        let mode = resolved_mode(
            self.tasks.window_mode(),
            requested_mode,
            parent.map(|p| p.0),
        );
        let area = parent.map_or(area, |p| p.1);
        let mut requested = bounds.unwrap_or(requested);
        // Task.onConfigurationChangedInner: a task entering freeform without
        // requested bounds takes its last non-fullscreen bounds; with neither,
        // it fills the display area.
        if mode == 5 && current != 5 && requested.is_none() {
            requested = restore;
        }
        let bounds = match requested {
            Some(b) if mode == 5 => self.task_move_bounds(id, Some(mode), b)?,
            Some(b) if mode == 6 => b,
            _ => area,
        };
        Ok(TaskChange {
            mode,
            requested_mode,
            bounds,
            requested,
        })
    }

    /// Applies a `task_change` of task `id`: the stack's visibility and
    /// lifecycle follow a changed windowing mode (`setWindowingMode`, then
    /// `ensureActivitiesVisible` and `resumeFocusedTasksTopActivities`).
    pub(super) fn apply_task_change(
        self: &Arc<Self>,
        id: i32,
        c: TaskChange,
    ) -> Result<(), String> {
        let changed = {
            let mut tasks = self.tasks.tasks.lock().unwrap();
            let task = tasks
                .iter_mut()
                .find(|t| t.id == id)
                .ok_or_else(|| format!("no task {id}"))?;
            task.requested_mode = c.requested_mode;
            std::mem::replace(&mut task.windowing_mode, c.mode) != c.mode
        };
        if changed {
            self.activities
                .stack
                .lock()
                .unwrap()
                .set_task_windowing_mode(id, c.mode);
        }
        self.move_task_requesting(id, c.bounds, Some(c.requested))?;
        if changed {
            let commands = self.activities.stack.lock().unwrap().ensure_visible();
            self.run(commands);
        }
        Ok(())
    }

    pub(super) fn reorder_task_to_back(self: &Arc<Self>, id: i32) -> Result<(), String> {
        let commands = self.activities.stack.lock().unwrap().reorder_to_back(id);
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
            op: window::REORDER_TO_BACK,
            task: id,
            ..Default::default()
        });
        self.run(commands);
        Ok(())
    }

    /// Task.saveLaunchingStateIfNeeded: visible fullscreen and freeform leaf
    /// tasks in a freeform display area (window mode), with their last
    /// non-fullscreen bounds (`LaunchParamsPersister.saveTaskToLaunchParam`).
    fn save_task_launch_params(&self, task: &Task) -> Result<(), String> {
        if !self.tasks.window_mode()
            || !task.has_been_visible
            || !matches!(task.windowing_mode, 1 | 5)
        {
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
                    windowing_mode: task.windowing_mode,
                    bounds: task.restore_bounds.unwrap_or_default(),
                    window_layout_affinity: task.window_layout_affinity.clone(),
                },
            )
    }
}

/// A task's admitted windowing mode, bounds and requested bounds
/// (`AppHost::task_change`).
pub(super) struct TaskChange {
    mode: i32,
    requested_mode: i32,
    bounds: Rect,
    requested: Option<Rect>,
}

/// The windowing mode and bounds of the organizer root task `task` is in.
fn parent_of(tasks: &[Task], task: &Task) -> Option<(i32, Rect)> {
    let parent = task.parent?;
    let p = tasks.iter().find(|t| t.id == parent)?;
    Some((p.windowing_mode, p.bounds))
}

/// `getWindowingMode` of a task requesting windowing mode `requested`, in
/// an organizer root task of windowing mode `parent` if in one, where an
/// undefined mode (or one this owner does not serve in a root task)
/// inherits the root task's. On the display area, a mode
/// `TaskDisplayArea.isValidWindowingMode` rejects is undefined and inherits
/// the display area's: fullscreen, or freeform in window mode.
fn resolved_mode(windows: bool, requested: i32, parent: Option<i32>) -> i32 {
    match parent {
        Some(_) if matches!(requested, 1 | 5 | 6) => requested,
        Some(p) => p,
        None if requested == 1 || !windows => 1,
        None => 5,
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
pub(super) mod tests {
    use super::*;
    use aim_binder_host::local::{Call, Reply, Service};
    use aim_binder_host::parcel::{Binder, Parcel, Reader};

    #[derive(Default)]
    pub(in crate::app_host) struct SurfaceTree {
        handles: Vec<Binder>,
        next: usize,
        children: BTreeSet<i32>,
        pub(in crate::app_host) removed: Vec<i32>,
        pub(in crate::app_host) parents: BTreeMap<i32, Option<i32>>,
        fail_create: Option<usize>,
        fail_apply: bool,
        held: Vec<aim_binder_host::local::Strong>,
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
                    assert_eq!(host.tasks.own_bounds(id), Some((5, bounds)));
                } else {
                    assert!(host.tasks.own_bounds(id).is_none());
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
                    super::super::activity_options::LaunchOptions {
                        mode,
                        ..Default::default()
                    },
                    &info,
                )
                .unwrap();
                assert_eq!(
                    host.tasks.own_bounds(id),
                    (mode == 5).then_some((5, before))
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
                // A freeform task persists its last non-fullscreen bounds.
                host.tasks.tasks.lock().unwrap()[0].restore_bounds = Some(saved);
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
            assert_eq!(host.tasks.own_bounds(id), Some((5, saved)));
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
            for (display, mode) in [("local:99", 5), ("local:0", 2), ("local:0", 5)] {
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
                                // A freeform record without bounds lays out by default.
                                bounds: if display == "local:0" && mode == 5 {
                                    Rect::default()
                                } else {
                                    saved
                                },
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
    fn a_launch_restores_a_saved_fullscreen_mode_and_its_last_bounds() {
        crate::system::with_discovery_session(|ready, _, _| {
            let info = ready.activity_info_of("p", "p.Main").unwrap();
            let component = crate::package::preferred::unflatten("p/p.Main").unwrap();
            let saved = Rect {
                left: 47,
                top: 83,
                right: 847,
                bottom: 1483,
            };
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
                            display_unique_id: Some("local:0".into()),
                            windowing_mode: 1,
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
            assert_eq!(host.tasks.own_bounds(id), None);
            let t = &host.tasks.tasks.lock().unwrap()[0];
            assert_eq!((t.windowing_mode, t.restore_bounds), (1, Some(saved)));
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

    /// Two freeform tasks 7 and 8 of one resumed activity each at `old` on
    /// the test phone display in window mode, task 7 at least 900x1100 and
    /// followed by its process: the process's thread, the activities and the
    /// server's end of the task bridge, and the composer's layers, of
    /// four surface handles.
    pub(in crate::app_host) fn wct_tasks(
        host: &Arc<AppHost>,
        export: &dyn Fn(Arc<dyn Service>) -> aim_binder_host::local::Strong,
        old: Rect,
    ) -> (
        Arc<super::super::process_configuration::tests::Recorder>,
        Vec<u64>,
        UnixStream,
        Arc<Mutex<SurfaceTree>>,
    ) {
        use super::super::activity_stack::{Activity, State};
        use super::super::process_configuration::tests::Recorder;
        let permissions = super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            Default::default(),
        );
        assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
        host.tasks.windows.set(true).unwrap();
        host.display_metrics
            .set(Ok(super::super::display::Metrics::of(
                &super::super::display::Display::phone_for_test(),
            )))
            .unwrap();
        host.display
            .set(Arc::new((
                super::super::display::Display::phone_for_test(),
                super::super::display_service::Resources {
                    name: "test".into(),
                    density_dpi: 320,
                    brightness: (10, 255, 102),
                    brightness_dim: 0.05,
                },
            )))
            .ok()
            .unwrap();
        host.framework
            .set(
                super::super::resources::Framework::load(
                    &aim_paths::original_image(),
                    320,
                    "en-US",
                )
                .unwrap(),
            )
            .ok()
            .unwrap();
        let tree = Arc::new(Mutex::new(SurfaceTree::default()));
        let endpoint = export(Arc::new(Surfaces(tree.clone())));
        let mut handles = Parcel::new();
        for _ in 0..4 {
            let held = export(Arc::new(super::super::activity::Token));
            handles.write_binder(Some(held.binder()));
            tree.lock().unwrap().held.push(held);
        }
        endpoint.transact(100, &handles, false).unwrap();
        host.composer
            .set(surface::Composer::for_test(&host.process, endpoint.handle))
            .ok()
            .unwrap();
        let global = host.global_configuration().unwrap();
        let mut app = super::super::tests::host_with_start(7, Arc::new(|| {}))
            .apps
            .lock()
            .unwrap()
            .remove(&7)
            .unwrap();
        app.uid = 10100;
        app.package = "p".into();
        app.packages = vec!["p".into()];
        let thread = Arc::new(Recorder::default());
        app.thread = Some(export(thread.clone()));
        app.config = Some(global.clone());
        host.apps.lock().unwrap().insert(7, app);
        let mut activity_tokens = Vec::new();
        for id in [7, 8] {
            host.tasks.animation_test_task(id, old);
            super::super::task_info::tests::add_app_task(host, id);
            let token = host.new_activity(
                7,
                10100,
                "p",
                host.activity_info_of("p", "p.Main").unwrap(),
                super::super::resources::WindowStyle::default(),
                id,
                super::super::activity_start::StartRequest::launcher("A"),
            );
            let mut records = host.activities.records.lock().unwrap();
            let record = records.get_mut(&token).unwrap();
            record.reported = Some(global.of_task(5, old));
            record.config_changes = i32::MAX;
            // The process follows the activity of task 7.
            record.ident = 9 - id;
            drop(records);
            let mut a = Activity::new(token, id, 7, "p/p.Main", 0, 0, None);
            a.state = State::Resumed;
            a.never_resumed = false;
            host.activities.stack.lock().unwrap().activities.push(a);
            activity_tokens.push(token);
        }
        host.activities.stack.lock().unwrap().freeform = true;
        host.tasks.tasks.lock().unwrap()[0].min_size = (900, 1100);
        let (bridge, receive) = UnixStream::pair().unwrap();
        receive.set_nonblocking(true).unwrap();
        *host.tasks.bridge.lock().unwrap() = Some(bridge);
        (thread, activity_tokens, receive, tree)
    }

    /// `startHomeActivity` of process 7's `p/p.Main`, left resizeable by
    /// default (`RESIZE_MODE_FORCE_RESIZEABLE`), into a new root home task
    /// after `wct_tasks`: its id and its activity's token. The activity is
    /// stopped behind the desktop.
    pub(in crate::app_host) fn home_task(host: &Arc<AppHost>) -> (i32, u64) {
        use super::super::activity::ACTIVITY_TYPE_HOME;
        use super::super::activity_stack::{Activity, State};
        let mut info = host.activity_info_of("p", "p.Main").unwrap();
        info.resize_mode = 4;
        let options = super::super::activity_options::LaunchOptions {
            activity_type: ACTIVITY_TYPE_HOME,
            ..Default::default()
        };
        let home = host
            .new_task_with_options(10100, "p", "p.Main", &info, None, options)
            .unwrap();
        super::super::task_info::tests::add_app_task(host, home);
        let mut request = super::super::activity_start::StartRequest::launcher("A");
        request.launch_options = options;
        let token = host.new_activity(
            7,
            10100,
            "p",
            info,
            super::super::resources::WindowStyle::default(),
            home,
            request,
        );
        let mut stack = host.activities.stack.lock().unwrap();
        let mut task = stack.tasks.pop().unwrap();
        task.intent.component = Some(crate::package::intent::ComponentName {
            package: "p".into(),
            class: "p.Main".into(),
        });
        stack.tasks.insert(0, task);
        let mut a = Activity::new(token, home, 7, "p/p.Main", 0, 0, None);
        a.state = State::Stopped;
        stack.activities.insert(0, a);
        (home, token)
    }

    #[test]
    fn the_root_home_task_is_behind_the_desktop_and_restarts_once_nothing_is_focused() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let old = Rect {
                left: 100,
                top: 100,
                right: 900,
                bottom: 1500,
            };
            let (_thread, _activities, mut receive, _) = wct_tasks(host, export, old);
            let (home, token) = home_task(host);
            // Under the app tasks, of no Mac window: the bridge hears nothing
            // of it, and the Mac-window bounds leave it out.
            assert_eq!(host.tasks.root_task_ids(), [8, 7, home]);
            assert_eq!(
                receive.read(&mut [0]).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert_eq!(host.tasks.all_bounds(), [old, old]);
            // `setActivityType`: a home activity is resizeable only if it
            // asked to be, so its task has no multi-window on a phone.
            assert_eq!(
                host.activities.records.lock().unwrap()[&token]
                    .info
                    .resize_mode,
                0
            );
            let info = host.organizer_task_info(home).unwrap();
            assert_eq!(
                (
                    info.activity_type,
                    info.configuration.window.activity_type,
                    info.resize_mode,
                    info.is_resizeable,
                    info.supports_multi_window,
                ),
                (2, 2, 0, false, false)
            );
            // Its process dies behind the focused apps: it goes unreported
            // and stays gone while an app is left to focus.
            {
                let mut stack = host.activities.stack.lock().unwrap();
                stack.activities.retain(|a| a.task != home);
                stack.tasks.retain(|t| t.id != home);
                stack.home = None;
            }
            host.remove_task(home);
            assert_eq!(
                receive.read(&mut [0]).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert_eq!(host.tasks.home.load(Ordering::Relaxed), HOME_GONE);
            assert!(!host.home_to_restart());
            // With nothing left to focus, the home starts again.
            host.activities.stack.lock().unwrap().activities.clear();
            assert!(host.home_to_restart());
        });
    }

    #[test]
    fn wct_dispatch_permission_admission_bounds_configuration_and_visible_reorder() {
        use super::super::{
            activity_stack::{State, Visibility},
            task_organizer::tests::apply_wct,
            window_container_transaction::{Change, Hop, Transaction, tests::fixture},
        };
        crate::system::with_discovery_binders(|host, _, _, export| {
            let old = Rect {
                left: 100,
                top: 100,
                right: 900,
                bottom: 1500,
            };
            let (thread, activity_tokens, mut receive, _) = wct_tasks(host, export, old);
            let token = host.task_token(7);
            let request = fixture(token);
            let denied = apply_wct(host, &request, 10100);
            assert_eq!(
                denied.reader().read_exception().unwrap().unwrap_err().code,
                aim_binder_host::parcel::EX_SECURITY
            );
            assert_eq!(host.tasks.bounds(7), Some(old));
            assert_eq!(
                host.tasks
                    .tasks
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|t| t.id == 7)
                    .unwrap()
                    .restore_bounds,
                Some(old)
            );
            assert_eq!(
                host.activities.records.lock().unwrap()[&activity_tokens[0]]
                    .reported
                    .as_ref()
                    .unwrap()
                    .window
                    .bounds,
                old
            );
            assert_eq!(
                host.activities
                    .stack
                    .lock()
                    .unwrap()
                    .tasks
                    .iter()
                    .map(|t| t.id)
                    .collect::<Vec<_>>(),
                [7, 8]
            );
            assert_eq!(
                receive.read(&mut [0]).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            let mut malformed = request.clone();
            malformed.set_i32_at(368, 3); // adjacency must fail before the valid resize
            let rejected = apply_wct(host, &malformed, 1000);
            assert_eq!(
                rejected
                    .reader()
                    .read_exception()
                    .unwrap()
                    .unwrap_err()
                    .code,
                aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION
            );
            // A valid resize followed by an unknown task must not change either owner or bridge.
            let mut bad = Transaction::read(&mut request.reader()).unwrap();
            bad.hierarchy
                .push(Hop::Reorder(Binder::Local(u64::MAX), false));
            assert!(host.apply_window_transaction(bad).is_err());
            let mut bad = Transaction::read(&mut request.reader()).unwrap();
            bad.changes.push(Change {
                token: host.task_token(8),
                bounds: Some(Some(Rect {
                    left: i32::MAX - 1,
                    top: 0,
                    right: i32::MAX,
                    bottom: 1,
                })),
                mode: None,
            });
            assert!(host.apply_window_transaction(bad).is_err());
            assert_eq!(host.tasks.bounds(7), Some(old));
            assert_eq!(
                host.activities
                    .stack
                    .lock()
                    .unwrap()
                    .tasks
                    .iter()
                    .map(|t| t.id)
                    .collect::<Vec<_>>(),
                [7, 8]
            );
            assert_eq!(
                receive.read(&mut [0]).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            let response = apply_wct(host, &request, 1000);
            let mut response = response.reader();
            response.read_exception().unwrap().unwrap();
            assert_eq!(response.remaining(), 0);
            let expected = Rect {
                left: -80,
                top: 120,
                right: 820,
                bottom: 1220,
            };
            assert_eq!(host.tasks.bounds(7), Some(expected));
            assert_eq!(host.tasks.bounds(8), Some(old));
            let records = host.activities.records.lock().unwrap();
            let config = records[&activity_tokens[0]].reported.as_ref().unwrap();
            assert_eq!(config.window.bounds, expected);
            assert_eq!(config.window.windowing_mode, 5);
            assert_eq!(
                (config.screen_width_dp, config.screen_height_dp),
                (450, 550)
            );
            // Its process is told before it, of its new size; then the
            // reorder moves the top resumed activity.
            let sent = thread.take(2);
            let [(Some(process), Some(next)), (None, Some(top))] = &sent[..] else {
                panic!("{sent:?}");
            };
            assert_eq!(
                top,
                "android.app.servertransaction.TopResumedActivityChangeItem"
            );
            assert_eq!(
                (process.screen_width_dp, process.screen_height_dp),
                (450, 550)
            );
            assert_eq!(process.window.bounds, expected);
            assert_eq!(
                next,
                "android.app.servertransaction.ActivityConfigurationChangeItem"
            );
            assert_eq!(
                records[&activity_tokens[1]]
                    .reported
                    .as_ref()
                    .unwrap()
                    .window
                    .bounds,
                old
            );
            drop(records);
            host.apply_window_transaction(Transaction {
                changes: vec![],
                hierarchy: vec![Hop::Reorder(token, false)],
            })
            .unwrap();
            let stack = host.activities.stack.lock().unwrap();
            assert_eq!(stack.tasks.iter().map(|t| t.id).collect::<Vec<_>>(), [7, 8]);
            for a in &stack.activities {
                assert_eq!(a.state, State::Resumed);
                assert_eq!(stack.visibility(a.task), Visibility::Visible);
            }
            assert_eq!(
                host.tasks
                    .tasks
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|t| t.id)
                    .collect::<Vec<_>>(),
                [7, 8]
            );
            drop(stack);
            let mut bytes = Vec::new();
            assert_eq!(
                receive.read_to_end(&mut bytes).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            let records: Vec<_> = bytes.chunks_exact(size_of::<Record>()).collect();
            assert!(
                records
                    .iter()
                    .any(|r| u32::from_ne_bytes(r[..4].try_into().unwrap())
                        == window::REORDER_TO_BACK)
            );
            assert!(
                !records.iter().any(
                    |r| u32::from_ne_bytes(r[..4].try_into().unwrap()) == window::MOVED_TO_BACK
                )
            );
            let bounds = records
                .iter()
                .find(|r| u32::from_ne_bytes(r[..4].try_into().unwrap()) == window::TASK)
                .unwrap();
            assert_eq!(
                (0..4)
                    .map(|i| i32::from_ne_bytes(bounds[8 + i * 4..12 + i * 4].try_into().unwrap()))
                    .collect::<Vec<_>>(),
                [-80, 120, 820, 1220]
            );
            assert_eq!(
                host.tasks
                    .tasks
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|t| t.id == 7)
                    .unwrap()
                    .restore_bounds,
                Some(expected)
            );
            // The same task owner used by #1678 resolves fullscreen bounds to the display.
            host.apply_task_launch_options(
                7,
                super::super::activity_options::LaunchOptions {
                    mode: 1,
                    ..Default::default()
                },
                &host.activity_info_of("p", "p.Main").unwrap(),
            )
            .unwrap();
            let display = host.display().unwrap();
            let area = Rect {
                left: 0,
                top: 0,
                right: display.0.width,
                bottom: display.0.height,
            };
            host.tasks.tasks.lock().unwrap()[0].min_size = (i32::MAX, i32::MAX);
            let response = apply_wct(host, &request, 1000);
            response.reader().read_exception().unwrap().unwrap();
            assert_eq!(host.tasks.bounds(7), Some(area));
            assert_eq!(host.tasks.own_bounds(7), None);
            assert_eq!(
                host.tasks
                    .tasks
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|t| t.id == 7)
                    .unwrap()
                    .restore_bounds,
                Some(expected)
            );
            let records = host.activities.records.lock().unwrap();
            let config = records[&activity_tokens[0]].reported.as_ref().unwrap();
            assert_eq!(config.window.windowing_mode, 1);
            assert_eq!(config.window.bounds, area);
            assert_eq!(
                records[&activity_tokens[1]]
                    .reported
                    .as_ref()
                    .unwrap()
                    .window
                    .bounds,
                old
            );
            drop(records);
            assert_eq!(host.tasks.bounds(8), Some(old));
            let mut bytes = Vec::new();
            assert_eq!(
                receive.read_to_end(&mut bytes).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            let last = bytes
                .chunks_exact(size_of::<Record>())
                .filter(|r| u32::from_ne_bytes(r[..4].try_into().unwrap()) == window::TASK)
                .last()
                .unwrap();
            assert_eq!(
                (0..4)
                    .map(|i| i32::from_ne_bytes(last[8 + i * 4..12 + i * 4].try_into().unwrap()))
                    .collect::<Vec<_>>(),
                [area.left, area.top, area.right, area.bottom]
            );
        });
    }

    #[test]
    fn wct_windowing_modes_own_task_geometry_configuration_and_lifecycle() {
        use super::super::{
            activity_stack::{State, Visibility},
            task_organizer::tests::apply_wct,
            window_container_transaction::{Change, Hop, Transaction, tests::fixture},
        };
        crate::system::with_discovery_binders(|host, _, _, export| {
            let old = Rect {
                left: 100,
                top: 100,
                right: 900,
                bottom: 1500,
            };
            let (_thread, activities, mut receive, _) = wct_tasks(host, export, old);
            let area = Rect {
                left: 0,
                top: 0,
                right: 1080,
                bottom: 1920,
            };
            let token = host.task_token(7);
            let task7 = |host: &AppHost| {
                let tasks = host.tasks.tasks.lock().unwrap();
                let t = tasks.iter().find(|t| t.id == 7).unwrap();
                (t.windowing_mode, t.bounds, t.restore_bounds)
            };
            let reported = |host: &AppHost, at: usize| {
                let records = host.activities.records.lock().unwrap();
                let c = records[&activities[at]].reported.clone().unwrap();
                (c.window.windowing_mode, c.window.bounds)
            };
            let last_task_record = |receive: &mut UnixStream| {
                let mut bytes = Vec::new();
                assert_eq!(
                    receive.read_to_end(&mut bytes).unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                let last = bytes
                    .chunks_exact(size_of::<Record>())
                    .filter(|r| u32::from_ne_bytes(r[..4].try_into().unwrap()) == window::TASK)
                    .last()
                    .map(|r| {
                        (0..4)
                            .map(|i| {
                                i32::from_ne_bytes(r[8 + i * 4..12 + i * 4].try_into().unwrap())
                            })
                            .collect::<Vec<_>>()
                    });
                last
            };
            // A mode change with a later unknown token is admitted as a whole, so nothing moves.
            let bad = Transaction {
                changes: vec![Change {
                    token,
                    bounds: None,
                    mode: Some(1),
                }],
                hierarchy: vec![Hop::Reorder(Binder::Local(u64::MAX), true)],
            };
            assert!(host.apply_window_transaction(bad).is_err());
            assert_eq!(task7(host), (5, old, Some(old)));
            assert_eq!(last_task_record(&mut receive), None);

            // setWindowingMode(FULLSCREEN) and reorder(onTop): the Change has no
            // configuration masks, so the pinned writer's bounds are not applied.
            let mut request = fixture(token);
            request.set_i32_at(340, 0);
            request.set_i32_at(344, 0);
            request.set_i32_at(352, 1);
            apply_wct(host, &request, 1000)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap();
            assert_eq!(task7(host), (1, area, Some(old)));
            assert_eq!(host.tasks.own_bounds(7), None);
            assert_eq!(reported(host, 0), (1, area));
            assert_eq!(reported(host, 1), (5, old));
            assert_eq!(last_task_record(&mut receive), Some(vec![0, 0, 1080, 1920]));
            {
                let stack = host.activities.stack.lock().unwrap();
                assert_eq!(stack.focused(), Some(7));
                // The fullscreen task in front hides the freeform one behind it.
                assert_eq!(stack.visibility(8), Visibility::Invisible);
                assert_ne!(stack.get(activities[1]).unwrap().state, State::Resumed);
            }

            let paused = host.activities.stack.lock().unwrap().paused(activities[1]);
            host.run(paused);
            // setWindowingMode(UNDEFINED), without a reorder, inherits the freeform
            // display area and takes the last non-fullscreen bounds, at least the
            // task's minimal size; the task behind it shows and resumes again.
            host.apply_window_transaction(Transaction {
                changes: vec![Change {
                    token,
                    bounds: None,
                    mode: Some(0),
                }],
                hierarchy: vec![],
            })
            .unwrap();
            let restored = Rect {
                left: 100,
                top: 100,
                right: 1000,
                bottom: 1500,
            };
            assert_eq!(task7(host), (5, restored, Some(restored)));
            assert_eq!(reported(host, 0), (5, restored));
            assert_eq!(
                last_task_record(&mut receive),
                Some(vec![100, 100, 1000, 1500])
            );
            {
                let stack = host.activities.stack.lock().unwrap();
                assert_eq!(stack.visibility(8), Visibility::Visible);
                assert_eq!(stack.get(activities[1]).unwrap().state, State::Resumed);
            }

            // setBounds(null) on a freeform task fills the display area but is not
            // recorded as its last non-fullscreen bounds.
            let mut request = fixture(token);
            for at in [244, 248, 252, 256] {
                request.set_i32_at(at, 0);
            }
            apply_wct(host, &request, 1000)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap();
            assert_eq!(task7(host), (5, area, Some(restored)));
            assert_eq!(host.tasks.own_bounds(7), Some((5, area)));
            assert_eq!(reported(host, 0), (5, area));
        });
    }

    #[test]
    fn the_macs_full_screen_sets_the_windowing_mode_and_records_answer() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let old = Rect {
                left: 100,
                top: 100,
                right: 1000,
                bottom: 1500,
            };
            let (_thread, _, mut receive, _) = wct_tasks(host, export, old);
            let task7 = || {
                let tasks = host.tasks.tasks.lock().unwrap();
                let t = tasks.iter().find(|t| t.id == 7).unwrap();
                (t.windowing_mode, t.bounds)
            };
            // The last TASK record: its bounds and windowing mode.
            let last = |receive: &mut UnixStream| {
                let mut bytes = Vec::new();
                assert_eq!(
                    receive.read_to_end(&mut bytes).unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                bytes
                    .chunks_exact(size_of::<Record>())
                    // SAFETY: `Record` is plain old data, `r` one's bytes.
                    .map(|r| unsafe { std::ptr::read_unaligned(r.as_ptr().cast::<Record>()) })
                    .filter(|r| r.op == window::TASK && r.task == 7)
                    .last()
                    .map(|r| (r.bounds, r.windowing_mode))
            };
            let set = |mode| {
                host.request(&Record {
                    op: window::SET_WINDOWING_MODE,
                    task: 7,
                    windowing_mode: mode,
                    ..Default::default()
                })
            };
            // The green button: fullscreen fills the display, and the record
            // says so, for the Mac's full screen.
            set(1).unwrap();
            assert_eq!(
                task7(),
                (
                    1,
                    Rect {
                        left: 0,
                        top: 0,
                        right: 1080,
                        bottom: 1920
                    }
                )
            );
            assert_eq!(last(&mut receive), Some(([0, 0, 1080, 1920], 1)));
            // Leaving it gives the task its last non-fullscreen bounds back.
            set(5).unwrap();
            assert_eq!(task7(), (5, old));
            assert_eq!(last(&mut receive), Some(([100, 100, 1000, 1500], 5)));
            // Another mode is refused, and the task's record answers that.
            assert!(set(6).is_err());
            assert_eq!(task7(), (5, old));
            assert_eq!(last(&mut receive), Some(([100, 100, 1000, 1500], 5)));

            // A task launched fullscreen has no freeform bounds to go back
            // to: leaving takes a new freeform task's, not the display's.
            {
                let mut tasks = host.tasks.tasks.lock().unwrap();
                let t = tasks.iter_mut().find(|t| t.id == 7).unwrap();
                (t.requested_bounds, t.restore_bounds) = (None, None);
                t.min_size = (-1, -1);
            }
            set(1).unwrap();
            set(5).unwrap();
            let display = host.display().unwrap();
            let others = [host.tasks.bounds(8).unwrap()];
            let expected = super::super::freeform::default_bounds(
                &super::super::freeform::Activity::from(
                    &host.activity_info_of("p", "p.Main").unwrap(),
                ),
                Rect {
                    left: 0,
                    top: 0,
                    right: display.0.width,
                    bottom: display.0.height,
                },
                display.0.density_dpi(),
                false,
                &others,
            );
            assert_eq!(task7(), (5, expected));
            assert_eq!(
                last(&mut receive),
                Some((
                    [expected.left, expected.top, expected.right, expected.bottom],
                    5
                ))
            );
        });
    }

    #[test]
    fn wct_mode_changes_keep_requested_bounds_and_persist_last_non_fullscreen_ones() {
        use super::super::window_container_transaction::{Change, Transaction};
        crate::system::with_discovery_binders(|host, _, _, export| {
            let old = Rect {
                left: 100,
                top: 100,
                right: 1000,
                bottom: 1500,
            };
            let _bridge = wct_tasks(host, export, old);
            let area = Rect {
                left: 0,
                top: 0,
                right: 1080,
                bottom: 1920,
            };
            let task = |id| {
                let tasks = host.tasks.tasks.lock().unwrap();
                let t = tasks.iter().find(|t| t.id == id).unwrap();
                (t.windowing_mode, t.bounds, t.restore_bounds)
            };
            let saved = || {
                let p = host
                    .tasks
                    .launch_params(host.data().unwrap())
                    .unwrap()
                    .get(
                        0,
                        &crate::package::preferred::unflatten("p/p.Main").unwrap(),
                    )
                    .unwrap()
                    .unwrap();
                (p.windowing_mode, p.bounds)
            };
            let change = |id, mode, bounds| {
                host.apply_window_transaction(Transaction {
                    changes: vec![Change {
                        token: host.task_token(id),
                        bounds,
                        mode,
                    }],
                    hierarchy: vec![],
                })
                .unwrap()
            };
            // setBounds(null) fills the display area; the last non-fullscreen
            // bounds stay those persisted.
            change(7, None, Some(None));
            assert_eq!(task(7), (5, area, Some(old)));
            assert_eq!(saved(), (5, old));
            // Bounds set with a fullscreen mode are requested, not resolved, and the
            // fullscreen task persists its mode.
            let requested = Rect {
                left: 50,
                top: 60,
                right: 1000,
                bottom: 1260,
            };
            change(7, Some(1), Some(Some(requested)));
            assert_eq!(task(7), (1, area, Some(old)));
            assert_eq!(saved(), (1, old));
            // Back in freeform, the requested bounds win over the last ones.
            change(7, Some(0), None);
            assert_eq!(task(7), (5, requested, Some(requested)));
            assert_eq!(saved(), (5, requested));

            // A task launched fullscreen without launch bounds has neither, so in
            // freeform it fills the display area and records nothing.
            {
                let mut tasks = host.tasks.tasks.lock().unwrap();
                let t = tasks.iter_mut().find(|t| t.id == 8).unwrap();
                (t.windowing_mode, t.bounds) = (1, area);
                (t.requested_bounds, t.restore_bounds) = (None, None);
            }
            host.activities
                .stack
                .lock()
                .unwrap()
                .set_task_windowing_mode(8, 1);
            change(8, Some(5), None);
            assert_eq!(task(8), (5, area, None));
            assert_eq!(saved(), (5, Rect::default()));
        });
    }
}

#[cfg(test)]
impl Tasks {
    pub(super) fn launch_test_task(&self, id: i32, bounds: Rect, bridge: UnixStream) {
        self.animation_test_task(id, bounds);
        self.tasks.lock().unwrap().last_mut().unwrap().launch = Some("p/p.Main".into());
        *self.bridge.lock().unwrap() = Some(bridge);
    }

    /// The root home task 1, of `h/h.Home`, filling `display`.
    pub(super) fn home_test_task(&self, display: Rect) {
        self.home.store(1, Ordering::Relaxed);
        self.tasks.lock().unwrap().insert(
            0,
            Task {
                id: 1,
                uid: 10050,
                package: "h".into(),
                component: "h/h.Home".into(),
                real_activity: "h/h.Home".into(),
                bounds: display,
                windowing_mode: 0,
                requested_mode: 0,
                parent: None,
                requested_bounds: None,
                restore_bounds: None,
                min_size: (-1, -1),
                layer: None,
                launch: None,
                window_layout_affinity: None,
                has_been_visible: false,
                organized: false,
                root: None,
            },
        );
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
            requested_mode: windowing_mode,
            parent: None,
            requested_bounds: (windowing_mode == 5).then_some(bounds),
            restore_bounds: (windowing_mode == 5).then_some(bounds),
            min_size: (-1, -1),
            layer: None,
            launch: None,
            window_layout_affinity: None,
            has_been_visible: true,
            organized: false,
            root: None,
        });
    }
}

impl AppHost {
    /// Each task of a layer, top to bottom: its id, whether it is ready to
    /// appear (`taskAppearedReady`: once visible, a root task at once),
    /// whether it is a root task, and its leash.
    pub(super) fn organizer_surfaces(
        &self,
    ) -> Result<Vec<(i32, bool, bool, super::task_organizer::Leash)>, Exception> {
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
                    task.has_been_visible || task.root.is_some(),
                    task.root.is_some(),
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
        migrate: impl FnOnce(&[(i32, &Layer, Rect, i32, Option<i32>)]) -> Result<Vec<Layer>, String>,
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
                    t.parent,
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
