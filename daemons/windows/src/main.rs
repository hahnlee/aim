//! `aim-windows`: the task bridge of window mode (`docs/windows.md`).
//!
//! The display server tells it the mode it shows the display in. In window
//! mode the default display runs the original freeform windowing, and each
//! freeform task is one macOS window: the bridge reports the tasks
//! ActivityTaskManager's task listener announces (bounds, title, package,
//! focus) and carries out what the windows ask for (move and resize, focus,
//! close, launch). In device mode it sets the display back to fullscreen
//! windowing and exits.
//!
//! A task gets a window once it runs an app's activity and has bounds of its
//! own on the display: a task that fills the display (home, and anything
//! not freeform) is the desktop the windows float over, and a task without
//! activities (a split-screen root WMShell organizes, a task restored from
//! recents and not started) or off the display is not shown.

mod framework;

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::FromRawFd;
use std::sync::{Arc, Mutex};

use aim_hostcall::display::{Window, Windows, mode, window};
use aim_hostcall::guest;
use binder::Interface;

use framework::{
    BpActivityTaskManager, BpWindowManager, Event, ITaskStackListener, WINDOWING_MODE_FREEFORM,
    WINDOWING_MODE_FULLSCREEN,
};

/// The default display, the one the display server shows.
const DISPLAY: i32 = 0;
/// The height of a freeform task's caption, which the original window
/// decoration draws inside the top of the task: WMShell's
/// `freeform_decor_caption_height` in the pinned image's SystemUI.
const CAPTION_DP: i32 = 42;

#[derive(Default)]
struct Task {
    package: Option<String>,
    label: Option<String>,
    /// An activity of it has run (it came to the front or had focus).
    running: bool,
    /// The bounds last reported, while the task has a window.
    shown: Option<[i32; 4]>,
}

struct Bridge {
    atm: BpActivityTaskManager,
    host: Mutex<File>,
    tasks: Mutex<HashMap<i32, Task>>,
    /// The display's size in pixels.
    size: (i32, i32),
    /// The caption's height in pixels.
    caption: i32,
}

impl Bridge {
    fn send(&self, w: &Window) {
        // SAFETY: `Window` is plain old data.
        let bytes = unsafe {
            std::slice::from_raw_parts((w as *const Window).cast::<u8>(), size_of::<Window>())
        };
        if let Err(e) = self.host.lock().unwrap().write_all(bytes) {
            log::error!("display server: {e}");
            std::process::exit(1);
        }
    }

    /// Whether `bounds` make a window: on the display, not all of it.
    fn windowed(&self, bounds: [i32; 4]) -> bool {
        let [l, t, r, b] = bounds;
        let (w, h) = self.size;
        l < r && t < b && l < w && t < h && r > 0 && b > 0 && bounds != [0, 0, w, h]
    }

    /// Report what changed about `task`'s window: it appears, moves or
    /// goes. `force` reports its bounds even when they did not change (the
    /// answer to a resize the display server asked for).
    ///
    /// Bounds Android gave the task itself (its launch position, or a shift
    /// away from another task when an activity starts in it) do not always
    /// reach the task's surface: the legacy freeform transitions can leave
    /// the surface where it was, and the window would show another part of
    /// the display. Those bounds are committed with `resizeTask`, whose
    /// change transition places the surface.
    fn refresh(&self, task: i32, force: bool) {
        let bounds = match self.atm.task_bounds(task) {
            Ok(b) => b.filter(|&b| self.windowed(b)),
            Err(e) => {
                log::debug!("task {task}: bounds: {e}");
                None
            }
        };
        let mut out = Vec::new();
        let mut commit = None;
        {
            let mut tasks = self.tasks.lock().unwrap();
            let t = tasks.entry(task).or_default();
            let bounds = bounds.filter(|_| t.running && t.package.is_some());
            match (t.shown, bounds) {
                (None, Some(b)) => {
                    if let Some(p) = &t.package {
                        out.push(Window::with_text(window::PACKAGE, task, p));
                    }
                    if let Some(l) = &t.label {
                        out.push(Window::with_text(window::TITLE, task, l));
                    }
                    out.push(self.task_record(task, b));
                    commit = Some(b);
                }
                (Some(old), Some(b)) if old != b || force => {
                    out.push(self.task_record(task, b));
                    if !force {
                        commit = Some(b);
                    }
                }
                (Some(_), None) => out.push(Window {
                    op: window::REMOVED,
                    task,
                    ..Default::default()
                }),
                _ => {}
            }
            t.shown = bounds;
        }
        for w in &out {
            self.send(w);
        }
        if let Some(b) = commit
            && let Err(e) = self.atm.resize_task(task, b)
        {
            log::warn!("task {task}: commit bounds: {e}");
        }
    }

