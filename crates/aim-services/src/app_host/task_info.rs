//! `IActivityTaskManager.getTasks` as ActivityTaskManager answers it at
//! `android-16.0.0_r1`: `ActivityTaskManagerService.getTasks` and
//! `RunningTasks.getTasks` choose the tasks of the app host's stack the
//! caller may see, `Task.fillTaskInfo` fills each
//! `ActivityManager.RunningTaskInfo` with what the app host knows of the
//! task and the original's values otherwise (picture-in-picture, app
//! compat and desktop windowing state: #1589), and it is written as
//! `RunningTaskInfo.writeToParcel` and `TaskInfo.writeTaskToParcel` write
//! it. Also `getAppTasks` (`RecentTasks.getAppTasksList`), whose
//! `IAppTask`s answer `getTaskInfo` with `createRecentTaskInfo`; the
//! recents list is the app host's tasks, the most recently active first,
//! and the other `IAppTask` methods and `getRecentTasks` are not
//! implemented (#1588).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iapptask as iat;

use super::activity::{ACTIVITY_TYPE_STANDARD, DEFAULT_DISPLAY, EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN, INVALID_DISPLAY, Token, USER_CURRENT};
use super::activity_stack::{Stack, Visibility};
use super::activity_start::{Component, LaunchIntent};
use super::config::{Configuration, Rect};
use super::settings::Table;
use super::{AppHost, PER_USER_RANGE};
use crate::package::info::{ActivityInfo, ApplicationInfo};

/// `ActivityTaskManager.INVALID_TASK_ID`.
const INVALID_TASK_ID: i32 = -1;
/// `ActivityInfo.RESIZE_MODE_*` `isResizeableMode` takes as resizeable;
/// `FLAG_SUPPORTS_PICTURE_IN_PICTURE`.
const RESIZEABLE_MODES: [i32; 6] = [1, 2, 4, 5, 6, 7];
const FLAG_SUPPORTS_PICTURE_IN_PICTURE: i32 = 0x40_0000;
/// `DisplayAreaOrganizer.FEATURE_DEFAULT_TASK_CONTAINER`.
const FEATURE_DEFAULT_TASK_CONTAINER: i32 = 1;
/// `WindowInsets.Type.defaultVisible()`: all but the IME.
const DEFAULT_VISIBLE_TYPES: i32 = !(1 << 3);
/// `TaskInfo.PROPERTY_VALUE_UNSET`.
const PROPERTY_VALUE_UNSET: i32 = -1;
/// `WindowManager.LARGE_SCREEN_SMALLEST_SCREEN_WIDTH_DP`.
const LARGE_SCREEN_SMALLEST_SCREEN_WIDTH_DP: i32 = 600;
/// `Configuration.ORIENTATION_LANDSCAPE`.
const ORIENTATION_LANDSCAPE: i32 = 2;
/// `ActivityInfo.CHECK_MIN_WIDTH_HEIGHT_FOR_MULTI_WINDOW`.
const CHECK_MIN_WIDTH_HEIGHT_FOR_MULTI_WINDOW: i64 = 197654537;

/// `RunningTasks.getTasks` over `stack` for `caller` (`allowed`: by
/// `isGetTasksAllowed`, `cross_user`: with `INTERACT_ACROSS_USERS`): the
/// tasks with a running activity, the caller's own, else of its user (its
/// only profile) or of any with `cross_user`, if allowed, top to bottom,
/// the focused visible one first, then the other visible ones, then the
/// rest, at most `max`; each with its last active time, a visible one's
/// after `now` (`elapsedRealtime`), the most recent first.
/// `filterOnlyVisibleRecents` keeps them all: they are standard tasks,
/// neither pinned nor locked, on a display that shows its tasks in
/// recents (`RecentTasks.isVisibleRecentTask`).
pub fn running_tasks(stack: &Stack, max: i32, caller: u32, allowed: bool, cross_user: bool, now: i64) -> Vec<(i32, i64)> {
    let user = caller / PER_USER_RANGE;
    let seen = |t: &&super::activity_stack::Task| {
        stack.top(t.id).is_some() && (t.uid == caller || ((t.user == user || cross_user) && allowed))
    };
    let (mut visible, invisible): (Vec<_>, Vec<_>) = stack.tasks.iter().rev().filter(seen).partition(|t| stack.visibility(t.id) != Visibility::Invisible);
    if let Some(at) = visible.iter().position(|t| Some(t.id) == stack.focused()) {
        let focused = visible.remove(at);
        visible.insert(0, focused);
    }
    let size = (max.max(0) as usize).min(visible.len() + invisible.len());
    let visible_count = visible.len();
    visible
        .into_iter()
        .chain(invisible)
        .take(size)
        .enumerate()
        .map(|(i, t)| (t.id, if i < visible_count { now + (size - i) as i64 } else { t.last_active }))
        .collect()
}

