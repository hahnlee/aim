//! The window shell (`docs/task-organizer.md`): the task organizer and
//! transition player of an image without WMShell, in system_server, as
//! `aim.window_shell`. Attaching to it makes window mode desktop windowing
//! (new tasks become freeform), and its listener hears each task's activity
//! type and top activity's manifest orientation.
//!
//! `IWindowShell` and `IWindowShellListener` are our AIDL
//! (`java/device-services/aidl`), which the platform's Java compiler gives
//! transaction codes in declaration order from 1; they are written here by
//! hand, as the framework's in [`crate::framework`].

use binder::binder_impl::{BorrowedParcel, TransactionCode};
use binder::{BinderFeatures, Interface, Status, StatusCode, Strong, declare_binder_interface};

use crate::framework::{call, connect, unused};

/// Its name in servicemanager.
const SERVICE: &str = "aim.window_shell";
/// `IWindowShell.attach`.
const ATTACH: u32 = 1;
/// `IWindowShellListener.onTaskChanged` (oneway).
const ON_TASK_CHANGED: u32 = 1;

pub trait IWindowShell: Interface {}

declare_binder_interface! {
    IWindowShell["dev.aim.server.IWindowShell"] {
        native: BnWindowShell(unused),
        proxy: BpWindowShell,
    }
}

impl IWindowShell for BpWindowShell {}
impl IWindowShell for binder::binder_impl::Binder<BnWindowShell> {}

impl BpWindowShell {
    pub fn attach(&self, l: &Strong<dyn IWindowShellListener>) -> Result<(), Status> {
        call(&self.binder, ATTACH, |p| p.write(&l.as_binder())).map(drop)
    }
}

/// The shell, waited for.
pub fn window_shell() -> Option<BpWindowShell> {
    connect::<BnWindowShell, _>(SERVICE)
}

pub trait IWindowShellListener: Interface {
    /// Task `task`'s activity type (`WindowConfiguration.ACTIVITY_TYPE_*`)
    /// and its top activity's `screenOrientation`, when either changed.
    fn task_changed(&self, task: i32, activity_type: i32, orientation: i32);
}

declare_binder_interface! {
    IWindowShellListener["dev.aim.server.IWindowShellListener"] {
        native: BnWindowShellListener(on_transact),
        proxy: BpWindowShellListener,
    }
}

impl IWindowShellListener for BpWindowShellListener {
    fn task_changed(&self, _: i32, _: i32, _: i32) {}
}

impl IWindowShellListener for binder::binder_impl::Binder<BnWindowShellListener> {
    fn task_changed(&self, task: i32, activity_type: i32, orientation: i32) {
        self.0.task_changed(task, activity_type, orientation)
    }
}

fn on_transact(
    listener: &dyn IWindowShellListener,
    code: TransactionCode,
    data: &BorrowedParcel<'_>,
    _reply: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    if code != ON_TASK_CHANGED {
        return Err(StatusCode::UNKNOWN_TRANSACTION);
    }
    listener.task_changed(data.read()?, data.read()?, data.read()?);
    Ok(())
}

pub fn new_listener<T: IWindowShellListener + Sync + Send + 'static>(
    inner: T,
) -> Strong<dyn IWindowShellListener> {
    BnWindowShellListener::new_binder(inner, BinderFeatures::default())
}
