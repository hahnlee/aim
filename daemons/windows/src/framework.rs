//! The framework's task and window interfaces, as a client, and the task
//! listener it calls back.
//!
//! `IActivityTaskManager`, `IWindowManager` and `ITaskStackListener` are
//! Java AIDL interfaces of the platform, with no NDK or Rust backend: their
//! methods take Java-only parcelables. The few calls the bridge makes are
//! written here by hand, with the transaction codes of the pinned image's
//! AIDL (`android-16.0.0_r1`, first method = 1, in declaration order; the
//! image's `framework.jar` stubs have the same `TRANSACTION_*` values).
//! A class associated with each remote binder writes the interface token,
//! as a generated proxy's does.

use aim_windows_core::{Error, Read, TaskInfo, write};
use binder::binder_impl::{
    AssociateClass, BorrowedParcel, IBinderInternal, Proxy, Remotable, TransactionCode,
};
use binder::{
    BinderFeatures, ExceptionCode, Interface, SpIBinder, Status, StatusCode, Strong,
    declare_binder_interface,
};

/// `IActivityTaskManager` transaction codes.
mod atm {
    pub const START_ACTIVITY_AS_USER: u32 = 3;
    pub const SET_FOCUSED_TASK: u32 = 20;
    pub const REMOVE_TASK: u32 = 22;
    pub const GET_TASK_BOUNDS: u32 = 32;
    pub const REGISTER_TASK_STACK_LISTENER: u32 = 45;
    pub const RESIZE_TASK: u32 = 48;
}

/// `IWindowManager` transaction codes.
mod wm {
    pub const GET_BASE_DISPLAY_SIZE: u32 = 6;
    pub const GET_BASE_DISPLAY_DENSITY: u32 = 10;
    pub const GET_WINDOWING_MODE: u32 = 98;
    pub const SET_WINDOWING_MODE: u32 = 99;
}

/// `ITaskStackListener` transaction codes (all oneway).
mod listener {
    pub const ON_TASK_STACK_CHANGED: u32 = 1;
    pub const ON_TASK_CREATED: u32 = 9;
    pub const ON_TASK_REMOVED: u32 = 10;
    pub const ON_TASK_MOVED_TO_FRONT: u32 = 11;
    pub const ON_TASK_DESCRIPTION_CHANGED: u32 = 12;
    pub const ON_TASK_FOCUS_CHANGED: u32 = 23;
    pub const ON_TASK_MOVED_TO_BACK: u32 = 26;
}

/// `WindowConfiguration.WINDOWING_MODE_*`.
pub const WINDOWING_MODE_FULLSCREEN: i32 = 1;
pub const WINDOWING_MODE_FREEFORM: i32 = 5;
/// `ActivityTaskManager.RESIZE_MODE_USER`: a resize the user asked for.
const RESIZE_MODE_USER: i32 = 1;
/// `Intent.FLAG_ACTIVITY_NEW_TASK | FLAG_ACTIVITY_RESET_TASK_IF_NEEDED`,
/// as a launcher starts an app.
const LAUNCH_FLAGS: i32 = 0x1000_0000 | 0x0020_0000;
/// `UserHandle.USER_CURRENT`.
const USER_CURRENT: i32 = -2;

type Result<T> = std::result::Result<T, Status>;
/// What parcel reads and writes, and transaction handlers, return.
type Wire<T> = std::result::Result<T, StatusCode>;

pub trait IActivityTaskManager: Interface {}
pub trait IWindowManager: Interface {}

/// Transactions of a Java service no one implements here.
fn unused<I: ?Sized>(
    _: &I,
    _: TransactionCode,
    _: &BorrowedParcel<'_>,
    _: &mut BorrowedParcel<'_>,
) -> Wire<()> {
    Err(StatusCode::UNKNOWN_TRANSACTION)
}

declare_binder_interface! {
    IActivityTaskManager["android.app.IActivityTaskManager"] {
        native: BnActivityTaskManager(unused),
        proxy: BpActivityTaskManager,
    }
}

declare_binder_interface! {
    IWindowManager["android.view.IWindowManager"] {
        native: BnWindowManager(unused),
        proxy: BpWindowManager,
    }
}

impl IActivityTaskManager for BpActivityTaskManager {}
impl IActivityTaskManager for binder::binder_impl::Binder<BnActivityTaskManager> {}
impl IWindowManager for BpWindowManager {}
impl IWindowManager for binder::binder_impl::Binder<BnWindowManager> {}

