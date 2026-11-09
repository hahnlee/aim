//! `IActivityTaskManager`'s root-task queries as ActivityTaskManager answers
//! them at `android-16.0.0_r1`: `getAllRootTaskInfos`,
//! `getAllRootTaskInfosOnDisplay`, `getRootTaskInfo`,
//! `getRootTaskInfoOnDisplay` and `getFocusedRootTaskInfo` (`ActivityTaskManagerService`, behind
//! `enforceTaskPermission`; `RootWindowContainer.getRootTaskInfo` and
//! `TaskDisplayArea.getRootTask`) over the app host's root tasks: the root
//! home task, the organizer root tasks with their tasks and the other app
//! tasks, each written as `ActivityTaskManager.RootTaskInfo.writeToParcel`
//! writes it.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::local::{Call, Reply};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iactivitytaskmanager as atm;

use super::activity::{
    ACTIVITY_TYPE_HOME, ACTIVITY_TYPE_STANDARD, DEFAULT_DISPLAY, INVALID_DISPLAY,
};
use super::activity_stack::Visibility;
use super::config::Rect;
use super::task_info::{RunningTaskInfo, write_rect};
use super::{AppHost, reply};

/// `WindowConfiguration.WINDOWING_MODE_PINNED`.
const WINDOWING_MODE_PINNED: i32 = 2;
/// `WindowConfiguration.ACTIVITY_TYPE_UNDEFINED`.
const ACTIVITY_TYPE_UNDEFINED: i32 = 0;

/// `ActivityTaskManager.RootTaskInfo`: `Task.fillTaskInfo` of the root
/// task, its bounds being its configuration's.
pub struct RootTaskInfo {
    pub task: RunningTaskInfo,
    /// Its leaf tasks bottom to top (`forAllLeafTasks`): itself if it
    /// holds no task.
    pub leaves: Vec<LeafTask>,
    /// `shouldBeVisible`: by the stack's visibility policy.
    pub visible: bool,
    /// Its index among the display's root tasks, bottom to top.
    pub position: i32,
}

/// What `getRootTaskInfo` reports of a leaf task.
pub struct LeafTask {
    pub id: i32,
    /// Its original or real activity, else its top activity's package.
    pub name: String,
    /// `getTaskBounds`.
    pub bounds: Rect,
    pub user_id: i32,
}

impl LeafTask {
    fn of(info: &RunningTaskInfo) -> LeafTask {
        let name = info
            .orig_activity
            .as_ref()
            .or(info.real_activity.as_ref())
            .map(|(package, class)| format!("{package}/{class}"))
            .or_else(|| info.top_activity.as_ref().map(|(p, _)| p.clone()))
            .unwrap_or_else(|| "unknown".into());
        LeafTask {
            id: info.task_id,
            name,
            bounds: info.configuration.window.bounds,
            user_id: info.user_id,
        }
    }
}

impl WriteParcelable for RootTaskInfo {
    fn write_to(&self, p: &mut Parcel) {
        write_rect(p, Some(&self.task.configuration.window.bounds));
        let count = self.leaves.len() as i32;
        p.write_i32(count);
        self.leaves.iter().for_each(|l| p.write_i32(l.id));
        p.write_i32(count);
        self.leaves
            .iter()
            .for_each(|l| p.write_string16(Some(&l.name)));
        p.write_i32(count);
        self.leaves
            .iter()
            .for_each(|l| write_rect(p, Some(&l.bounds)));
        p.write_i32(count);
        self.leaves.iter().for_each(|l| p.write_i32(l.user_id));
        p.write_i32(self.visible as i32);
        p.write_i32(self.position);
        self.task.write_task(p);
    }
}