/// `ActivityManager.RunningTaskInfo`.
pub struct RunningTaskInfo {
    pub user_id: i32,
    pub task_id: i32,
    pub effective_uid: i32,
    pub base_intent: BaseIntent,
    pub base_activity: Option<(String, String)>,
    pub top_activity: Option<(String, String)>,
    pub orig_activity: Option<(String, String)>,
    pub real_activity: Option<(String, String)>,
    pub num_activities: i32,
    pub last_active_time: i64,
    pub supports_multi_window: bool,
    pub resize_mode: i32,
    pub configuration: Configuration,
    /// Its `WindowContainerToken`.
    pub token: Binder,
    pub top_activity_info: Option<ActivityInfo>,
    pub is_resizeable: bool,
    pub min_width: i32,
    pub min_height: i32,
    pub default_min_size: i32,
    pub position_in_parent: (i32, i32),
    pub is_focused: bool,
    pub is_visible: bool,
    pub is_top_activity_no_display: bool,
    pub is_top_activity_transparent: bool,
    pub is_activity_stack_transparent: bool,
    pub last_non_fullscreen_bounds: Option<Rect>,
    /// `AppCompatTaskInfo.topActivityAppBounds`.
    pub top_activity_app_bounds: Rect,
}

/// A task's base intent, as `fillTaskInfo` copies it: whole, or its
/// filter with its flags (`cloneFilter`) unless the extras are kept.
pub enum BaseIntent {
    /// The launcher's intent of a component, with its flags.
    Component { component: (String, String), flags: i32, strip: bool },
    Raw(Box<super::kept::RawIntent>),
}

impl BaseIntent {
    /// `task`'s, stripped (`cloneFilter`, its flags kept) if `strip`.
    fn of(task: &super::activity_stack::Task, strip: bool) -> Result<BaseIntent, String> {
        Ok(match &task.base {
            LaunchIntent::Component => {
                let component = task.intent.component.as_ref().map(|c| (c.package.clone(), c.class.clone())).ok_or("no component")?;
                BaseIntent::Component { component, flags: task.intent.flags, strip }
            }
            LaunchIntent::Raw(kept) => {
                let intent = &kept.intent;
                BaseIntent::Raw(Box::new(if strip {
                    let mut filter = intent.clone_filter().map_err(|e| format!("{e:?}"))?;
                    filter.set_flags(intent.intent.flags, 0);
                    filter
                } else {
                    intent.for_task().map_err(|e| format!("{e:?}"))?
                }))
            }
        })
    }
}

impl WriteParcelable for BaseIntent {
    fn write_to(&self, p: &mut Parcel) {
        match self {
            BaseIntent::Component { component, flags, strip } => super::activity::Intent {
                component: component.clone(),
                flags: *flags,
                extended_flags: if *strip { 0 } else { EXTENDED_FLAG_MISSING_CREATOR_OR_INVALID_TOKEN },
                content_user_hint: USER_CURRENT,
            }
            .write_to(p),
            BaseIntent::Raw(raw) => raw.write_to(p),
        }
    }
}

/// `ComponentName.writeToParcel(c, out)`: null as a null string.
fn write_component(p: &mut Parcel, c: &Option<(String, String)>) {
    match c {
        Some((package, class)) => Component(package, class).write_to(p),
        None => p.write_string16(None),
    }
}

/// `writeTypedObject` of a `Rect`.
fn write_rect(p: &mut Parcel, r: Option<&Rect>) {
    match r {
        Some(r) => {
            p.write_i32(1);
            [r.left, r.top, r.right, r.bottom].iter().for_each(|v| p.write_i32(*v));
        }
        None => p.write_i32(0),
    }
}

impl WriteParcelable for RunningTaskInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.task_id); // RunningTaskInfo.id
        self.write_task(p);
    }
}

/// `ActivityManager.RecentTaskInfo` of a running task: its `id` and
/// `persistentId` are its task id, then the `TaskInfo`.
pub struct RecentTaskInfo(RunningTaskInfo);