    fn task_record(&self, task: i32, bounds: [i32; 4]) -> Window {
        Window {
            op: window::TASK,
            task,
            bounds,
            caption: self.caption,
            ..Default::default()
        }
    }

    fn shown(&self, task: i32) -> bool {
        self.tasks
            .lock()
            .unwrap()
            .get(&task)
            .is_some_and(|t| t.shown.is_some())
    }

    fn front(&self, task: i32) {
        if self.shown(task) {
            self.send(&Window {
                op: window::FRONT,
                task,
                ..Default::default()
            });
        }
    }

    /// Take what `info` says of the task's package and label.
    fn update(&self, info: &aim_windows_core::TaskInfo) {
        let task = info.task_id;
        let mut out = Vec::new();
        {
            let mut tasks = self.tasks.lock().unwrap();
            let t = tasks.entry(task).or_default();
            t.running |= info.running;
            let shown = t.shown.is_some();
            if t.package.is_none() && info.package.is_some() {
                t.package = info.package.clone();
                if let (true, Some(p)) = (shown, &t.package) {
                    out.push(Window::with_text(window::PACKAGE, task, p));
                }
            }
            if t.label != info.label {
                t.label = info.label.clone();
                if shown {
                    let label = t.label.as_deref().unwrap_or_default();
                    out.push(Window::with_text(window::TITLE, task, label));
                }
            }
        }
        for w in &out {
            self.send(w);
        }
    }

    fn on_event(&self, e: Event) {
        match e {
            Event::StackChanged => {
                let known: Vec<i32> = self.tasks.lock().unwrap().keys().copied().collect();
                for task in known {
                    self.refresh(task, false);
                }
            }
            Event::Created(task, package) => {
                self.tasks.lock().unwrap().entry(task).or_default().package = package;
                self.refresh(task, false);
            }
            Event::Removed(task) => {
                let t = self.tasks.lock().unwrap().remove(&task);
                if t.is_some_and(|t| t.shown.is_some()) {
                    self.send(&Window {
                        op: window::REMOVED,
                        task,
                        ..Default::default()
                    });
                }
            }
            Event::MovedToFront(info) if info.display_id == DISPLAY => {
                self.update(&info);
                self.refresh(info.task_id, false);
                self.front(info.task_id);
            }
            Event::DescriptionChanged(info) if info.display_id == DISPLAY => self.update(&info),
            Event::MovedToBack(info) if self.shown(info.task_id) => self.send(&Window {
                op: window::MOVED_TO_BACK,
                task: info.task_id,
                ..Default::default()
            }),
            Event::Focused(task) => {
                let started = {
                    let mut tasks = self.tasks.lock().unwrap();
                    let t = tasks.entry(task).or_default();
                    !std::mem::replace(&mut t.running, true)
                };
                if started {
                    self.refresh(task, false);
                }
                self.front(task);
            }
            Event::MovedToFront(_) | Event::DescriptionChanged(_) | Event::MovedToBack(_) => {}
        }
    }

