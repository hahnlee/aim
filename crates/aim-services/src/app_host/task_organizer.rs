//! WindowOrganizerController / TaskOrganizerController, android-16.0.0_r1.
//! Registration delegates actual native task surfaces; organizer root tasks
//! are created, listed, deleted and hold tasks. Unsupported hierarchy and
//! transaction operations stay explicit (#2616).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::surface::Layer;
use super::{AppHost, reply};
use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception,
    Parcel,
};
use aim_service_aidl::{
    ReadParcelable, WriteParcelable, android_window_itaskorganizer as callback,
    android_window_itaskorganizercontroller as toc,
    android_window_iwindoworganizercontroller as woc,
};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
pub(super) struct Organizers {
    controller: OnceLock<Binder>,
    tasks: OnceLock<Binder>,
    state: Mutex<State>,
}
#[derive(Default)]
struct State {
    entries: Vec<Entry>,
    reports: VecDeque<(Arc<Strong>, u32, Parcel)>,
    reporting: bool,
}
struct Entry {
    binder: Binder,
    held: Arc<Strong>,
    death: u64,
    tasks: BTreeMap<i32, Appeared>,
}
#[derive(Clone)]
pub(super) struct Leash {
    parcel: Parcel,
    _held: Arc<Strong>,
}
impl Leash {
    pub(super) fn new(host: &AppHost, client: Binder, layer: &Layer) -> Self {
        let mut parcel = Parcel::new();
        parcel.write_string8(Some(&layer.name));
        parcel.write_i32(0);
        parcel.write_i32(0);
        parcel.write_i32(1);
        layer.control(client).write_to(&mut parcel);
        let Binder::Handle(handle) = layer.handle else {
            unreachable!()
        };
        Self {
            parcel,
            _held: Arc::new(host.process.strong(handle)),
        }
    }
}
impl WriteParcelable for Leash {
    fn write_to(&self, p: &mut Parcel) {
        p.write_raw(self.parcel.data(), self.parcel.objects());
        p.keep_alive(self._held.clone());
    }
}
#[derive(Clone)]
struct Appeared {
    info: Parcel,
    leash: Leash,
}
impl WriteParcelable for Appeared {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(1);
        p.write_raw(self.info.data(), self.info.objects());
        p.write_i32(1);
        self.leash.write_to(p);
    }
}
/// A `WindowContainerToken`: its strong binder.
struct ContainerToken(Binder);
impl ReadParcelable for ContainerToken {
    fn read_from(
        r: &mut aim_binder_host::parcel::Reader<'_>,
    ) -> aim_binder_host::parcel::Result<Self> {
        Ok(Self(r.read_binder()?.ok_or(BAD_VALUE)?))
    }
}
/// A task of the organizer snapshot: its id, whether it is ready to appear,
/// whether it is an organizer root task, and what appears.
type Current = (i32, bool, bool, Appeared);
impl State {
    fn report(&mut self, target: Arc<Strong>, code: u32, task: &Appeared) {
        let mut p = Parcel::new();
        p.write_interface_token(callback::DESCRIPTOR);
        p.write_i32(1);
        p.write_raw(task.info.data(), task.info.objects());
        if code == callback::ON_TASK_APPEARED {
            p.write_i32(1);
            task.leash.write_to(&mut p);
        }
        self.reports.push_back((target, code, p));
    }
    /// Records `current` for the active organizer and reports what changed;
    /// on `initial` registration, returns the tasks the organizer did not
    /// create and reports its root tasks appeared (`registerTaskOrganizer`).
    fn update(&mut self, current: &[Current], initial: bool) -> Vec<Appeared> {
        let Some(entry) = self.entries.last_mut() else {
            return Vec::new();
        };
        let target = entry.held.clone();
        let mut previous = std::mem::take(&mut entry.tasks);
        let mut reports = Vec::new();
        let mut returned = Vec::new();
        for (id, ready, root, task) in current {
            let before = previous.remove(id);
            if before.is_none() && !initial && !ready {
                continue;
            }
            if initial && !root {
                returned.push(task.clone());
            } else if before.is_none() {
                reports.push((callback::ON_TASK_APPEARED, task.clone()));
            } else if before.as_ref().unwrap().info.data() != task.info.data() {
                reports.push((callback::ON_TASK_INFO_CHANGED, task.clone()));
            }
            entry.tasks.insert(*id, task.clone());
        }
        reports.extend(
            previous
                .into_values()
                .map(|t| (callback::ON_TASK_VANISHED, t)),
        );
        for (code, task) in reports {
            self.report(target.clone(), code, &task);
        }
        returned
    }
}
struct Controller(Arc<AppHost>, bool);
impl Service for Controller {
    fn descriptor(&self) -> &str {
        if self.1 {
            toc::DESCRIPTOR
        } else {
            woc::DESCRIPTOR
        }
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if let Err(e) = self.0.enforce_organizer_permission(call.sender_euid as i32) {
            return reply(Err(e));
        }
        match (self.1, call.code) {
            (false, woc::APPLY_TRANSACTION) => {
                call.data.enforce_interface(woc::DESCRIPTOR)?;
                reply((|| {
                    if call
                        .data
                        .read_i32()
                        .map_err(|_| Exception::illegal_argument("WCT"))?
                        != 1
                    {
                        return Err(Exception::new(EX_NULL_POINTER, "transaction"));
                    }
                    let transaction =
                        super::window_container_transaction::Transaction::read(&mut call.data)?;
                    self.0.apply_window_transaction(transaction)?;
                    let mut p = Parcel::new();
                    p.write_no_exception();
                    Ok(p)
                })())
            }
            (false, woc::GET_TASK_ORGANIZER_CONTROLLER) => {
                woc::GetTaskOrganizerController::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let binder = *self.0.organizers.tasks.get_or_init(|| {
                    self.0
                        .process
                        .add_service(Arc::new(Controller(self.0.clone(), true)))
                });
                let mut p = Parcel::new();
                woc::write_get_task_organizer_controller_reply(&mut p, Some(binder));
                Ok(p)
            }
            (true, toc::REGISTER_TASK_ORGANIZER) => {
                let a = toc::RegisterTaskOrganizer::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.register_task_organizer(a.organizer))
            }
            (true, toc::UNREGISTER_TASK_ORGANIZER) => {
                let a =
                    toc::UnregisterTaskOrganizer::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.unregister_task_organizer(a.organizer, None))
            }
            (true, toc::CREATE_ROOT_TASK) => {
                let a = toc::CreateRootTask::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.create_organizer_root_task(
                    a.display_id,
                    a.windowing_mode,
                    a.launch_cookie,
                ))
            }
            (true, toc::DELETE_ROOT_TASK) => {
                let a = toc::DeleteRootTask::<ContainerToken>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(self.0.delete_root_task(a.task.map(|t| t.0)))
            }
            (true, toc::GET_CHILD_TASKS) => {
                let a = toc::GetChildTasks::<ContainerToken>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .organizer_child_tasks(a.parent.map(|t| t.0), a.activity_types),
                )
            }
            (true, toc::GET_ROOT_TASKS) => {
                let a = toc::GetRootTasks::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.organizer_root_tasks(a.display_id, a.activity_types))
            }
            (_, code) => reply(Err(self.0.unimplemented(
                if self.1 {
                    "task_organizer"
                } else {
                    "window_organizer"
                },
                self.descriptor(),
                code,
            ))),
        }
    }
}
impl AppHost {
    pub(super) fn window_organizer_controller(self: &Arc<Self>) -> Binder {
        *self.organizers.controller.get_or_init(|| {
            self.process
                .add_service(Arc::new(Controller(self.clone(), false)))
        })
    }
    fn enforce_organizer_permission(&self, uid: i32) -> Result<(), Exception> {
        if self.check_permission(Some("android.permission.MANAGE_ACTIVITY_TASKS"), uid)? == 0
            || self.check_permission(Some("android.permission.MANAGE_ACTIVITY_STACKS"), uid)? == 0
        {
            Ok(())
        } else {
            Err(Exception::security("Requires MANAGE_ACTIVITY_TASKS"))
        }
    }
    pub(super) fn clear_organizer_launch_params(
        &self,
        uid: i32,
        packages: Option<Vec<Option<String>>>,
    ) -> Result<Parcel, Exception> {
        self.enforce_organizer_permission(uid)?;
        let packages = packages.ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageNames"))?;
        let owner = self.tasks.launch_params(self.data()?)?;
        for package in packages.into_iter().flatten() {
            owner.remove_package(&package).map_err(state_error)?;
        }
        let mut p = Parcel::new();
        p.write_no_exception();
        Ok(p)
    }
    fn organizer_snapshot(&self) -> Result<Vec<Current>, Exception> {
        self.organizer_surfaces()?
            .into_iter()
            .map(|(id, ready, root, leash)| {
                let mut info = Parcel::new();
                self.organizer_task_info(id)?.write_to(&mut info);
                Ok((id, ready, root, Appeared { info, leash }))
            })
            .collect()
    }
    fn migrate_organizer_snapshot(&self, current: &mut [Current]) -> Result<(), Exception> {
        for (id, leash) in self.migrate_organized_tasks().map_err(state_error)? {
            if let Some((_, _, _, task)) = current.iter_mut().find(|t| t.0 == id) {
                task.leash = leash;
            }
        }
        Ok(())
    }
    pub(super) fn register_task_organizer(
        self: &Arc<Self>,
        binder: Option<Binder>,
    ) -> Result<Parcel, Exception> {
        let binder = binder.ok_or_else(|| Exception::new(EX_NULL_POINTER, "organizer"))?;
        let Binder::Handle(handle) = binder else {
            return Err(Exception::illegal_argument("nonremote organizer"));
        };
        let mut state = self.organizers.state.lock().unwrap();
        // Finish all fallible owner reads before publishing an Entry or a death watch.
        let mut current = self.organizer_snapshot()?;
        let existing = state.entries.iter().position(|e| e.binder == binder);
        if existing.is_none() {
            self.migrate_organizer_snapshot(&mut current)?;
            if let Some(old) = state.entries.last_mut() {
                let target = old.held.clone();
                let tasks = std::mem::take(&mut old.tasks);
                for task in tasks.into_values() {
                    state.report(target.clone(), callback::ON_TASK_VANISHED, &task);
                }
            }
            let held = Arc::new(self.process.strong(handle));
            let host = self.me.clone();
            let owner = Arc::downgrade(&held);
            let death = self.process.link_to_death(
                &held,
                Box::new(move || {
                    if let (Some(host), Some(owner)) = (host.upgrade(), owner.upgrade()) {
                        if let Err(e) = host.unregister_task_organizer(Some(binder), Some(&owner)) {
                            eprintln!("app host: task organizer death: {e:?}");
                        }
                    }
                }),
            );
            state.entries.push(Entry {
                binder,
                held,
                death,
                tasks: BTreeMap::new(),
            });
        }
        let returned = if existing.is_some_and(|at| at + 1 != state.entries.len()) {
            // A known inactive caller does not become active or rewrite its successor's history.
            current
                .into_iter()
                .filter(|t| !t.2)
                .map(|(_, _, _, task)| task)
                .collect()
        } else {
            self.set_tasks_organized(true);
            state.update(&current, true)
        };
        let slice = crate::package::list_slice::Slice::new(
            self.process.clone(),
            "android.window.TaskAppearedInfo",
            returned,
        );
        let mut p = Parcel::new();
        toc::write_register_task_organizer_reply(&mut p, Some(&slice));
        drop(state);
        self.dispatch_organizer_reports();
        Ok(p)
    }
    pub(super) fn unregister_task_organizer(
        self: &Arc<Self>,
        binder: Option<Binder>,
        expected: Option<&Arc<Strong>>,
    ) -> Result<Parcel, Exception> {
        let binder = binder.ok_or_else(|| Exception::new(EX_NULL_POINTER, "organizer"))?;
        let mut state = self.organizers.state.lock().unwrap();
        let mut roots = Vec::new();
        if let Some(at) = state.entries.iter().position(|e| {
            e.binder == binder && expected.is_none_or(|held| Arc::ptr_eq(held, &e.held))
        }) {
            let active = at + 1 == state.entries.len();
            let mut current = if active && state.entries.len() > 1 {
                self.organizer_snapshot()?
            } else {
                Vec::new()
            };
            if active {
                self.migrate_organizer_snapshot(&mut current)?;
            }
            let removed = state.entries.remove(at);
            self.process.clear_death(&removed.held, removed.death);
            state
                .reports
                .retain(|(target, _, _)| !Arc::ptr_eq(target, &removed.held));
            if active {
                // TaskOrganizerState.dispose: its root tasks go with it.
                roots = self.remove_root_tasks();
                current.retain(|t| !roots.contains(&t.0));
                self.set_tasks_organized(!state.entries.is_empty());
                if !state.entries.is_empty() {
                    state.update(&current, false);
                }
            }
        }
        drop(state);
        self.remove_root_children(&roots);
        self.dispatch_organizer_reports();
        let mut p = Parcel::new();
        p.write_no_exception();
        Ok(p)
    }
    /// `createRootTask`: an unknown display creates nothing, as the
    /// original logs and returns.
    fn create_organizer_root_task(
        &self,
        display: i32,
        mode: i32,
        cookie: Option<Binder>,
    ) -> Result<Parcel, Exception> {
        if display != super::activity::DEFAULT_DISPLAY {
            eprintln!("app host: createRootTask unknown displayId={display}");
        } else {
            // Task.Builder.validateRootTask: TaskDisplayArea.isWindowingModeSupported.
            let multi_window = self.supports_multi_window()?;
            match mode {
                6 if multi_window => {}
                0 | 1 => return Err(unsupported_root(mode)),
                2 | 5 if multi_window => return Err(unsupported_root(mode)),
                _ => {
                    return Err(Exception::illegal_argument(format!(
                        "Can't create root task for unsupported windowingMode={mode}"
                    )));
                }
            }
            self.create_root_task(mode, cookie).map_err(state_error)?;
            self.organizer_tasks_changed()?;
        }
        let mut p = Parcel::new();
        toc::write_create_root_task_reply(&mut p);
        Ok(p)
    }
    /// `deleteRootTask`: false for a token of no task.
    fn delete_root_task(self: &Arc<Self>, token: Option<Binder>) -> Result<Parcel, Exception> {
        let token = token.ok_or_else(|| Exception::new(EX_NULL_POINTER, "task"))?;
        let deleted = match self.task_of_token(token) {
            None => false,
            Some(id) if self.tasks.root(id).is_none() => {
                return Err(Exception::illegal_argument(format!(
                    "Attempt to delete task not created by organizer task={id}"
                )));
            }
            Some(id) => {
                self.remove_root_children(&[id]);
                self.remove_task(id);
                true
            }
        };
        let mut p = Parcel::new();
        toc::write_delete_root_task_reply(&mut p, deleted);
        Ok(p)
    }
    /// `getRootTasks`: the display's root tasks of `types` if given, top to
    /// bottom.
    fn organizer_root_tasks(
        &self,
        display: i32,
        types: Option<Vec<i32>>,
    ) -> Result<Parcel, Exception> {
        if display != super::activity::DEFAULT_DISPLAY {
            return Err(Exception::illegal_argument(format!(
                "Display {display} doesn't exist"
            )));
        }
        let mut infos = Vec::new();
        for id in self.tasks.root_task_ids() {
            let info = self.organizer_task_info(id)?;
            if types
                .as_ref()
                .is_none_or(|t| t.contains(&info.activity_type))
            {
                infos.push(Some(info));
            }
        }
        let mut p = Parcel::new();
        toc::write_get_root_tasks_reply(&mut p, Some(&infos));
        Ok(p)
    }
    /// `getChildTasks`: the tasks of an organizer root task top to bottom,
    /// of `types` if given; null for a token of no root task.
    fn organizer_child_tasks(
        &self,
        parent: Option<Binder>,
        types: Option<Vec<i32>>,
    ) -> Result<Parcel, Exception> {
        let parent = parent
            .ok_or_else(|| Exception::illegal_argument("Can't get children of null parent"))?;
        let infos = match self.task_of_token(parent) {
            Some(root) if self.tasks.root(root).is_some() => {
                let mut infos = Vec::new();
                for child in self.tasks.children(Some(root)) {
                    let info = self.organizer_task_info(child)?;
                    if types
                        .as_ref()
                        .is_none_or(|t| t.contains(&info.activity_type))
                    {
                        infos.push(Some(info));
                    }
                }
                Some(infos)
            }
            _ => None,
        };
        let mut p = Parcel::new();
        toc::write_get_child_tasks_reply(&mut p, infos.as_deref());
        Ok(p)
    }
    pub(super) fn organizer_tasks_changed(&self) -> Result<(), Exception> {
        let mut state = self.organizers.state.lock().unwrap();
        if !state.entries.is_empty() {
            state.update(&self.organizer_snapshot()?, false);
            self.set_tasks_organized(true);
        }
        drop(state);
        self.dispatch_organizer_reports();
        Ok(())
    }
    pub(super) fn notify_task_organizers(&self) {
        if let Err(e) = self.organizer_tasks_changed() {
            eprintln!("app host: task organizers: {e:?}");
        }
    }
    fn dispatch_organizer_reports(&self) {
        let mut state = self.organizers.state.lock().unwrap();
        if state.reporting {
            return;
        }
        state.reporting = true;
        loop {
            let Some((target, code, parcel)) = state.reports.pop_front() else {
                state.reporting = false;
                return;
            };
            drop(state);
            if let Err(e) = target.transact(code, &parcel, true) {
                eprintln!("app host: task organizer callback: {e}");
            }
            state = self.organizers.state.lock().unwrap();
        }
    }
}
fn unsupported_root(mode: i32) -> Exception {
    Exception::new(
        EX_UNSUPPORTED_OPERATION,
        format!("organizer root tasks in windowing mode {mode} (#2616)"),
    )
}
fn state_error(e: String) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, e)
}

#[cfg(test)]
#[path = "task_organizer_tests.rs"]
pub(super) mod tests;