impl WriteParcelable for RecentTaskInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.0.task_id);
        p.write_i32(self.0.task_id);
        self.0.write_task(p);
    }
}

impl RunningTaskInfo {
    /// `TaskInfo.writeTaskToParcel`.
    fn write_task(&self, p: &mut Parcel) {
        p.write_i32(self.user_id);
        p.write_i32(self.task_id);
        p.write_i32(self.effective_uid);
        p.write_i32(DEFAULT_DISPLAY);
        p.write_bool(true); // isRunning: it has a running activity
        p.write_i32(1);
        self.base_intent.write_to(p);
        for c in [&self.base_activity, &self.top_activity, &self.orig_activity, &self.real_activity] {
            write_component(p, c);
        }
        p.write_i32(self.num_activities);
        p.write_i64(self.last_active_time);
        // `TaskDescription`: the defaults, as no activity of the app host
        // sets one (#1422), with the task's resize mode and minimal size.
        p.write_i32(1);
        p.write_i32(0); // no label
        p.write_i32(0); // no icon
        [0; 6].iter().for_each(|v| p.write_i32(*v)); // colors and bar appearances
        p.write_bool(false); // no status bar contrast
        p.write_bool(false); // no navigation bar contrast
        p.write_i32(self.resize_mode);
        p.write_i32(self.min_width);
        p.write_i32(self.min_height);
        p.write_i32(0); // no icon file
        p.write_i32(0); // floating background color
        p.write_bool(self.supports_multi_window);
        p.write_i32(self.resize_mode);
        self.configuration.write_to(p);
        p.write_binder(Some(self.token));
        p.write_i32(ACTIVITY_TYPE_STANDARD);
        p.write_i32(0); // no PictureInPictureParams
        p.write_bool(false); // shouldDockBigOverlays
        p.write_i32(INVALID_TASK_ID); // launchIntoPipHostTaskId
        p.write_i32(INVALID_TASK_ID); // lastParentTaskIdBeforePip
        write_rect(p, Some(&Rect::default())); // no display cutout
        match &self.top_activity_info {
            Some(info) => {
                p.write_i32(1);
                info.write_to(p);
            }
            None => p.write_i32(0),
        }
        p.write_bool(self.is_resizeable);
        p.write_i32(self.min_width);
        p.write_i32(self.min_height);
        p.write_i32(self.default_min_size);
        p.write_i32(0); // no launch cookies (`writeBinderList`)
        p.write_i32(1);
        p.write_i32(self.position_in_parent.0);
        p.write_i32(self.position_in_parent.1);
        p.write_i32(INVALID_TASK_ID); // parentTaskId: no organizer's
        p.write_bool(self.is_focused);
        p.write_bool(self.is_visible);
        p.write_bool(self.is_visible); // isVisibleRequested
        p.write_bool(self.is_top_activity_no_display);
        p.write_bool(false); // isSleeping: the display never sleeps
        p.write_i32(0); // no LocusId
        p.write_i32(FEATURE_DEFAULT_TASK_CONTAINER);
        p.write_bool(self.is_top_activity_transparent);
        p.write_bool(self.is_activity_stack_transparent);
        write_rect(p, self.last_non_fullscreen_bounds.as_ref());
        p.write_i32(0); // no captured link: no desktop windowing
        p.write_i64(0);
        p.write_i32(DEFAULT_VISIBLE_TYPES);
        p.write_i64(0); // topActivityRequestOpenInBrowserEducationTimestamp
        // `AppCompatTaskInfo`: no letterboxing, size compat or override.
        p.write_i32(1);
        p.write_i32(0); // top activity flags
        [PROPERTY_VALUE_UNSET; 4].iter().for_each(|v| p.write_i32(*v));
        write_rect(p, Some(&self.top_activity_app_bounds));
        write_rect(p, None); // no letterbox bounds
        p.write_i32(1);
        p.write_i32(0); // CAMERA_COMPAT_FREEFORM_UNSPECIFIED
        write_rect(p, None); // no top activity main window frame
    }
}

/// `ActivityTaskManagerService`'s multi-window support (`retrieveSettings`)
/// and the display area's (`TaskDisplayArea.supportsNonResizableMultiWindow`
/// and `supportsActivityMinWidthHeightMultiWindow`).
struct MultiWindow {
    supported: bool,
    force_resizable: bool,
    dev_non_resizable: bool,
    non_resizable: i32,
    respects_min_size: i32,
    min_percentage: (f32, f32),
}