/// `TaskDisplayArea.getRootTask`'s match of a root task in windowing mode
/// `this_mode` of activity type `this_kind` for `mode` and `kind`
/// (`Task.isCompatible`, `ConfigurationContainer.isCompatible`).
fn compatible((this_mode, this_kind): (i32, i32), mode: i32, kind: i32) -> bool {
    // An undefined type asks for the topmost root task of the mode.
    if kind == ACTIVITY_TYPE_UNDEFINED && mode == this_mode {
        return true;
    }
    // Undefined types end up standard.
    let kind = if kind == ACTIVITY_TYPE_UNDEFINED {
        ACTIVITY_TYPE_STANDARD
    } else {
        kind
    };
    if kind == this_kind && mode == this_mode {
        return true;
    }
    if kind != ACTIVITY_TYPE_STANDARD
        || (this_kind != ACTIVITY_TYPE_STANDARD && this_kind != ACTIVITY_TYPE_UNDEFINED)
    {
        return kind == this_kind;
    }
    mode == this_mode
}

impl AppHost {
    /// `ActivityTaskManagerService.enforceTaskPermission` of `func` for a
    /// caller `uid` in process `pid`.
    fn enforce_task_permission(&self, uid: i32, pid: i32, func: &str) -> Result<(), Exception> {
        for permission in [
            "android.permission.MANAGE_ACTIVITY_TASKS",
            "android.permission.MANAGE_ACTIVITY_STACKS",
        ] {
            if self.check_permission(Some(permission), uid)? == 0 {
                return Ok(());
            }
        }
        Err(Exception::security(format!(
            "Permission Denial: {func} from pid={pid}, uid={uid} requires \
             android.permission.MANAGE_ACTIVITY_TASKS"
        )))
    }

    /// The root tasks top to bottom, those being removed left out: the
    /// snapshot a query's positions follow.
    fn live_root_tasks(&self) -> Vec<i32> {
        self.tasks
            .root_task_ids()
            .into_iter()
            .filter(|&id| !self.task_gone(id))
            .collect()
    }

    /// `RootWindowContainer.getAllRootTaskInfos(display)`: the root tasks
    /// top to bottom, of every display for `INVALID_DISPLAY`, none of an
    /// unknown display. One that goes meanwhile is left out.
    pub(super) fn all_root_task_infos(&self, display: i32) -> Result<Vec<RootTaskInfo>, Exception> {
        if display != INVALID_DISPLAY && display != DEFAULT_DISPLAY {
            return Ok(Vec::new());
        }
        let mut infos = Vec::new();
        for id in self.live_root_tasks() {
            infos.extend(self.root_task_info_at(id)?);
        }
        let bottom = infos.len() as i32 - 1;
        for (i, info) in infos.iter_mut().enumerate() {
            info.position = bottom - i as i32;
        }
        Ok(infos)
    }

