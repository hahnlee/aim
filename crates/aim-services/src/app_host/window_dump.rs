//! WindowManagerService.dump / dumpDebugLocked, android-16.0.0_r1.
//! Field numbers come from the hash-pinned TEST_SOURCE_FILES in sources.lock.
//! Only state owned by the native display, task, activity and window owners is emitted.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::super::activity_stack::State;
use super::super::config::ProtoOut;
use super::*;
use aim_binder_host::parcel::FAILED_TRANSACTION;
use std::fmt::Write as _;
use std::io::Write as _;

/// Both output formats are built together from the same owner observation.
#[derive(Default)]
struct Message {
    proto: ProtoOut,
    text: String,
}
impl Message {
    fn int(&mut self, name: &str, field: u64, value: i32) {
        self.proto.int32(field, value);
        writeln!(self.text, "{name}: {value}").unwrap();
    }
    fn string(&mut self, name: &str, field: u64, value: &str) {
        self.proto.bytes(field, value.as_bytes());
        writeln!(self.text, "{name}: {value:?}").unwrap();
    }
    fn message(&mut self, name: &str, field: u64, value: Self) {
        self.proto.message(field, &value.proto.0);
        writeln!(self.text, "{name} {{").unwrap();
        for line in value.text.lines() {
            writeln!(self.text, "  {line}").unwrap();
        }
        self.text.push_str("}\n");
    }
    fn rect(&mut self, name: &str, field: u64, value: Rect) {
        self.message(
            name,
            field,
            Self {
                proto: ProtoOut(value.to_proto()),
                text: format!("{value:?}\n"),
            },
        );
    }

    fn child(&mut self, field: u64, node: Self) {
        let mut child = Self::default();
        let name = match field {
            3 => "display_content",
            4 => "display_area",
            5 => "task",
            6 => "activity",
            8 => "window",
            _ => unreachable!(),
        };
        child.message(name, field, node);
        self.message("children", 5, child);
    }
}

fn identifier(id: Option<i32>, user: Option<u32>, name: &str) -> Message {
    let mut out = Message::default();
    if let Some(id) = id {
        out.int("hash_code", 1, id);
    }
    if let Some(user) = user {
        out.int("user_id", 2, user as i32);
    }
    out.string("title", 3, name);
    out
}
fn container(
    name: &str,
    id: Option<i32>,
    user: Option<u32>,
    config: &Configuration,
    visible: bool,
) -> Message {
    let mut out = Message::default();
    let mut configs = Message::default();
    let full = Message {
        proto: ProtoOut(config.to_proto()),
        text: format!("{config:?}\n"),
    };
    configs.message("full_configuration", 2, full);
    out.message("configuration_container", 1, configs);
    out.int("visible", 3, i32::from(visible));
    out.message("identifier", 6, identifier(id, user, name));
    out
}
fn short_component(name: &str) -> String {
    match name.split_once('/') {
        Some((package, class))
            if class.starts_with(package) && class.as_bytes().get(package.len()) == Some(&b'.') =>
        {
            format!("{package}/{}", &class[package.len()..])
        }
        _ => name.into(),
    }
}

impl WindowManager {
    pub(super) fn dump(&self, call: &mut Call<'_>) -> Reply {
        // Binder.onTransact: raw fd then String[], no interface token.
        let fd = call.data.read_fd()?;
        let mut output = self
            .0
            .process
            .file(fd)
            .and_then(|file| aim_binder_host::server::file_fd(&file))
            .ok_or(BAD_VALUE)?;
        let args = aim_service_aidl::read_string_list(&mut call.data)?;
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        let result = (|| {
            if self
                .0
                .check_permission(Some("android.permission.DUMP"), call.sender_euid as i32)?
                != 0
            {
                return Ok(format!("Permission Denial: can't dump WindowManager from from pid={}, uid={} due to missing android.permission.DUMP permission\n", call.sender_pid, call.sender_euid).into_bytes());
            }
            let (proto, help, unknown) = options(args.as_deref().unwrap_or_default());
            if help {
                return Ok(b"Window manager dump options:\n  -a: include all available server state.\n  --proto: output dump in protocol buffer format.\n".to_vec());
            }
            let state = self.0.window_dump(&self.0.global_configuration()?);
            let mut bytes = unknown.into_bytes();
            bytes.extend(if proto {
                state.proto.0
            } else {
                state.text.into_bytes()
            });
            Ok(bytes)
        })();
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(e) => return reply(Err(e)),
        };
        output.write_all(&bytes).map_err(|_| FAILED_TRANSACTION)?;
        let mut p = Parcel::new();
        p.write_no_exception();
        Ok(p)
    }
}