impl MultiWindow {
    /// `Task.supportsMultiWindowInDisplayArea` of a task (`resizeable`, of
    /// a minimal size `min`, its root activity's `checks_min_size`) on a
    /// display of `display`.
    fn supports(&self, resizeable: bool, min: (i32, i32), checks_min_size: bool, display: &Configuration) -> bool {
        let large = display.smallest_screen_width_dp >= LARGE_SCREEN_SMALLEST_SCREEN_WIDTH_DP;
        let non_resizable = self.dev_non_resizable || self.non_resizable == 1 || (self.non_resizable != -1 && large);
        if !self.supported || (!resizeable && !non_resizable) {
            return false;
        }
        if !checks_min_size || (min.0 <= 0 && min.1 <= 0) || self.respects_min_size == -1 || (self.respects_min_size == 0 && large) {
            return true;
        }
        let density = display.density_dpi as f32 / 160.0;
        if display.orientation == ORIENTATION_LANDSCAPE {
            min.0 <= (self.min_percentage.1 * display.screen_width_dp as f32 * density) as i32
        } else {
            min.1 <= (self.min_percentage.0 * display.screen_height_dp as f32 * density) as i32
        }
    }
}

impl AppHost {
    /// `getTasks` by `uid` of at most `max` tasks on display `display_id`
    /// (`INVALID_DISPLAY` for all), their base intents' extras kept if
    /// `keep_intent_extra`.
    pub(super) fn get_tasks(&self, uid: i32, max: i32, keep_intent_extra: bool, display_id: i32) -> Result<Vec<Option<RunningTaskInfo>>, Exception> {
        let has = |p: &str| self.check_permission(Some(p), uid).map(|r| r == 0);
        let cross_user = has("android.permission.INTERACT_ACROSS_USERS")? || has("android.permission.INTERACT_ACROSS_USERS_FULL")?;
        let allowed = self.get_tasks_allowed(uid)?;
        // `getRunningTasks`: the app host has one display.
        if display_id != INVALID_DISPLAY && display_id != DEFAULT_DISPLAY {
            return Ok(Vec::new());
        }
        let chosen = running_tasks(&self.activities.stack.lock().unwrap(), max, uid as u32, allowed, cross_user, super::alarm_service::elapsed_ms());
        if chosen.is_empty() {
            return Ok(Vec::new());
        }
        let multi_window = self.multi_window()?;
        chosen.into_iter().map(|(task, last_active)| self.task_info(task, last_active, !keep_intent_extra, allowed, &multi_window).map(Some)).collect()
    }

    /// `getAppTasks` by `uid` for `package`: the tasks of `RecentTasks
    /// .getAppTasksList`, the most recently active first, each the caller's
    /// `IAppTask`.
    pub(super) fn app_tasks(&self, uid: i32, package: Option<&str>) -> Result<Vec<Binder>, Exception> {
        // `assertPackageMatchesCallingUid`: `isSameApp`, the system's for
        // no package.
        let same = match package {
            Some(p) => self.owns_package(uid, p)?,
            None => uid == 1000,
        };
        if !same {
            return Err(Exception::security(format!(
                "Permission Denial: package={} does not belong to uid={uid}",
                package.unwrap_or("null")
            )));
        }
        let package = package.unwrap_or_default();
        let mut tasks: Vec<(i64, i32)> = {
            let stack = self.activities.stack.lock().unwrap();
            stack
                .tasks
                .iter()
                .filter(|t| t.uid as i32 == uid && t.real_activity.split('/').next() == Some(package))
                .map(|t| (t.last_active, t.id))
                .collect()
        };
        tasks.sort_by(|a, b| b.0.cmp(&a.0));
        let me = self.me.upgrade().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host gone"))?;
        Ok(tasks.into_iter().map(|(_, task)| self.process.add_service(Arc::new(AppTask { host: me.clone(), task, uid }))).collect())
    }

