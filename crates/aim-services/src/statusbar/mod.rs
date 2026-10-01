//! The lightweight shell's status bar (`docs/m1-shell.md`, ADR 0013 M1): a
//! native `IStatusBar` of the system uid, registered with the original
//! StatusBarManagerService (`IStatusBarService.registerStatusBar`) in
//! place of SystemUI's, which it replaces: it is started only where
//! SystemUI does not run.
//!
//! What SystemUI shows for it, the app's shim shows on the Mac, through
//! the display server ([`aim_host_display::shell`]):
//! - a text toast (`showToast`/`hideToast`) as a capsule at the bottom of
//!   the app's window; NotificationManagerService keeps queueing and
//!   timing it, and the app's `ITransientNotificationCallback` hears
//!   `onToastShown`/`onToastHidden` when the Mac shows and closes it, as
//!   ToastUI calls them;
//! - a system status icon (`setIcon`/`removeIcon`) in the owning app's
//!   menu bar item;
//! - `BiometricPrompt`'s device credential (`showAuthenticationDialog`) as
//!   a sheet of the app's window, verified through `ILockSettings`
//!   ([`auth`]);
//! - the screen pinning request (`showScreenPinningRequest`) as a sheet;
//!   accepted, the task is pinned (`startSystemLockTaskMode`), as
//!   SystemUI's ScreenPinningRequest does; while a task is pinned
//!   (`showPinningEnterExitToast`), its app's menu bar item unpins it
//!   (`stopSystemLockTaskMode`), as SystemUI's navigation bar does.
//!
//! Every other call is answered as on a device without a notification
//! shade, quick settings or navigation bar: it changes nothing, and a
//! request for a Quick Settings tile is answered as a dismissed dialog
//! (the app hears "not added", decision D8).

mod auth;
mod parcels;
mod toasts;

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use aim_binder_driver::{Credentials, Device, Driver};
use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{Binder, Exception, Parcel, Reader, UNKNOWN_TRANSACTION};
use aim_host_display::shell::{Message, StatusIcon};
use aim_host_display::wire;
use aim_service_aidl::{
    Returned, android_app_iactivitytaskmanager as atm, android_os_iservicemanager as sm,
    com_android_internal_statusbar_iaddtileresultcallback as tile_callback,
    com_android_internal_statusbar_istatusbar as bar,
    com_android_internal_statusbar_istatusbarservice as service,
};

use crate::notifications::{Bridge, ProcessFiles};
use parcels::{Component, Registered, RootTask, Skipped, StatusBarIcon, Text};

/// `Process.SYSTEM_UID` and system_server's context, as the other native
/// services.
const SYSTEM_UID: u32 = 1000;
const SYSTEM_SERVER_CONTEXT: &str = "u:r:system_server:s0";
/// How often it looks for StatusBarManagerService while it is not there.
const RETRY: Duration = Duration::from_secs(1);
/// `StatusBarManager.TILE_ADD_REQUEST_RESULT_DIALOG_DISMISSED`.
const TILE_DIALOG_DISMISSED: i32 = 3;

struct State {
    /// StatusBarManagerService, while registered with it.
    service: Option<Arc<Strong>>,
    /// The toast on screen, and those the Mac has not closed, by id.
    toast: Option<u32>,
    toasts: HashMap<u32, toasts::Sent>,
    next_toast: u32,
    /// The package of each slot's icon.
    icons: HashMap<String, String>,
    auth: Option<auth::Request>,
    /// The pinned task, while there is one.
    pinned: Option<i32>,
}

pub struct StatusBar {
    process: Arc<LocalProcess>,
    node: Binder,
    link: Mutex<UnixStream>,
    state: Mutex<State>,
    /// Signalled when StatusBarManagerService dies.
    gone: Condvar,
}

/// The `IStatusBar` node.
struct Node(Weak<StatusBar>);

impl StatusBar {
    /// Starts the status bar in the background: it connects to the display
    /// server at `display` and registers with StatusBarManagerService once
    /// it is published (again after system_server restarts). Only where
    /// SystemUI does not run: the registration replaces its bar.
    pub fn start(driver: &Arc<Driver>, display: &Path) {
        let driver = driver.clone();
        let display = display.to_owned();
        let _ = std::thread::Builder::new()
            .name("statusbar".into())
            .spawn(move || {
                if let Err(e) = StatusBar::run(&driver, &display) {
                    eprintln!("guest-init: statusbar: {e}");
                }
            });
    }