fn options(args: &[Option<String>]) -> (bool, bool, String) {
    // PriorityDump consumes --proto wherever it occurs before WMS parses -a/-h.
    let proto = args.iter().any(|a| a.as_deref() == Some("--proto"));
    let mut unknown = String::new();
    for a in args.iter().filter(|a| a.as_deref() != Some("--proto")) {
        let Some(a) = a.as_deref().filter(|a| a.starts_with('-')) else {
            break;
        };
        match a {
            "-a" => {}
            "-h" => return (proto, true, unknown),
            _ => writeln!(unknown, "Unknown argument: {a}; use -h for help").unwrap(),
        }
    }
    (proto, false, unknown)
}

struct WindowDump<'a> {
    windows: &'a HashMap<u32, Window>,
    configs: &'a HashMap<u64, (Option<Configuration>, i32, i64, i32)>,
    task_configs: &'a HashMap<i32, Configuration>,
    shown: &'a HashMap<u64, bool>,
    global: &'a Configuration,
}
impl WindowDump<'_> {
    fn add(&self, parent: &mut Message, mut windows: Vec<&Window>) {
        windows.sort_by_key(|w| w.id);
        for w in windows {
            parent.child(8, self.node(w));
        }
    }
    fn roots(&self, activity: Option<u64>, task: Option<i32>) -> Vec<&Window> {
        self.windows
            .values()
            .filter(|w| w.parent.is_none() && w.activity == activity && w.task == task)
            .collect()
    }
    fn node(&self, w: &Window) -> Message {
        let config = w
            .activity
            .and_then(|a| self.configs.get(&a))
            .and_then(|r| r.0.as_ref())
            .or_else(|| w.task.and_then(|t| self.task_configs.get(&t)))
            .unwrap_or(self.global);
        let visible =
            w.layer.is_some() && w.activity.is_none_or(|a| self.shown.get(&a) == Some(&true));
        let mut wc = container(
            &w.name,
            Some(w.id as i32),
            Some(w.uid / super::super::PER_USER_RANGE),
            config,
            visible,
        );
        self.add(
            &mut wc,
            self.windows
                .values()
                .filter(|child| child.parent == Some(w.id))
                .collect(),
        );
        let mut out = Message::default();
        out.message("window_container", 1, wc);
        out.int("display_id", 3, 0);
        if let Some(task) = w.task {
            out.int("stack_id", 4, task);
        }
        let mut attrs = Message::default();
        attrs.int("type", 1, w.attrs.kind);
        attrs.int("flags", 24, w.attrs.flags);
        out.message("attributes", 5, attrs);
        let [left, top, right, bottom] = w.attrs.surface_insets;
        out.rect(
            "surface_insets",
            12,
            Rect {
                left,
                top,
                right,
                bottom,
            },
        );
        let mut animator = Message::default();
        let mut surface = Message::default();
        surface.int("shown", 1, i32::from(visible));
        if w.layer.is_some() {
            animator.message("surface", 2, surface);
        }
        out.message("animator", 13, animator);
        out.int("requested_width", 18, w.requested.0);
        out.int("requested_height", 19, w.requested.1);
        out.int("has_surface", 22, i32::from(w.layer.is_some()));
        out.int("is_visible", 38, i32::from(visible));
        let mut frames = Message::default();
        frames.rect("display_frame", 4, w.frames.display);
        frames.rect("frame", 5, w.frames.frame);
        frames.rect("parent_frame", 8, w.frames.parent);
        // The native owner applies no compatibility transform to these frames.
        frames.rect("compat_frame", 16, w.frames.frame);
        out.message("window_frames", 41, frames);
        out.int("requested_visible_types", 48, w.requested_visible_types);
        out
    }
}