    /// `retrieveSettings`' multi-window support.
    fn multi_window(&self) -> Result<MultiWindow, Exception> {
        let inputs = self.inputs()?;
        let packages = &inputs.packages;
        let global = |name: &str| inputs.settings.get(Table::Global, name).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0) != 0;
        let (state, _) = self.packages()?;
        let feature = |name: &str| state.system.features.iter().any(|(f, _)| f == name);
        let property = |name: &str| super::system_boolean(self.property(name).as_deref(), false);
        // `ActivityManager.isLowRamDeviceStatic`.
        let low_ram = property("ro.config.low_ram") || (property("ro.debuggable") && property("debug.force_low_ram"));
        let config = |name: &str| packages.framework_bool(name).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("no framework bool {name}")));
        let integer = |name: &str| packages.framework_integer(name).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("no framework integer {name}")));
        let float = |name: &str| packages.framework_float(name).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("no framework float {name}")));
        // `ActivityTaskManager.supportsMultiWindow`, `supportsSplitScreenMultiWindow`.
        let supports = (!low_ram || feature("android.hardware.type.watch")) && config("config_supportsMultiWindow")?;
        let split_screen = supports && config("config_supportsSplitScreenMultiWindow")?;
        let freeform = feature("android.software.freeform_window_management") || global("enable_freeform_support");
        let pip = supports && feature("android.software.picture_in_picture");
        let multi_display = feature("android.software.activities_on_secondary_displays");
        let force_resizable = global("force_resizable_activities");
        Ok(MultiWindow {
            supported: (supports || force_resizable) && (freeform || split_screen || pip || multi_display),
            force_resizable,
            dev_non_resizable: global("enable_non_resizable_multi_window"),
            non_resizable: integer("config_supportsNonResizableMultiWindow")?,
            respects_min_size: integer("config_respectsActivityMinWidthHeightMultiWindow")?,
            min_percentage: (float("config_minPercentageMultiWindowSupportHeight")?, float("config_minPercentageMultiWindowSupportWidth")?),
        })
    }

    /// `Task.fillTaskInfo` of `task` (`RunningTasks.createRunningTaskInfo`):
    /// its last active time `last_active`, its base intent's extras
    /// stripped if `strip`, what a caller not `allowed` may not see of
    /// another uid's activities trimmed (`trimIneffectiveInfo`).
    fn task_info(&self, task: i32, last_active: i64, strip: bool, allowed: bool, multi_window: &MultiWindow) -> Result<RunningTaskInfo, Exception> {
        let gone = || Exception::new(EX_ILLEGAL_STATE, format!("no task {task}"));
        let split = |c: &str| c.split_once('/').map(|(p, c)| (p.to_string(), c.to_string()));
        let (t, running, top, focused, visibility, translucent, top_occludes) = {
            let stack = self.activities.stack.lock().unwrap();
            let t = stack.task(task).ok_or_else(gone)?.clone();
            let running: Vec<u64> = stack.activities.iter().filter(|a| a.task == task && !a.finishing).map(|a| a.token).collect();
            let top = stack.top(task).ok_or_else(gone)?;
            (t, running, top.token, stack.focused() == Some(task), stack.visibility(task), stack.translucent(task), top.occludes_parent)
        };
        let base_token = *running.first().ok_or_else(gone)?;
        let (top_info, top_uid, top_no_display, base_uid, root_info) = {
            let records = self.activities.records.lock().unwrap();
            let record = |token: u64| records.get(&token).ok_or_else(gone);
            let (top, base) = (record(top)?, record(base_token)?);
            (top.info.clone(), top.uid, top.style.no_display, base.uid, base.info.clone())
        };
        let top_component = super::activity_start::real_component(&top_info);
        let base_activity = super::activity_start::real_component(&root_info);
        let global = self.global_config(top_uid).or_else(|| self.global_config(base_uid)).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("task {task} of no running process")))?;
        let bounds = self.tasks.freeform_bounds(task);
        let mut configuration = match bounds {
            Some(bounds) => global.of_freeform_task(bounds),
            None => global.clone(),
        };
        configuration.window.activity_type = ACTIVITY_TYPE_STANDARD;
        let resize_mode = root_info.resize_mode;
        let min_size = self.tasks.min_size(task).ok_or_else(gone)?;
        let force_resizable = multi_window.force_resizable;
        let is_resizeable = force_resizable || RESIZEABLE_MODES.contains(&resize_mode) || root_info.flags & FLAG_SUPPORTS_PICTURE_IN_PICTURE != 0;
        let package = root_info.info.application_info.item.package_name.as_deref();
        let checks_min_size = self.inputs()?.compat.is_enabled(CHECK_MIN_WIDTH_HEIGHT_FOR_MULTI_WINDOW, package, root_info.info.application_info.target_sdk_version);
        let default_min_size = self.inputs()?.packages.framework_dimension_dp("default_minimal_size_resizable_task").ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no default_minimal_size_resizable_task"))? as i32;
        let base_intent = BaseIntent::of(&t, strip).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("the base intent of task {task}: {e}")))?;
        let intent_component = t.intent.component.as_ref().map(|c| format!("{}/{}", c.package, c.class));
        let mut info = RunningTaskInfo {
            user_id: t.user as i32,
            task_id: task,
            effective_uid: t.uid as i32,
            base_intent,
            base_activity: split(&base_activity),
            top_activity: split(&top_component),
            orig_activity: intent_component.filter(|c| *c != t.real_activity).as_deref().and_then(split),
            real_activity: split(&t.real_activity),
            num_activities: running.len() as i32,
            last_active_time: last_active,
            supports_multi_window: multi_window.supports(is_resizeable, min_size, checks_min_size, &global),
            resize_mode,
            configuration,
            token: self.task_token(task),
            top_activity_info: Some(top_info),
            is_resizeable,
            min_width: min_size.0,
            min_height: min_size.1,
            default_min_size,
            position_in_parent: bounds.map_or((0, 0), |b| (b.left, b.top)),
            is_focused: focused,
            is_visible: visibility != Visibility::Invisible,
            is_top_activity_no_display: top_no_display,
            is_top_activity_transparent: !top_occludes,
            is_activity_stack_transparent: translucent,
            last_non_fullscreen_bounds: bounds,
            top_activity_app_bounds: global.window.app_bounds.unwrap_or(global.window.bounds),
        };
        if !allowed {
            trim_ineffective_info(&mut info, top_uid, base_uid);
        }
        Ok(info)
    }

    /// The `WindowContainerToken` of `task`, made at its first use.
    fn task_token(&self, task: i32) -> Binder {
        *self.activities.task_tokens.lock().unwrap().entry(task).or_insert_with(|| self.process.add_service(Arc::new(Token)))
    }
}