/// Send `code` with the arguments `args` writes, and read the reply's
/// status: the reply, positioned after it.
fn call(
    binder: &SpIBinder,
    code: u32,
    args: impl FnOnce(&mut BorrowedParcel<'_>) -> Wire<()>,
) -> Result<binder::binder_impl::Parcel> {
    let mut data = binder.prepare_transact()?;
    args(&mut data.borrowed())?;
    let reply = binder.submit_transact(code, data, 0)?;
    let status: Status = reply.read()?;
    if status.is_ok() {
        Ok(reply)
    } else {
        Err(status)
    }
}

/// `Rect` as `writeTypedObject` writes it.
fn write_rect(p: &mut BorrowedParcel<'_>, r: [i32; 4]) -> Wire<()> {
    p.write(&1i32)?;
    r.iter().try_for_each(|v| p.write(v))
}

fn write_string8(p: &mut BorrowedParcel<'_>, s: Option<&str>) -> Wire<()> {
    write::string8(s).iter().try_for_each(|w| p.write(w))
}

/// An `Intent` that starts `package`'s launcher activity, as
/// `Intent.writeToParcel` writes it after its typed-object marker.
fn write_launch_intent(p: &mut BorrowedParcel<'_>, package: &str) -> Wire<()> {
    write_string8(p, Some("android.intent.action.MAIN"))?;
    p.write(&0i32)?; // no Uri
    write_string8(p, None)?; // type
    write_string8(p, None)?; // identifier
    p.write(&LAUNCH_FLAGS)?;
    p.write(&0i32)?; // extended flags
    write_string8(p, Some(package))?;
    p.write(&None::<String>)?; // no component
    p.write(&0i32)?; // no source bounds
    p.write(&1i32)?;
    write_string8(p, Some("android.intent.category.LAUNCHER"))?;
    p.write(&0i32)?; // no selector
    p.write(&0i32)?; // no ClipData
    p.write(&USER_CURRENT)?; // content user hint
    p.write(&-1i32)?; // no extras
    p.write(&0i32)?; // no original intent
    p.write(&0i32) // no creator token
}

impl BpActivityTaskManager {
    pub fn register_task_stack_listener(&self, l: &Strong<dyn ITaskStackListener>) -> Result<()> {
        call(&self.binder, atm::REGISTER_TASK_STACK_LISTENER, |p| {
            p.write(&l.as_binder())
        })
        .map(drop)
    }

    /// The task's bounds, None when it has none (it fills its parent).
    pub fn task_bounds(&self, task: i32) -> Result<Option<[i32; 4]>> {
        let reply = call(&self.binder, atm::GET_TASK_BOUNDS, |p| p.write(&task))?;
        if reply.read::<i32>()? == 0 {
            return Ok(None);
        }
        let mut r = [0; 4];
        for v in &mut r {
            *v = reply.read()?;
        }
        Ok(Some(r))
    }

    pub fn resize_task(&self, task: i32, bounds: [i32; 4]) -> Result<()> {
        call(&self.binder, atm::RESIZE_TASK, |p| {
            p.write(&task)?;
            write_rect(p, bounds)?;
            p.write(&RESIZE_MODE_USER)
        })
        .map(drop)
    }

    pub fn set_focused_task(&self, task: i32) -> Result<()> {
        call(&self.binder, atm::SET_FOCUSED_TASK, |p| p.write(&task)).map(drop)
    }

    pub fn remove_task(&self, task: i32) -> Result<bool> {
        Ok(call(&self.binder, atm::REMOVE_TASK, |p| p.write(&task))?.read()?)
    }

    /// Start `package`'s launcher activity as a launcher would; the
    /// `ActivityManager.START_*` result.
    pub fn start_package(&self, package: &str) -> Result<i32> {
        let reply = call(&self.binder, atm::START_ACTIVITY_AS_USER, |p| {
            p.write(&None::<SpIBinder>)?; // caller
            p.write(&"android")?; // callingPackage
            p.write(&None::<String>)?; // callingFeatureId
            p.write(&1i32)?;
            write_launch_intent(p, package)?;
            p.write(&None::<String>)?; // resolvedType
            p.write(&None::<SpIBinder>)?; // resultTo
            p.write(&None::<String>)?; // resultWho
            p.write(&0i32)?; // requestCode
            p.write(&0i32)?; // flags
            p.write(&0i32)?; // no ProfilerInfo
            p.write(&0i32)?; // no options
            p.write(&USER_CURRENT)
        })?;
        Ok(reply.read()?)
    }
}

impl BpWindowManager {
    pub fn windowing_mode(&self, display: i32) -> Result<i32> {
        Ok(call(&self.binder, wm::GET_WINDOWING_MODE, |p| p.write(&display))?.read()?)
    }

    pub fn set_windowing_mode(&self, display: i32, mode: i32) -> Result<()> {
        call(&self.binder, wm::SET_WINDOWING_MODE, |p| {
            p.write(&display)?;
            p.write(&mode)
        })
        .map(drop)
    }

    /// The display's size in pixels, with any override (`wm size`).
    pub fn display_size(&self, display: i32) -> Result<(i32, i32)> {
        let reply = call(&self.binder, wm::GET_BASE_DISPLAY_SIZE, |p| {
            p.write(&display)
        })?;
        // The `out Point`, as `writeTypedObject` writes it.
        if reply.read::<i32>()? == 0 {
            return Err(Status::new_exception(ExceptionCode::ILLEGAL_STATE, None));
        }
        Ok((reply.read()?, reply.read()?))
    }

    /// The display's density in dots per inch, with any override.
    pub fn display_density(&self, display: i32) -> Result<i32> {
        Ok(call(&self.binder, wm::GET_BASE_DISPLAY_DENSITY, |p| {
            p.write(&display)
        })?
        .read()?)
    }
}

pub fn activity_task_manager() -> Option<BpActivityTaskManager> {
    connect::<BnActivityTaskManager, _>("activity_task")
}

pub fn window_manager() -> Option<BpWindowManager> {
    connect::<BnWindowManager, _>("window")
}

/// The service `name`, waited for, with its interface's class associated
/// (so transactions carry the interface token).
fn connect<N: Remotable, P: Proxy>(name: &str) -> Option<P> {
    let mut b = binder::wait_for_service(name)?;
    if !b.associate_class(N::get_class()) {
        log::error!("{name} is not a {}", N::get_descriptor());
        return None;
    }
    P::from_binder(b).ok()
}

/// What a task callback says happened.
#[derive(Debug)]
pub enum Event {
    /// Tasks moved, resized or changed in some way.
    StackChanged,
    Created(i32, Option<String>),
    Removed(i32),
    MovedToFront(TaskInfo),
    DescriptionChanged(TaskInfo),
    Focused(i32),
    MovedToBack(TaskInfo),
}

pub trait ITaskStackListener: Interface {
    fn event(&self, e: Event);
}

declare_binder_interface! {
    ITaskStackListener["android.app.ITaskStackListener"] {
        native: BnTaskStackListener(on_transact),
        proxy: BpTaskStackListener,
    }
}

impl ITaskStackListener for BpTaskStackListener {
    fn event(&self, _: Event) {}
}

impl ITaskStackListener for binder::binder_impl::Binder<BnTaskStackListener> {
    fn event(&self, e: Event) {
        self.0.event(e)
    }
}

/// A `Parcel` as the core crate reads it.
struct Reader<'a, 'b>(&'a BorrowedParcel<'b>);

impl Read for Reader<'_, '_> {
    fn int(&mut self) -> std::result::Result<i32, Error> {
        self.0.read().map_err(|_| Error::Malformed)
    }
    fn long(&mut self) -> std::result::Result<i64, Error> {
        self.0.read().map_err(|_| Error::Malformed)
    }
    fn string16(&mut self) -> std::result::Result<Option<String>, Error> {
        self.0.read().map_err(|_| Error::Malformed)
    }
    fn binder(&mut self) -> std::result::Result<(), Error> {
        self.0
            .read::<Option<SpIBinder>>()
            .map(drop)
            .map_err(|_| Error::Malformed)
    }
    fn skip(&mut self, n: usize) -> std::result::Result<(), Error> {
        let pos = self.0.get_data_position() as usize + n;
        if pos > self.0.get_data_size() as usize {
            return Err(Error::Malformed);
        }
        // SAFETY: a position inside the parcel's data.
        unsafe { self.0.set_data_position(pos as i32) }.map_err(|_| Error::Malformed)
    }
}

fn task_info(data: &BorrowedParcel<'_>) -> Wire<TaskInfo> {
    match aim_windows_core::running_task_info(&mut Reader(data)) {
        Ok(Some(info)) => Ok(info),
        Ok(None) => Err(StatusCode::UNEXPECTED_NULL),
        Err(e) => {
            log::warn!("a RunningTaskInfo not read: {e:?}");
            Err(StatusCode::BAD_VALUE)
        }
    }
}

fn on_transact(
    listener: &dyn ITaskStackListener,
    code: TransactionCode,
    data: &BorrowedParcel<'_>,
    _reply: &mut BorrowedParcel<'_>,
) -> Wire<()> {
    let e = match code {
        listener::ON_TASK_STACK_CHANGED => Event::StackChanged,
        listener::ON_TASK_CREATED => {
            let task = data.read()?;
            // `in ComponentName`: written with `writeTypedObject`.
            let package = if data.read::<i32>()? != 0 {
                aim_windows_core::component(&mut Reader(data))
                    .ok()
                    .flatten()
            } else {
                None
            };
            Event::Created(task, package)
        }
        listener::ON_TASK_REMOVED => Event::Removed(data.read()?),
        listener::ON_TASK_MOVED_TO_FRONT => Event::MovedToFront(task_info(data)?),
        listener::ON_TASK_DESCRIPTION_CHANGED => Event::DescriptionChanged(task_info(data)?),
        listener::ON_TASK_MOVED_TO_BACK => Event::MovedToBack(task_info(data)?),
        listener::ON_TASK_FOCUS_CHANGED => {
            let task = data.read()?;
            if !data.read::<bool>()? {
                return Ok(());
            }
            Event::Focused(task)
        }
        // The other callbacks (pinned activities, snapshots, orientation,
        // lock task mode, ...) change nothing the windows show.
        _ => return Ok(()),
    };
    listener.event(e);
    Ok(())
}

pub fn new_listener<T: ITaskStackListener + Sync + Send + 'static>(
    inner: T,
) -> Strong<dyn ITaskStackListener> {
    BnTaskStackListener::new_binder(inner, BinderFeatures::default())
}