impl AppHost {
    fn window_dump(&self, global: &Configuration) -> Message {
        // Never hold an owner lock while calling another owner or writing the fd.
        let (tasks, activities, focused, shown) = {
            let stack = self.activities.stack.lock().unwrap();
            (
                stack.tasks.clone(),
                stack.activities.clone(),
                stack.focused(),
                stack
                    .activities
                    .iter()
                    .map(|a| (a.token, stack.shown(a)))
                    .collect::<HashMap<_, _>>(),
            )
        };
        let configs: HashMap<_, _> = self
            .activities
            .records
            .lock()
            .unwrap()
            .iter()
            .map(|(token, r)| {
                (
                    *token,
                    (
                        r.reported.clone(),
                        r.requested_orientation,
                        r.start_seq,
                        r.info.resize_mode,
                    ),
                )
            })
            .collect();
        let pids: HashMap<_, _> = self
            .apps
            .lock()
            .unwrap()
            .iter()
            .map(|(s, a)| (*s, a.pid))
            .collect();
        let focus = self
            .windows
            .focus
            .lock()
            .unwrap()
            .current
            .as_ref()
            .map(|(id, _)| *id);
        let mut area = container("DefaultTaskDisplayArea", None, None, global, true);
        let mut task_configs = HashMap::new();
        for task in &tasks {
            let Some(bounds) = self.tasks.bounds(task.id) else {
                continue;
            };
            let mut config = if self.tasks.freeform_bounds(task.id).is_some() {
                global.of_freeform_task(bounds)
            } else {
                global.clone()
            };
            config.window.activity_type = super::super::activity::ACTIVITY_TYPE_STANDARD;
            task_configs.insert(task.id, config);
        }
        let min_sizes: HashMap<_, _> = tasks
            .iter()
            .filter_map(|t| self.tasks.min_size(t.id).map(|size| (t.id, size)))
            .collect();
        let windows = self.windows.windows.lock().unwrap();
        let window = WindowDump {
            windows: &windows,
            configs: &configs,
            task_configs: &task_configs,
            shown: &shown,
            global,
        };
        for task in &tasks {
            let Some(config) = task_configs.get(&task.id) else {
                continue;
            };
            let members: Vec<_> = activities.iter().filter(|a| a.task == task.id).collect();
            let visible = members.iter().any(|a| shown[&a.token]);
            let root = members.iter().find(|a| !a.finishing).map(|a| a.token);
            let mut wc = container(
                &format!("Task={}", task.id),
                Some(task.id),
                Some(task.user),
                config,
                visible,
            );
            let mut t = Message::default();
            t.int("id", 2, task.id);
            t.int("root_task_id", 16, task.id);
            t.int(
                "fills_parent",
                4,
                i32::from(config.window.bounds == global.window.bounds),
            );
            t.rect("bounds", 5, config.window.bounds);
            t.string("real_activity", 13, &short_component(&task.real_activity));
            if let Some(affinity) = &task.affinity {
                t.string("affinity", 29, affinity);
            }
            if let Some(a) = members.iter().rev().find(|a| a.state == State::Resumed) {
                t.message(
                    "resumed_activity",
                    12,
                    identifier(
                        Some(a.token as i32),
                        Some(a.user),
                        &short_component(&a.component),
                    ),
                );
            }
            for a in &members {
                let metadata = configs.get(&a.token);
                let config = metadata.and_then(|r| r.0.as_ref()).unwrap_or(config);
                let name = short_component(&a.component);
                let mut ac = container(
                    &name,
                    Some(a.token as i32),
                    Some(a.user),
                    config,
                    shown[&a.token],
                );
                window.add(&mut ac, window.roots(Some(a.token), Some(task.id)));
                let mut token = Message::default();
                token.message("window_container", 1, ac);
                let mut activity = Message::default();
                activity.string("name", 1, &name);
                activity.message("window_token", 2, token);
                activity.int("visible", 24, i32::from(shown[&a.token]));
                activity.string("state", 27, &format!("{:?}", a.state).to_uppercase());
                activity.int("front_of_task", 28, i32::from(Some(a.token) == root));
                activity.int("translucent", 30, i32::from(!a.occludes_parent));
                if let Some((_, orientation, seq, resize)) = metadata {
                    activity.int("override_orientation", 37, *orientation);
                    if let Some(pid) = pids.get(seq).filter(|p| **p > 0) {
                        activity.int("proc_id", 29, *pid);
                    }
                    if Some(a.token) == root {
                        t.int("resize_mode", 18, *resize);
                    }
                }
                wc.child(6, activity);
            }
            window.add(&mut wc, window.roots(None, Some(task.id)));
            let mut fragment = Message::default();
            fragment.message("window_container", 1, wc);
            fragment.int("display_id", 2, 0);
            fragment.int("activity_type", 3, config.window.activity_type);
            if let Some((w, h)) = min_sizes.get(&task.id) {
                fragment.int("min_width", 4, *w);
                fragment.int("min_height", 5, *h);
            }
            t.message("task_fragment", 31, fragment);
            area.child(5, t);
        }
        window.add(&mut area, window.roots(None, None));
        let mut task_area = Message::default();
        task_area.message("window_container", 1, area);
        task_area.string("name", 2, "DefaultTaskDisplayArea");
        task_area.int("is_task_display_area", 4, 1);
        task_area.int("feature_id", 6, 1); // Same native default container as TaskInfo.
        let mut display_wc = container("Display 0", None, None, global, true);
        display_wc.child(4, task_area);
        let mut display_area = Message::default();
        display_area.message("window_container", 1, display_wc);
        display_area.int("is_root_display_area", 5, 1);
        let mut display = Message::default();
        display.message("root_display_area", 21, display_area);
        display.int("id", 2, 0);
        display.int("dpi", 9, global.density_dpi);
        let mut info = Message::default();
        info.int("logical_width", 1, global.window.bounds.width());
        info.int("logical_height", 2, global.window.bounds.height());
        if let Some(bounds) = global.window.app_bounds {
            info.int("app_width", 3, bounds.width());
            info.int("app_height", 4, bounds.height());
        }
        display.message("display_info", 10, info);
        display.int("focused_root_task_id", 23, focused.unwrap_or(-1));
        let front = focused.and_then(|id| {
            activities
                .iter()
                .rev()
                .find(|a| a.task == id && !a.finishing)
        });
        let mut out = Message::default();
        if let Some(a) = front {
            let name = short_component(&a.component);
            out.string("focused_app", 4, &name);
            display.string("focused_app", 15, &name);
            if a.state == State::Resumed {
                display.message(
                    "resumed_activity",
                    24,
                    identifier(Some(a.token as i32), Some(a.user), &name),
                );
            }
        }
        if let Some(w) = focus.and_then(|id| windows.values().find(|w| w.id == id)) {
            out.message(
                "focused_window",
                3,
                identifier(
                    Some(w.id as i32),
                    Some(w.uid / super::super::PER_USER_RANGE),
                    &w.name,
                ),
            );
            display.message(
                "current_focus_identifier",
                43,
                identifier(
                    Some(w.id as i32),
                    Some(w.uid / super::super::PER_USER_RANGE),
                    &w.name,
                ),
            );
        }
        drop(windows);
        let mut rotation = Message::default();
        rotation.int("rotation", 1, global.window.display_rotation);
        rotation.int(
            "frozen_to_user_rotation",
            2,
            i32::from(self.platform().ok().is_some_and(|p| {
                p.settings
                    .get(
                        super::super::settings::Table::System,
                        "accelerometer_rotation",
                    )
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(0)
                    == 0
            })),
        );
        display.message("display_rotation", 33, rotation);
        let mut root_wc = container("RootWindowContainer", None, None, global, true);
        root_wc.child(3, display);
        let mut root = Message::default();
        root.message("window_container", 1, root_wc);
        // CTS's legacy decoder dereferences policy even when no Android policy fields exist.
        out.message("policy", 1, Message::default());
        out.message("root_window_container", 2, root);
        out.int("focused_display_id", 9, 0);
        out.int("window_frames_valid", 11, 1); // Layout is synchronous under windows.windows.
        out
    }
}

#[cfg(test)]
#[path = "window_dump_tests.rs"]
mod tests;
