//! The window shell (`docs/task-organizer.md`): the task organizer and
//! transition player of an image without WMShell, in system_server, as
//! `aim.window_shell`. Attaching to it makes window mode desktop windowing
//! (new tasks become freeform), and its listener hears each task's activity
//! type and top activity's manifest orientation, and each task whose
//! transition finished.
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
/// `IWindowShell.setPipBounds`.
const SET_PIP_BOUNDS: u32 = 2;
/// `IWindowShellListener.onTaskChanged` (oneway).
const ON_TASK_CHANGED: u32 = 1;
/// `IWindowShellListener.onTaskPlaced` (oneway).
const ON_TASK_PLACED: u32 = 2;

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

    /// Moves or resizes `task`, in picture-in-picture, to `bounds`.
    pub fn set_pip_bounds(&self, task: i32, bounds: [i32; 4]) -> Result<(), Status> {
        call(&self.binder, SET_PIP_BOUNDS, |p| {
            p.write(&task)?;
            bounds.iter().try_for_each(|b| p.write(b))
        })
        .map(drop)
    }
}

/// The shell, waited for.
pub fn window_shell() -> Option<BpWindowShell> {
    connect::<BnWindowShell, _>(SERVICE)
}

pub trait IWindowShellListener: Interface {
    /// Task `task`'s activity type (`WindowConfiguration.ACTIVITY_TYPE_*`),
    /// its top activity's `screenOrientation` and its windowing mode
    /// (`WindowConfiguration.WINDOWING_MODE_*`), when one changed.
    fn task_changed(&self, task: i32, activity_type: i32, orientation: i32, mode: i32);
    /// A transition task `task` took part in finished: its surface is at
    /// its bounds, showing what it has drawn (its starting window or app).
    fn task_placed(&self, task: i32);
}

declare_binder_interface! {
    IWindowShellListener["dev.aim.server.IWindowShellListener"] {
        native: BnWindowShellListener(on_transact),
        proxy: BpWindowShellListener,
    }
}

impl IWindowShellListener for BpWindowShellListener {
    fn task_changed(&self, _: i32, _: i32, _: i32, _: i32) {}
    fn task_placed(&self, _: i32) {}
}

impl IWindowShellListener for binder::binder_impl::Binder<BnWindowShellListener> {
    fn task_changed(&self, task: i32, activity_type: i32, orientation: i32, mode: i32) {
        self.0.task_changed(task, activity_type, orientation, mode)
    }
    fn task_placed(&self, task: i32) {
        self.0.task_placed(task)
    }
}

fn on_transact(
    listener: &dyn IWindowShellListener,
    code: TransactionCode,
    data: &BorrowedParcel<'_>,
    _reply: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    match code {
        ON_TASK_CHANGED => {
            let (task, activity_type) = (data.read()?, data.read()?);
            listener.task_changed(task, activity_type, data.read()?, data.read()?)
        }
        ON_TASK_PLACED => listener.task_placed(data.read()?),
        _ => return Err(StatusCode::UNKNOWN_TRANSACTION),
    }
    Ok(())
}

pub fn new_listener<T: IWindowShellListener + Sync + Send + 'static>(
    inner: T,
) -> Strong<dyn IWindowShellListener> {
    BnWindowShellListener::new_binder(inner, BinderFeatures::default())
}