    fn run(driver: &Arc<Driver>, display: &Path) -> Result<(), String> {
        let link = connect(display)?;
        let process = LocalProcess::open(
            driver,
            Device::Binder,
            Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID,
                security_context: Some(SYSTEM_SERVER_CONTEXT.into()),
            },
        );
        let reader = link.try_clone().map_err(|e| e.to_string())?;
        let this = Arc::new_cyclic(|this: &Weak<StatusBar>| StatusBar {
            node: process.add_service(Arc::new(Node(this.clone()))),
            process: process.clone(),
            link: Mutex::new(link),
            state: Mutex::new(State {
                service: None,
                toast: None,
                toasts: HashMap::new(),
                next_toast: 1,
                icons: HashMap::new(),
                auth: None,
                pinned: None,
            }),
            gone: Condvar::new(),
        });
        process.start();
        let mac = this.clone();
        std::thread::Builder::new()
            .name("statusbar-mac".into())
            .spawn(move || mac.serve_mac(reader))
            .map_err(|e| e.to_string())?;
        this.follow_service();
        Ok(())
    }

    /// Registers with StatusBarManagerService whenever it is published,
    /// until the process ends.
    fn follow_service(self: &Arc<Self>) {
        loop {
            let Some(service) = self.find("statusbar") else {
                std::thread::sleep(RETRY);
                continue;
            };
            let service = Arc::new(service);
            let this = Arc::downgrade(self);
            self.process.link_to_death(
                &service,
                Box::new(move || {
                    if let Some(b) = this.upgrade() {
                        b.service_died();
                    }
                }),
            );
            let mut data = Parcel::new();
            service::RegisterStatusBar {
                callbacks: Some(self.node),
            }
            .write(&mut data);
            let registered = service
                .transact(service::REGISTER_STATUS_BAR, &data, false)
                .map_err(|s| format!("status {s}"))
                .and_then(|reply| {
                    match service::read_register_status_bar_reply::<Registered>(&mut reply.reader())
                    {
                        Ok(Ok(r)) => Ok(r),
                        Ok(Err(e)) => Err(e.message),
                        Err(s) => Err(format!("status {s}")),
                    }
                });
            let icons = match registered {
                Ok(r) => r.map(|r| r.0).unwrap_or_default(),
                Err(e) => {
                    eprintln!("guest-init: statusbar: registerStatusBar: {e}");
                    std::thread::sleep(RETRY);
                    continue;
                }
            };
            self.state.lock().unwrap().service = Some(service);
            // The icons set before: shown as if set now.
            for (slot, icon) in icons {
                self.set_icon(slot, icon);
            }
            let mut state = self.state.lock().unwrap();
            while state.service.is_some() {
                state = self.gone.wait(state).unwrap();
            }
        }
    }

    /// StatusBarManagerService died with system_server: what it asked for
    /// is gone.
    fn service_died(&self) {
        let (toast, icons, request, pinned) = {
            let mut state = self.state.lock().unwrap();
            state.service = None;
            let toast = state.toast.take();
            (
                toast.and_then(|id| Some((id, state.toasts.remove(&id)?.package))),
                std::mem::take(&mut state.icons),
                state.auth.take(),
                state.pinned.take(),
            )
        };
        self.state.lock().unwrap().toasts.clear();
        if let Some((id, package)) = toast {
            self.send(&Message::HideToast { id, package });
        }
        for (slot, package) in icons {
            self.send(&Message::Icon {
                package,
                slot,
                icon: None,
            });
        }
        if let Some(r) = request {
            self.send(&r.dismiss());
        }
        if let Some(task) = pinned {
            self.send(&Message::LockTask {
                task,
                pinned: false,
            });
        }
        self.gone.notify_all();
    }

    /// `name` from servicemanager.
    fn find(&self, name: &str) -> Option<Strong> {
        let mut data = Parcel::new();
        sm::CheckService {
            name: Some(name.into()),
        }
        .write(&mut data);
        let reply = self
            .process
            .transact(0, sm::CHECK_SERVICE, &data, false)
            .ok()?;
        let Ok(Ok(Some(Binder::Handle(h)))) = sm::read_check_service_reply(&mut reply.reader())
        else {
            return None;
        };
        Some(self.process.strong(h))
    }

    /// Calls `code` of service `name` and reads the reply.
    fn call<T>(
        &self,
        name: &str,
        code: u32,
        write: impl FnOnce(&mut Parcel),
        read: impl FnOnce(&mut Reader<'_>) -> aim_binder_host::parcel::Result<Returned<T>>,
    ) -> Result<T, String> {
        let service = self.find(name).ok_or(format!("no {name} service"))?;
        let mut data = Parcel::new();
        write(&mut data);
        let reply = service
            .transact(code, &data, false)
            .map_err(|s| format!("{name}: status {s}"))?;
        read(&mut reply.reader())
            .map_err(|s| format!("{name}: status {s}"))?
            .map_err(|e: Exception| format!("{name}: {}", e.message))
    }

    /// A one-way call to a callback the guest gave.
    fn tell(&self, target: &Strong, code: u32, write: impl FnOnce(&mut Parcel)) {
        let mut data = Parcel::new();
        write(&mut data);
        if let Err(s) = target.transact(code, &data, true) {
            eprintln!("guest-init: statusbar: callback {code}: status {s}");
        }
    }

    fn send(&self, m: &Message) {
        let mut link = self.link.lock().unwrap();
        if let Err(e) = link.write_all(&m.frame()) {
            eprintln!("guest-init: statusbar: display server: {e}");
        }
    }

    fn strong(&self, binder: Option<Binder>) -> Option<Strong> {
        match binder? {
            Binder::Handle(h) => Some(self.process.strong(h)),
            Binder::Local(_) => None,
        }
    }

    fn set_icon(&self, slot: String, icon: StatusBarIcon) {
        let image = icon
            .icon
            .as_ref()
            .and_then(|i| Bridge::mac()?.image(i, icon.user));
        let package = icon.package.clone();
        let previous = self
            .state
            .lock()
            .unwrap()
            .icons
            .insert(slot.clone(), package.clone());
        // A slot taken over by another package leaves the first one's item.
        if let Some(p) = previous.filter(|p| *p != package) {
            self.send(&Message::Icon {
                package: p,
                slot: slot.clone(),
                icon: None,
            });
        }
        self.send(&Message::Icon {
            package,
            slot,
            icon: Some(StatusIcon {
                image,
                level: icon.level,
                number: icon.number,
                description: icon.description.unwrap_or_default(),
                visible: icon.visible,
            }),
        });
    }

    fn remove_icon(&self, slot: String) {
        let package = self.state.lock().unwrap().icons.remove(&slot);
        if let Some(package) = package {
            self.send(&Message::Icon {
                package,
                slot,
                icon: None,
            });
        }
    }

    fn pin_request(&self, task: i32) {
        self.send(&Message::Pin { task });
    }

    /// `showPinningEnterExitToast`: screen pinning started or ended. The
    /// task pinned is the focused one: LockTaskController moves it to the
    /// front before it tells the status bar.
    fn lock_task(&self, entering: bool) {
        let task = entering
            .then(|| {
                self.call(
                    "activity_task",
                    atm::GET_FOCUSED_ROOT_TASK_INFO,
                    |p| atm::GetFocusedRootTaskInfo {}.write(p),
                    atm::read_get_focused_root_task_info_reply::<RootTask>,
                )
                .inspect_err(|e| eprintln!("guest-init: statusbar: the pinned task: {e}"))
                .ok()
                .flatten()
                .map(|t| t.id)
            })
            .flatten();
        eprintln!("guest-init: statusbar: pinning {entering}, task {task:?}");
        let previous = std::mem::replace(&mut self.state.lock().unwrap().pinned, task);
        if let Some(task) = previous {
            self.send(&Message::LockTask {
                task,
                pinned: false,
            });
        }
        if let Some(task) = task {
            self.send(&Message::LockTask { task, pinned: true });
        }
    }

    /// The display server's messages: what the Mac showed and what the
    /// user did.
    fn serve_mac(self: Arc<Self>, mut reader: UnixStream) {
        loop {
            match Message::read(&mut reader) {
                Ok(Some(m)) => self.from_mac(m),
                Ok(None) => return,
                Err(e) => {
                    eprintln!("guest-init: statusbar: display server: {e}");
                    return;
                }
            }
        }
    }

    fn from_mac(self: &Arc<Self>, m: Message) {
        match m {
            Message::ToastShown { id } => self.toast_shown(id),
            Message::ToastHidden { id, shown } => self.toast_hidden(id, shown),
            Message::Secret { id, secret } => self.verify(id, &secret),
            Message::Cancel { id, shown } => self.cancelled(id, shown),
            Message::Pinned { task, accepted } => {
                if !accepted {
                    return;
                }
                let pinned = self.call(
                    "activity_task",
                    atm::START_SYSTEM_LOCK_TASK_MODE,
                    |p| atm::StartSystemLockTaskMode { task_id: task }.write(p),
                    atm::read_start_system_lock_task_mode_reply,
                );
                if let Err(e) = pinned {
                    eprintln!("guest-init: statusbar: pinning task {task}: {e}");
                }
            }
            Message::Unpin { task } => {
                if self.state.lock().unwrap().pinned != Some(task) {
                    return;
                }
                let unpinned = self.call(
                    "activity_task",
                    atm::STOP_SYSTEM_LOCK_TASK_MODE,
                    |p| atm::StopSystemLockTaskMode {}.write(p),
                    atm::read_stop_system_lock_task_mode_reply,
                );
                if let Err(e) = unpinned {
                    eprintln!("guest-init: statusbar: unpinning task {task}: {e}");
                }
            }
            Message::Toast { .. }
            | Message::HideToast { .. }
            | Message::Icon { .. }
            | Message::Authenticate(_)
            | Message::Retry { .. }
            | Message::Dismiss { .. }
            | Message::Pin { .. }
            | Message::LockTask { .. } => {
                eprintln!("guest-init: statusbar: {m:?} is not from the Mac");
            }
        }
    }
}

