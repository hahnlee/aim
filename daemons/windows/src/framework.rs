//! The framework's task and window interfaces, as a client, and the task
//! listener it calls back.
//!
//! `IActivityTaskManager`, `IWindowManager` and `ITaskStackListener` are
//! Java AIDL interfaces of the platform, with no NDK or Rust backend: their
//! methods take Java-only parcelables. The few calls the bridge makes are
//! encoded here over AOSP's `binder::Parcel`, using transaction tables
//! generated from the pinned AIDL and checked against `framework.jar`.
//! A class associated with each remote binder writes the interface token,
//! as a generated proxy's does.

use aim_framework_aidl::{
    android_app_iactivitytaskmanager as atm, android_app_itaskstacklistener as listener,
    android_view_iwindowmanager as wm,
};
use aim_windows_core::{Error, Orientation, Read, TaskInfo, write};
use binder::binder_impl::{
    AssociateClass, BorrowedParcel, IBinderInternal, Proxy, Remotable, TransactionCode,
};
use binder::{
    BinderFeatures, ExceptionCode, Interface, SpIBinder, Status, StatusCode, Strong,
    declare_binder_interface,
};

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
pub(crate) fn unused<I: ?Sized>(
    _: &I,
    _: TransactionCode,
    _: &BorrowedParcel<'_>,
    _: &mut BorrowedParcel<'_>,
) -> Wire<()> {
    Err(StatusCode::UNKNOWN_TRANSACTION)
}

declare_binder_interface! {
    IActivityTaskManager[atm::DESCRIPTOR] {
        native: BnActivityTaskManager(unused),
        proxy: BpActivityTaskManager,
    }
}

declare_binder_interface! {
    IWindowManager[wm::DESCRIPTOR] {
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
pub(crate) fn call(
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

/// An `Intent` that starts launcher activity `class` of `package`, as
/// `Intent.writeToParcel` writes it after its typed-object marker.
fn write_launch_intent(p: &mut BorrowedParcel<'_>, package: &str, class: &str) -> Wire<()> {
    write_string8(p, Some("android.intent.action.MAIN"))?;
    p.write(&0i32)?; // no Uri
    write_string8(p, None)?; // type
    write_string8(p, None)?; // identifier
    p.write(&LAUNCH_FLAGS)?;
    p.write(&0i32)?; // extended flags
    write_string8(p, Some(package))?;
    p.write(package)?; // the component: package and class
    p.write(class)?;
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

    /// Start launcher activity `class` of `package` as a launcher would,
    /// and wait until it has drawn its first frame (or the launch ended),
    /// as `am start -W` does; the `ActivityManager.START_*` result.
    /// `bounds` are the launch bounds (`ActivityOptions.setLaunchBounds`),
    /// `windowing_mode` the launch windowing mode.
    pub fn start_activity_and_wait(
        &self,
        package: &str,
        class: &str,
        bounds: Option<[i32; 4]>,
        windowing_mode: Option<i32>,
    ) -> Result<i32> {
        let reply = call(&self.binder, atm::START_ACTIVITY_AND_WAIT, |p| {
            p.write(&None::<SpIBinder>)?; // caller
            p.write(&"android")?; // callingPackage
            p.write(&None::<String>)?; // callingFeatureId
            p.write(&1i32)?;
            write_launch_intent(p, package, class)?;
            p.write(&None::<String>)?; // resolvedType
            p.write(&None::<SpIBinder>)?; // resultTo
            p.write(&None::<String>)?; // resultWho
            p.write(&0i32)?; // requestCode
            p.write(&0i32)?; // flags
            p.write(&0i32)?; // no ProfilerInfo
            if bounds.is_some() || windowing_mode.is_some() {
                p.write(&1i32)?;
                write::launch_options(bounds, windowing_mode)
                    .iter()
                    .try_for_each(|w| p.write(w))?;
            } else {
                p.write(&0i32)?; // no options
            }
            p.write(&USER_CURRENT)
        })?;
        // The `WaitResult`, as `writeTypedObject` writes it: its result first.
        if reply.read::<i32>()? == 0 {
            return Err(Status::new_exception(ExceptionCode::ILLEGAL_STATE, None));
        }
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
pub(crate) fn connect<N: Remotable, P: Proxy>(name: &str) -> Option<P> {
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
    /// An activity of the task asked for an orientation.
    Orientation(i32, Orientation),
}

pub trait ITaskStackListener: Interface {
    fn event(&self, e: Event);
}

declare_binder_interface! {
    ITaskStackListener[listener::DESCRIPTOR] {
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
                    .map(|(package, _)| package)
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
        listener::ON_ACTIVITY_REQUESTED_ORIENTATION_CHANGED => {
            let task = data.read()?;
            Event::Orientation(task, Orientation::from_screen_orientation(data.read()?))
        }
        // The other callbacks (pinned activities, snapshots, lock task
        // mode, ...) change nothing the windows show.
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

#[cfg(test)]
mod tests {
    use super::*;
    use binder::binder_impl::Parcel;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Listener(Mutex<Vec<Event>>);

    impl Interface for Listener {}
    impl ITaskStackListener for Listener {
        fn event(&self, event: Event) {
            self.0.lock().unwrap().push(event);
        }
    }

    fn dispatch(listener: &Listener, code: u32, words: &[i32]) -> Wire<()> {
        let mut data = Parcel::new();
        for word in words {
            data.borrowed().write(word)?;
        }
        // SAFETY: rewind to the start of the initialized parcel.
        unsafe { data.borrowed().set_data_position(0) }?;
        on_transact(
            listener,
            code,
            data.borrowed_ref(),
            &mut Parcel::new().borrowed(),
        )
    }

    // Run against the image's libbinder_ndk with the NDK guest test harness.
    #[test]
    fn generated_listener_codes_dispatch_parcels() {
        let listener = Listener::default();
        dispatch(&listener, listener::ON_TASK_CREATED, &[42, 0]).unwrap();
        dispatch(&listener, listener::ON_TASK_FOCUS_CHANGED, &[42, 0]).unwrap();
        dispatch(&listener, listener::ON_TASK_FOCUS_CHANGED, &[42, 1]).unwrap();
        dispatch(&listener, listener::ON_TASK_REMOVED, &[42]).unwrap();
        assert!(matches!(
            &listener.0.lock().unwrap()[..],
            [
                Event::Created(42, None),
                Event::Focused(42),
                Event::Removed(42)
            ]
        ));
    }

    #[test]
    fn malformed_listener_parcel_returns_the_ndk_error() {
        let listener = Listener::default();
        assert_eq!(
            dispatch(&listener, listener::ON_TASK_FOCUS_CHANGED, &[42]),
            Err(StatusCode::NOT_ENOUGH_DATA)
        );
        assert!(listener.0.lock().unwrap().is_empty());
    }
}
