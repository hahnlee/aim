//! `aim-keyboard layout NAME`: the built-in keyboard (the display server's
//! `aim-keyboard`) gets layout NAME, one of the image's InputDevices
//! layouts (`keyboard_layout_english_us_dvorak`), as its layout override
//! in InputManager (`KeyboardLayoutManager`), which applies it as the
//! keyboard's character map overlay (docs/input.md, "Layouts").
//!
//! init runs it with the Mac's layout (`vendor.aim.mac.keyboard_layout`,
//! docs/mac-settings.md), as a device's vendor would set its keyboard's
//! layout. `IInputManager` is a Java AIDL interface with no NDK or Rust
//! backend: the one call is written by hand with the transaction code of
//! the pinned image's AIDL (`android-16.0.0_r1`, first method = 1; the
//! image's `framework.jar` stub has the same `TRANSACTION_*` value).

use binder::binder_impl::{
    AssociateClass, BorrowedParcel, IBinderInternal, Remotable, TransactionCode,
};
use binder::{Interface, Status, StatusCode, declare_binder_interface};
use sha1::{Digest, Sha1};

/// `IInputManager.setKeyboardLayoutOverrideForInputDevice`.
const SET_KEYBOARD_LAYOUT_OVERRIDE: u32 = 19;
/// The InputDevices package's layouts are `package/receiver/name`.
const LAYOUTS: &str = "com.android.inputdevices/com.android.inputdevices.InputDeviceReceiver/";
/// The display server's keyboard (`aim_host_display::input::keyboard`).
const KEYBOARD: &str = "aim-keyboard";

pub trait IInputManager: Interface {}

fn unused(
    _: &dyn IInputManager,
    _: TransactionCode,
    _: &BorrowedParcel<'_>,
    _: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    Err(StatusCode::UNKNOWN_TRANSACTION)
}

declare_binder_interface! {
    IInputManager["android.hardware.input.IInputManager"] {
        native: BnInputManager(unused),
        proxy: BpInputManager,
    }
}

impl IInputManager for BpInputManager {}
impl IInputManager for binder::binder_impl::Binder<BnInputManager> {}

/// The descriptor EventHub gives the built-in input device `name`
/// (vendor and product 0): the SHA-1 of `:0000:0000:name:NAME`
/// (`generateDescriptor`), which it keeps stable across releases.
fn descriptor(name: &str) -> String {
    Sha1::digest(format!(":0000:0000:name:{name}"))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether `name` can be a layout's resource name.
fn is_layout(name: &str) -> bool {
    name.starts_with("keyboard_layout_")
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Set `layout` for the keyboard: the `InputDeviceIdentifier` (its
/// descriptor, vendor and product) and the layout's descriptor.
fn set_layout(layout: &str) -> Result<(), Status> {
    let mut input = binder::wait_for_service("input").ok_or(StatusCode::NAME_NOT_FOUND)?;
    if !input.associate_class(BnInputManager::get_class()) {
        return Err(StatusCode::BAD_TYPE.into());
    }
    let mut data = input.prepare_transact()?;
    let mut p = data.borrowed();
    p.write(&1i32)?;
    p.write(descriptor(KEYBOARD).as_str())?;
    p.write(&0i32)?;
    p.write(&0i32)?;
    p.write(format!("{LAYOUTS}{layout}").as_str())?;
    let reply = input.submit_transact(SET_KEYBOARD_LAYOUT_OVERRIDE, data, 0)?;
    let status: Status = reply.read()?;
    if status.is_ok() { Ok(()) } else { Err(status) }
}

fn main() {
    daemon_log::init("aim-keyboard");
    let args: Vec<String> = std::env::args().collect();
    let [_, command, layout] = &args[..] else {
        log::error!("usage: aim-keyboard layout NAME");
        std::process::exit(2);
    };
    if command != "layout" || !is_layout(layout) {
        log::error!("usage: aim-keyboard layout NAME (keyboard_layout_...)");
        std::process::exit(2);
    }
    if let Err(e) = set_layout(layout) {
        log::error!("{layout}: {e}");
        std::process::exit(1);
    }
    log::info!("{KEYBOARD}: {layout}");
}