/// The display server's status bar connection.
fn connect(display: &Path) -> Result<UnixStream, String> {
    let link = UnixStream::connect(display).map_err(|e| format!("{}: {e}", display.display()))?;
    let hello = wire::Request {
        op: wire::OP_SHELL,
        id: wire::VERSION,
        ..Default::default()
    };
    wire::send(link.as_fd(), wire::bytes(&hello), None).map_err(|e| e.to_string())?;
    // The server's mode: toasts and sheets are shown in either.
    wire::recv_record::<u32>(link.as_fd())
        .map_err(|e| e.to_string())?
        .ok_or("the display server closed the connection")?;
    // SO_NOSIGPIPE: a write after the server has gone fails instead.
    let on: libc::c_int = 1;
    // SAFETY: setsockopt on our socket with a local int.
    unsafe {
        libc::setsockopt(
            link.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            (&on as *const libc::c_int).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    Ok(link)
}

impl Service for Node {
    fn descriptor(&self) -> &str {
        bar::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        // Icons' and prompts' bitmaps, the shell commands' pipes.
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(sb) = self.0.upgrade() else {
            return Err(UNKNOWN_TRANSACTION);
        };
        // Only StatusBarManagerService, in system_server, holds it.
        if call.sender_euid != SYSTEM_UID {
            eprintln!(
                "guest-init: statusbar: call {} from uid {} refused",
                call.code, call.sender_euid
            );
            return Ok(Parcel::new());
        }
        let code = call.code;
        let r = &mut call.data;
        let read = (|| -> aim_binder_host::parcel::Result<()> {
            match code {
                bar::SET_ICON => {
                    r.enforce_interface(bar::DESCRIPTOR)?;
                    let slot = r.read_string16()?.unwrap_or_default();
                    if r.read_i32()? != 0 {
                        let icon = StatusBarIcon::read(r, &ProcessFiles(&sb.process))?;
                        sb.set_icon(slot, icon);
                    }
                }
                bar::REMOVE_ICON => {
                    r.enforce_interface(bar::DESCRIPTOR)?;
                    let slot = r.read_string16()?.unwrap_or_default();
                    sb.remove_icon(slot);
                }
                bar::SHOW_TOAST => sb.show_toast(bar::ShowToast::<Text>::read(r)?),
                bar::HIDE_TOAST => sb.hide_toast(bar::HideToast::read(r)?),
                bar::SHOW_AUTHENTICATION_DIALOG => {
                    sb.show_authentication(bar::ShowAuthenticationDialog::read(r)?)
                }
                bar::HIDE_AUTHENTICATION_DIALOG => {
                    sb.hide_authentication(bar::HideAuthenticationDialog::read(r)?.request_id)
                }
                bar::SHOW_SCREEN_PINNING_REQUEST => {
                    sb.pin_request(bar::ShowScreenPinningRequest::read(r)?.task_id)
                }
                bar::SHOW_PINNING_ENTER_EXIT_TOAST => {
                    sb.lock_task(bar::ShowPinningEnterExitToast::read(r)?.entering)
                }
                bar::REQUEST_ADD_TILE => {
                    let args = bar::RequestAddTile::<Component, Text, Skipped>::read(r)?;
                    if let Some(c) = sb.strong(args.callback) {
                        sb.tell(&c, tile_callback::ON_TILE_REQUEST, |p| {
                            tile_callback::OnTileRequest {
                                user_response: TILE_DIALOG_DISMISSED,
                            }
                            .write(p)
                        });
                    }
                }
                // The shade, quick settings, navigation and task bars, recents,
                // system bar appearance, the keyguard's and the IME's bar
                // state, assistant and gesture hints, biometric sensor state
                // (no sensors), media tap-to-transfer, shell commands: none of
                // them exist here, so nothing changes (one-way calls).
                _ => {}
            }
            Ok(())
        })();
        // One-way: the caller hears nothing of a call that cannot be read.
        if let Err(s) = read {
            eprintln!("guest-init: statusbar: call {code}: status {s}");
        }
        Ok(Parcel::new())
    }
}