/// `Task.trimIneffectiveInfo` of `info`, of a task whose top activity's uid
/// is `top_uid` and base activity's `base_uid`: another uid's activities
/// are not named.
fn trim_ineffective_info(info: &mut RunningTaskInfo, top_uid: u32, base_uid: u32) {
    let empty = || Some((String::new(), String::new()));
    if top_uid as i32 != info.effective_uid
        && let Some(top) = info.top_activity_info.as_mut()
    {
        info.top_activity = empty();
        let blank = Some(String::new());
        top.info.item.package_name = blank.clone();
        top.task_affinity = blank.clone();
        top.info.process_name = blank.clone();
        top.info.item.name = blank.clone();
        top.parent_activity_name = blank.clone();
        top.target_activity = blank.clone();
        top.info.split_name = blank.clone();
        let mut app: ApplicationInfo = (*top.info.application_info).clone();
        for field in [
            &mut app.class_name,
            &mut app.credential_protected_data_dir,
            &mut app.data_dir,
            &mut app.device_protected_data_dir,
            &mut app.manage_space_activity_name,
            &mut app.native_library_dir,
            &mut app.native_library_root_dir,
            &mut app.process_name,
            &mut app.public_source_dir,
            &mut app.scan_public_source_dir,
            &mut app.scan_source_dir,
            &mut app.source_dir,
            &mut app.task_affinity,
            &mut app.item.name,
            &mut app.item.package_name,
        ] {
            *field = blank.clone();
        }
        top.info.application_info = Arc::new(app);
    }
    if base_uid as i32 != info.effective_uid {
        info.base_activity = empty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_host::activity_stack::{Activity, Task};
    use crate::app_host::config::Device;
    use crate::app_host::display::Display;
    use crate::package::intent::{ComponentName, Intent};
    use aim_binder_host::parcel::Reader;
    use aim_service_aidl::ReadParcelable;

    /// Task 7 of uid 10100 behind task 8 of uid 10200, each of one
    /// activity, last active at 70 and 80.
    fn stack(freeform: bool) -> Stack {
        let mut s = Stack::default();
        s.freeform = freeform;
        for (id, uid, token) in [(7, 10_100, 1), (8, 10_200, 2)] {
            let intent = Intent::default();
            s.tasks.push(Task { id, affinity: None, real_activity: format!("p{id}/A"), intent, base: LaunchIntent::Component, uid, user: 0, last_active: id as i64 * 10 });
            s.activities.push(Activity::new(token, id, 1, "p/A", 0, 0, None));
        }
        s
    }

    #[test]
    fn the_caller_sees_its_own_tasks_unless_allowed_the_visible_ones_first() {
        let s = stack(false);
        // Its own, hidden behind the other: its own last active time.
        assert_eq!(running_tasks(&s, 10, 10_100, false, false, 1000), [(7, 70)]);
        // All: the visible one's time after now.
        assert_eq!(running_tasks(&s, 10, 10_100, true, false, 1000), [(8, 1002), (7, 70)]);
        assert_eq!(running_tasks(&s, 1, 10_100, true, false, 1000), [(8, 1001)]);
        assert_eq!(running_tasks(&s, 0, 10_100, true, false, 1000), []);
        // In window mode both are visible, the focused one first.
        assert_eq!(running_tasks(&stack(true), 10, 10_100, true, false, 1000), [(8, 1002), (7, 1001)]);
        // Another user's tasks need INTERACT_ACROSS_USERS.
        let other = 10 * PER_USER_RANGE + 10_100;
        assert_eq!(running_tasks(&s, 10, other, true, false, 1000), []);
        assert_eq!(running_tasks(&s, 10, other, true, true, 1000).len(), 2);
        // A task without a running activity is not running.
        let mut s = stack(false);
        s.activities[1].finishing = true;
        assert_eq!(running_tasks(&s, 10, 10_100, true, false, 1000), [(7, 1001)]);
    }

    /// The parcel, read back in `TaskInfo.readTaskFromParcel`'s order.
    #[test]
    fn a_running_task_info_is_written_as_the_original_reads_it() {
        let device = Device { locales: "en-US", right_to_left: false, night: false, font_scale: 1.0, font_weight_adjustment: 0, seq: 1 };
        let bounds = Rect { left: 10, top: 20, right: 830, bottom: 1480 };
        let configuration = Configuration::of(&Display::phone_for_test(), &device).of_freeform_task(bounds);
        let task = Task {
            id: 9,
            affinity: None,
            real_activity: "p/p.Main".into(),
            intent: Intent { flags: 0x1000_0000, component: Some(ComponentName { package: "p".into(), class: "p.Main".into() }), ..Default::default() },
            base: LaunchIntent::Component,
            uid: 10_100,
            user: 0,
            last_active: 0,
        };
        let info = RunningTaskInfo {
            user_id: 0,
            task_id: 9,
            effective_uid: 10_100,
            base_intent: BaseIntent::of(&task, true).unwrap(),
            base_activity: Some(("p".into(), "p.Main".into())),
            top_activity: Some(("p".into(), "p.Top".into())),
            orig_activity: None,
            real_activity: Some(("p".into(), "p.Main".into())),
            num_activities: 2,
            last_active_time: 1234,
            supports_multi_window: true,
            resize_mode: 2,
            configuration: configuration.clone(),
            token: Binder::Local(0x40),
            top_activity_info: None,
            is_resizeable: true,
            min_width: -1,
            min_height: -1,
            default_min_size: 220,
            position_in_parent: (10, 20),
            is_focused: true,
            is_visible: true,
            is_top_activity_no_display: false,
            is_top_activity_transparent: true,
            is_activity_stack_transparent: false,
            last_non_fullscreen_bounds: Some(bounds),
            top_activity_app_bounds: bounds,
        };
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let mut r = Reader::new(p.data(), p.objects());
        let ints = |r: &mut Reader<'_>, n: usize| (0..n).map(|_| r.read_i32().unwrap()).collect::<Vec<_>>();
        let rect = |r: &mut Reader<'_>| (r.read_i32().unwrap() != 0).then(|| [0; 4].map(|_| r.read_i32().unwrap()));
        let component = |r: &mut Reader<'_>| r.read_string16().unwrap().map(|p| (p, r.read_string16().unwrap().unwrap()));
        // RunningTaskInfo.id, userId, taskId, effectiveUid, displayId,
        // isRunning, a base intent.
        assert_eq!(ints(&mut r, 7), [9, 0, 9, 10_100, 0, 1, 1]);
        let base = crate::app_host::kept::RawIntent::read_from(&mut r).unwrap();
        // The filter of the launcher's intent, with its flags.
        assert_eq!((base.intent.flags, base.intent.extended_flags, base.intent.component.map(|c| c.class)), (0x1000_0000, 0, Some("p.Main".into())));
        assert_eq!(component(&mut r), Some(("p".into(), "p.Main".into())));
        assert_eq!(component(&mut r), Some(("p".into(), "p.Top".into())));
        assert_eq!(component(&mut r), None);
        assert_eq!(component(&mut r), Some(("p".into(), "p.Main".into())));
        assert_eq!((r.read_i32().unwrap(), r.read_i64().unwrap()), (2, 1234));
        // TaskDescription: no label or icon, its colors and bars, its
        // resize mode and minimal size, no icon file, its floating color.
        assert_eq!(ints(&mut r, 3), [1, 0, 0]);
        r.skip(6 * 4 + 2 * 4).unwrap();
        assert_eq!(ints(&mut r, 5), [2, -1, -1, 0, 0]);
        // supportsMultiWindow, resizeMode, the configuration, the token.
        assert_eq!(ints(&mut r, 2), [1, 2]);
        let mut config = Parcel::new();
        configuration.write_to(&mut config);
        assert_eq!(&p.data()[r.position()..r.position() + config.data().len()], config.data());
        r.skip(config.data().len()).unwrap();
        assert_eq!(r.read_binder().unwrap(), Some(Binder::Local(0x40)));
        // topActivityType, no PiP params, no big overlays, no PiP tasks,
        // the cutout's insets, no topActivityInfo, isResizeable, the
        // minimal sizes, no launch cookies, positionInParent, parentTaskId.
        assert_eq!(ints(&mut r, 5), [ACTIVITY_TYPE_STANDARD, 0, 0, -1, -1]);
        assert_eq!(rect(&mut r), Some([0; 4]));
        assert_eq!(ints(&mut r, 9), [0, 1, -1, -1, 220, 0, 1, 10, 20]);
        assert_eq!(r.read_i32().unwrap(), -1);
        // isFocused, isVisible, isVisibleRequested, no display, sleeping;
        // no LocusId, the feature, the transparencies, the bounds.
        assert_eq!(ints(&mut r, 9), [1, 1, 1, 0, 0, 0, FEATURE_DEFAULT_TASK_CONTAINER, 1, 0]);
        assert_eq!(rect(&mut r), Some([10, 20, 830, 1480]));
        assert_eq!((r.read_i32().unwrap(), r.read_i64().unwrap(), r.read_i32().unwrap(), r.read_i64().unwrap()), (0, 0, !8, 0));
        // AppCompatTaskInfo.
        assert_eq!(ints(&mut r, 6), [1, 0, -1, -1, -1, -1]);
        assert_eq!((rect(&mut r), rect(&mut r)), (Some([10, 20, 830, 1480]), None));
        assert_eq!(ints(&mut r, 2), [1, 0]);
        assert_eq!(rect(&mut r), None); // topActivityMainWindowFrame
        assert_eq!(r.remaining(), 0);
    }
}

/// `AppTaskImpl`: a task of `getAppTasks`, for its caller `uid`.
struct AppTask {
    host: Arc<AppHost>,
    task: i32,
    uid: i32,
}

impl Service for AppTask {
    fn descriptor(&self) -> &str {
        iat::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `checkCallerOrSystemOrRoot`.
        let caller = call.sender_euid as i32;
        if caller != self.uid && caller != 1000 && caller != 0 {
            return super::reply(Err(Exception::security(format!(
                "Caller {} does not match caller of getAppTasks(): {caller}",
                self.uid
            ))));
        }
        match call.code {
            // `createRecentTaskInfo(task, stripExtras false, getTasksAllowed
            // true)`.
            iat::GET_TASK_INFO => {
                iat::GetTaskInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let host = &self.host;
                let info = (|| {
                    let last_active = {
                        let stack = host.activities.stack.lock().unwrap();
                        stack.task(self.task).map(|t| t.last_active)
                    };
                    let last_active = last_active.ok_or_else(|| Exception::illegal_argument(format!("Unable to find task ID {}", self.task)))?;
                    host.task_info(self.task, last_active, false, true, &host.multi_window()?)
                })();
                super::reply(info.map(|info| {
                    let mut p = Parcel::new();
                    iat::write_get_task_info_reply(&mut p, Some(&RecentTaskInfo(info)));
                    p
                }))
            }
            code => super::reply(Err(self.host.unimplemented("app_task", code))),
        }
    }
}