    /// Carry out one request of the display server.
    fn request(&self, w: &Window) {
        let task = w.task;
        let r = match w.op {
            window::SET_BOUNDS => {
                let r = self.atm.resize_task(task, w.bounds);
                // The window follows the bounds the task got, which may
                // differ (a minimum size).
                self.refresh(task, true);
                r
            }
            window::FOCUS => self.atm.set_focused_task(task),
            window::CLOSE => self.atm.remove_task(task).map(drop),
            window::LAUNCH => match w.text().split_once('/') {
                Some((package, class)) => self.atm.start_activity(package, class).map(|r| {
                    if r < 0 {
                        log::warn!("start {}: {r}", w.text());
                    }
                }),
                None => {
                    log::warn!("start {}: not package/class", w.text());
                    Ok(())
                }
            },
            op => {
                log::warn!("unknown request {op}");
                Ok(())
            }
        };
        if let Err(e) = r {
            log::warn!("request {} for task {task}: {e}", w.op);
        }
    }
}

struct Listener(Arc<Bridge>);

impl Interface for Listener {}

impl ITaskStackListener for Listener {
    fn event(&self, e: Event) {
        self.0.on_event(e);
    }
}

fn main() {
    daemon_log::init("aim-windows");
    let mut w = Windows::default();
    let fd = match guest::display_windows(&mut w) {
        Ok(fd) => fd,
        Err(e) => {
            log::error!("no display server: {e:?}");
            std::process::exit(1);
        }
    };
    // SAFETY: the host call returned a new fd that we now own.
    let mut host = unsafe { File::from_raw_fd(fd) };
    binder::ProcessState::start_thread_pool();
    let (Some(atm), Some(wm)) = (
        framework::activity_task_manager(),
        framework::window_manager(),
    ) else {
        std::process::exit(1);
    };
    let windows = w.mode == mode::WINDOWS;
    set_windowing(&wm, windows);
    if !windows {
        return;
    }
    let (size, density) = match (wm.display_size(DISPLAY), wm.display_density(DISPLAY)) {
        (Ok(s), Ok(d)) => (s, d),
        (Err(e), _) | (_, Err(e)) => {
            log::error!("display {DISPLAY}: {e}");
            std::process::exit(1);
        }
    };
    // As `getDimensionPixelSize` rounds.
    let caption = (CAPTION_DP * density + 80) / 160;
    let Ok(writer) = host.try_clone() else {
        std::process::exit(1);
    };
    let bridge = Arc::new(Bridge {
        atm,
        host: Mutex::new(writer),
        tasks: Mutex::new(HashMap::new()),
        size,
        caption,
    });
    let listener = framework::new_listener(Listener(bridge.clone()));
    if let Err(e) = bridge.atm.register_task_stack_listener(&listener) {
        log::error!("registerTaskStackListener: {e}");
        std::process::exit(1);
    }
    log::info!(
        "window mode: display {}x{}, caption {caption} px",
        size.0,
        size.1
    );
    let mut w = Window::default();
    loop {
        // SAFETY: `Window` is plain old data; every byte pattern is a value.
        let buf = unsafe {
            std::slice::from_raw_parts_mut(
                (&mut w as *mut Window).cast::<u8>(),
                size_of::<Window>(),
            )
        };
        if host.read_exact(buf).is_err() {
            // The display server quit.
            return;
        }
        bridge.request(&w);
    }
}

/// Freeform windowing on the default display in window mode, fullscreen
/// otherwise; `setWindowingMode` persists it, so a device-mode boot after
/// a window-mode one resets it.
fn set_windowing(wm: &BpWindowManager, windows: bool) {
    let want = if windows {
        WINDOWING_MODE_FREEFORM
    } else {
        WINDOWING_MODE_FULLSCREEN
    };
    match wm.windowing_mode(DISPLAY) {
        Ok(m) if m == want => {}
        _ => {
            if let Err(e) = wm.set_windowing_mode(DISPLAY, want) {
                log::error!("setWindowingMode({DISPLAY}, {want}): {e}");
            }
        }
    }
}