    /// `RootWindowContainer.getRootTaskInfo(windowingMode, activityType,
    /// displayId)`: the root home task for `ACTIVITY_TYPE_HOME`, no pinned
    /// root task (the app host has none, #1589), else the topmost
    /// compatible root task.
    pub(super) fn root_task_info_of(
        &self,
        mode: i32,
        kind: i32,
        display: i32,
    ) -> Result<Option<RootTaskInfo>, Exception> {
        if display != DEFAULT_DISPLAY
            || (kind != ACTIVITY_TYPE_HOME && mode == WINDOWING_MODE_PINNED)
        {
            return Ok(None);
        }
        let global = self.global_configuration()?.window.windowing_mode;
        let ids = self.live_root_tasks();
        for at in 0..ids.len() {
            let this = self.root_task_kind(ids[at], global);
            let found = if kind == ACTIVITY_TYPE_HOME {
                this.1 == ACTIVITY_TYPE_HOME
            } else {
                compatible(this, mode, kind)
            };
            if found && let Some(info) = self.root_task_info_in(&ids, at)? {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    /// `getFocusedRootTaskInfo`: the root task of the focused task
    /// (`getTopDisplayFocusedRootTask`), if any.
    pub(super) fn focused_root_task_info(&self) -> Result<Option<RootTaskInfo>, Exception> {
        let Some(task) = self.activities.stack.lock().unwrap().focused() else {
            return Ok(None);
        };
        let root = self.tasks.parent(task).unwrap_or(task);
        let ids = self.live_root_tasks();
        match ids.iter().position(|&id| id == root) {
            Some(at) => self.root_task_info_in(&ids, at),
            None => Ok(None),
        }
    }

    /// The info of root task `ids[at]` at its position among `ids`.
    fn root_task_info_in(&self, ids: &[i32], at: usize) -> Result<Option<RootTaskInfo>, Exception> {
        Ok(self.root_task_info_at(ids[at])?.map(|mut info| {
            info.position = (ids.len() - 1 - at) as i32;
            info
        }))
    }

    /// The windowing mode and activity type of root task `id`
    /// (`getWindowingMode`, `getActivityType`), `global` the display's
    /// mode: an organizer root task's type is undefined.
    fn root_task_kind(&self, id: i32, global: i32) -> (i32, i32) {
        match self.tasks.root(id) {
            Some(root) => (root.windowing_mode, ACTIVITY_TYPE_UNDEFINED),
            None => (
                self.tasks.own_bounds(id).map_or(global, |(mode, _)| mode),
                self.activities.stack.lock().unwrap().activity_type(id),
            ),
        }
    }

    /// `RootWindowContainer.getRootTaskInfo(Task)` of root task `id`, its
    /// position unset, none if it went meanwhile. Its leaves are its tasks,
    /// else itself; it should be visible as the stack's visibility policy
    /// has its leaves (`shouldBeVisible`); its top activity is named as the
    /// intent of the top one (`topRunningActivity().intent.getComponent()`).
    fn root_task_info_at(&self, id: i32) -> Result<Option<RootTaskInfo>, Exception> {
        let mut task = match self.organizer_task_info(id) {
            Ok(task) => task,
            Err(_) if self.task_gone(id) => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut ids = self.tasks.children(Some(id));
        if ids.is_empty() {
            ids.push(id);
        }
        let mut leaves = Vec::new();
        for &leaf in ids.iter().rev() {
            match self.organizer_task_info(leaf) {
                Ok(info) => leaves.push(LeafTask::of(&info)),
                Err(_) if self.task_gone(leaf) => {}
                Err(e) => return Err(e),
            }
        }
        let (visible, top) = {
            let stack = self.activities.stack.lock().unwrap();
            let records = self.activities.records.lock().unwrap();
            let visible = ids
                .iter()
                .any(|&leaf| stack.visibility(leaf) != Visibility::Invisible);
            let top = ids.iter().find_map(|&leaf| {
                let r = records.get(&stack.top(leaf)?.token)?;
                let c = r.intent.intent(&r.package, &r.class).component?;
                Some((c.package, c.class))
            });
            (visible, top)
        };
        task.top_activity = top;
        Ok(Some(RootTaskInfo {
            task,
            leaves,
            visible,
            position: 0,
        }))
    }
}

/// The root-task queries of `IActivityTaskManager`.
pub(super) fn transact(host: &AppHost, call: &mut Call<'_>) -> Reply {
    let (uid, pid) = (call.sender_euid as i32, call.sender_pid);
    let list = |p: &mut Parcel, infos: Vec<RootTaskInfo>| {
        let infos: Vec<_> = infos.into_iter().map(Some).collect();
        atm::write_get_all_root_task_infos_reply(p, Some(&infos));
    };
    let one = |p: &mut Parcel, info: Option<RootTaskInfo>| {
        atm::write_get_root_task_info_reply(p, info.as_ref());
    };
    let mut p = Parcel::new();
    reply(
        match call.code {
            atm::GET_ALL_ROOT_TASK_INFOS => {
                atm::GetAllRootTaskInfos::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.enforce_task_permission(uid, pid, "getAllRootTaskInfos()")
                    .and_then(|()| host.all_root_task_infos(INVALID_DISPLAY))
                    .map(|infos| list(&mut p, infos))
            }
            atm::GET_ALL_ROOT_TASK_INFOS_ON_DISPLAY => {
                let a = atm::GetAllRootTaskInfosOnDisplay::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                host.enforce_task_permission(uid, pid, "getAllRootTaskInfosOnDisplay()")
                    .and_then(|()| host.all_root_task_infos(a.display_id))
                    .map(|infos| list(&mut p, infos))
            }
            atm::GET_ROOT_TASK_INFO => {
                let a = atm::GetRootTaskInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.enforce_task_permission(uid, pid, "getRootTaskInfo()")
                    .and_then(|()| {
                        host.root_task_info_of(a.windowing_mode, a.activity_type, DEFAULT_DISPLAY)
                    })
                    .map(|info| one(&mut p, info))
            }
            atm::GET_ROOT_TASK_INFO_ON_DISPLAY => {
                let a =
                    atm::GetRootTaskInfoOnDisplay::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.enforce_task_permission(uid, pid, "getRootTaskInfoOnDisplay()")
                    .and_then(|()| {
                        host.root_task_info_of(a.windowing_mode, a.activity_type, a.display_id)
                    })
                    .map(|info| one(&mut p, info))
            }
            atm::GET_FOCUSED_ROOT_TASK_INFO => {
                atm::GetFocusedRootTaskInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.enforce_task_permission(uid, pid, "getFocusedRootTaskInfo()")
                    .and_then(|()| host.focused_root_task_info())
                    .map(|info| atm::write_get_focused_root_task_info_reply(&mut p, info.as_ref()))
            }
            _ => return Err(BAD_VALUE),
        }
        .map(|()| p),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use aim_binder_host::local::Service;
    use aim_binder_host::parcel::EX_SECURITY;

    use super::*;
    use crate::app_host::activity::{ActivityTaskManager, Token};

    /// The parcel, read back in `RootTaskInfo.readFromParcel`'s order: its
    /// own fields, then `TaskInfo.writeTaskToParcel`'s.
    #[test]
    fn a_root_task_info_is_written_as_the_original_reads_it() {
        let task = super::super::task_info::tests::organizer_fixture();
        let mut expected_task = Parcel::new();
        task.write_task(&mut expected_task);
        let bounds = task.configuration.window.bounds;
        let child = Rect {
            left: 1,
            top: 2,
            right: 3,
            bottom: 4,
        };
        let info = RootTaskInfo {
            task,
            leaves: vec![
                LeafTask {
                    id: 9,
                    name: "p/p.Main".into(),
                    bounds: child,
                    user_id: 0,
                },
                LeafTask {
                    id: 10,
                    name: "unknown".into(),
                    bounds,
                    user_id: 10,
                },
            ],
            visible: true,
            position: 3,
        };
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let mut r = p.reader();
        let mut rect = || {
            assert_eq!(r.read_i32().unwrap(), 1);
            [0; 4].map(|_| r.read_i32().unwrap())
        };
        let flat = |b: Rect| [b.left, b.top, b.right, b.bottom];
        assert_eq!(rect(), flat(bounds));
        let mut ints = || {
            let n = r.read_i32().unwrap();
            (0..n).map(|_| r.read_i32().unwrap()).collect::<Vec<_>>()
        };
        assert_eq!(ints(), [9, 10]);
        assert_eq!(r.read_i32().unwrap(), 2);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("p/p.Main"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("unknown"));
        assert_eq!(r.read_i32().unwrap(), 2);
        for b in [child, bounds] {
            assert_eq!(r.read_i32().unwrap(), 1);
            assert_eq!([0; 4].map(|_| r.read_i32().unwrap()), flat(b));
        }
        let n = r.read_i32().unwrap();
        assert_eq!(
            (0..n).map(|_| r.read_i32().unwrap()).collect::<Vec<_>>(),
            [0, 10]
        );
        assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [1, 3]);
        assert_eq!(&p.data()[r.position()..], expected_task.data());
    }

    fn ask(host: &Arc<AppHost>, code: u32, args: &[i32], uid: u32) -> Parcel {
        let mut data = Parcel::new();
        data.write_interface_token(atm::DESCRIPTOR);
        args.iter().for_each(|a| data.write_i32(*a));
        ActivityTaskManager(host.clone(), host.process.add_service(Arc::new(Token)))
            .transact(&mut Call {
                code,
                flags: 0,
                sender_pid: 42,
                sender_euid: uid,
                data: data.reader(),
            })
            .unwrap()
    }

    /// The root tasks of `wct_tasks`, the root home task and an organizer
    /// root task holding task 8, top to bottom: their ids, positions,
    /// leaves and visibility through each query, behind MANAGE_ACTIVITY_TASKS.
    #[test]
    fn root_task_queries_report_the_root_tasks_by_mode_and_type() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let old = Rect {
                left: 100,
                top: 100,
                right: 900,
                bottom: 1500,
            };
            let (_thread, _activities, _bridge, _tree) =
                super::super::tasks::tests::wct_tasks(host, export, old);
            let (home, _) = super::super::tasks::tests::home_task(host);
            for task in &mut host.activities.stack.lock().unwrap().tasks {
                task.intent.component = Some(crate::package::intent::ComponentName {
                    package: "p".into(),
                    class: "p.Main".into(),
                });
            }
            super::super::task_organizer::tests::create_root(host, 0, 6, None)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap();
            let root = *host.tasks.root_task_ids().last().unwrap();
            host.reparent_task(8, Some(root), true).unwrap();

            let all = host.all_root_task_infos(INVALID_DISPLAY).unwrap();
            let ids: Vec<_> = all
                .iter()
                .map(|r| (r.task.task_id, r.position, r.visible))
                .collect();
            assert_eq!(ids, [(7, 2, true), (home, 1, false), (root, 0, true)]);
            let leaves = |r: &RootTaskInfo| {
                r.leaves
                    .iter()
                    .map(|l| (l.id, l.name.clone(), l.user_id))
                    .collect::<Vec<_>>()
            };
            assert_eq!(leaves(&all[0]), [(7, "p/p.Main".to_string(), 0)]);
            assert_eq!(leaves(&all[2]), [(8, "p/p.Main".to_string(), 0)]);
            // The organizer root task's activities are its task's (#2748),
            // the top one named by its intent's component (`A`), not by
            // its ActivityInfo (`p.Main`).
            let t = &all[2].task;
            assert_eq!(t.top_activity, Some(("p".into(), "A".into())));
            assert_eq!(t.base_activity, Some(("p".into(), "p.Main".into())));
            assert_eq!(
                (
                    t.is_running,
                    t.num_activities,
                    t.activity_type,
                    t.is_visible
                ),
                (true, 1, ACTIVITY_TYPE_STANDARD, true)
            );
            assert!(t.top_activity_info.is_some());
            assert_eq!(
                t.configuration.window.activity_type,
                ACTIVITY_TYPE_UNDEFINED
            );
            assert_eq!(all[2].leaves[0].bounds, host.tasks.bounds(8).unwrap());
            assert_eq!(all[1].task.activity_type, ACTIVITY_TYPE_HOME);
            assert!(host.all_root_task_infos(1).unwrap().is_empty());

            let find = |mode, kind, display| {
                host.root_task_info_of(mode, kind, display)
                    .unwrap()
                    .map(|r| r.task.task_id)
            };
            let freeform = all[0].task.configuration.window.windowing_mode;
            assert_eq!(find(0, ACTIVITY_TYPE_HOME, 0), Some(home));
            assert_eq!(find(6, ACTIVITY_TYPE_UNDEFINED, 0), Some(root));
            assert_eq!(find(freeform, ACTIVITY_TYPE_STANDARD, 0), Some(7));
            assert_eq!(find(freeform, ACTIVITY_TYPE_UNDEFINED, 0), Some(7));
            assert_eq!(find(WINDOWING_MODE_PINNED, ACTIVITY_TYPE_STANDARD, 0), None);
            assert_eq!(find(1, ACTIVITY_TYPE_STANDARD, 0), None);
            assert_eq!(find(0, ACTIVITY_TYPE_HOME, 1), None);

            // The calls, each behind enforceTaskPermission.
            for (code, args) in [
                (atm::GET_ALL_ROOT_TASK_INFOS, &[][..]),
                (atm::GET_ALL_ROOT_TASK_INFOS_ON_DISPLAY, &[0]),
                (atm::GET_ROOT_TASK_INFO, &[0, ACTIVITY_TYPE_HOME]),
                (
                    atm::GET_ROOT_TASK_INFO_ON_DISPLAY,
                    &[0, ACTIVITY_TYPE_HOME, 0],
                ),
                (atm::GET_FOCUSED_ROOT_TASK_INFO, &[]),
            ] {
                let denied = ask(host, code, args, 10100)
                    .reader()
                    .read_exception()
                    .unwrap()
                    .unwrap_err();
                assert_eq!(denied.code, EX_SECURITY);
                assert!(
                    denied.message.ends_with(
                        "from pid=42, uid=10100 requires android.permission.MANAGE_ACTIVITY_TASKS"
                    ),
                    "{}",
                    denied.message
                );
            }
            let reply = ask(host, atm::GET_ALL_ROOT_TASK_INFOS, &[], 1000);
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 3);
            assert_eq!(r.read_i32().unwrap(), 1);
            let mut first = Parcel::new();
            all[0].write_to(&mut first);
            let at = r.position();
            assert_eq!(&reply.data()[at..at + first.data().len()], first.data());
            let reply = ask(host, atm::GET_ALL_ROOT_TASK_INFOS_ON_DISPLAY, &[1], 1000);
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 0);
            let reply = ask(
                host,
                atm::GET_ROOT_TASK_INFO,
                &[0, ACTIVITY_TYPE_HOME],
                1000,
            );
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 1);
            r.skip(4 * 5).unwrap(); // bounds
            assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [1, home]);
            let reply = ask(
                host,
                atm::GET_ROOT_TASK_INFO_ON_DISPLAY,
                &[WINDOWING_MODE_PINNED, ACTIVITY_TYPE_STANDARD, 0],
                1000,
            );
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 0);

            // getFocusedRootTaskInfo: the focused task's root task.
            let focused = host.activities.stack.lock().unwrap().focused().unwrap();
            let expected = host.tasks.parent(focused).unwrap_or(focused);
            let info = host.focused_root_task_info().unwrap().unwrap();
            assert_eq!(info.task.task_id, expected);
            let reply = ask(host, atm::GET_FOCUSED_ROOT_TASK_INFO, &[], 1000);
            let mut r = reply.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 1);
            let mut written = Parcel::new();
            info.write_to(&mut written);
            assert_eq!(&reply.data()[r.position()..], written.data());

            // Task 7 being removed (out of the stack, still in the task
            // owner) is left out, and the positions follow.
            {
                let mut stack = host.activities.stack.lock().unwrap();
                stack.tasks.retain(|t| t.id != 7);
                stack.activities.retain(|a| a.task != 7);
            }
            let all = host.all_root_task_infos(INVALID_DISPLAY).unwrap();
            let ids: Vec<_> = all.iter().map(|r| (r.task.task_id, r.position)).collect();
            assert_eq!(ids, [(home, 1), (root, 0)]);
            assert_eq!(find(freeform, ACTIVITY_TYPE_STANDARD, 0), None);
            assert_eq!(find(0, ACTIVITY_TYPE_HOME, 0), Some(home));
        });
    }
}
