//! `android.hardware.bluetooth-service.aim`: the `IBluetoothHci/default`
//! vendor HAL of the derived image (ADR 0012, `docs/bluetooth.md`).
//!
//! It passes HCI packets between the Android Bluetooth stack and the
//! host-call module `bluetooth`, a virtual LE controller over CoreBluetooth.
//! Packets toward the stack wait in the host; a reader thread polls the fd
//! the host returned, takes them, and hands them to the stack's callbacks.

mod service;

use aim_hostcall::{bluetooth, guest, module};
use android_hardware_bluetooth::aidl::android::hardware::bluetooth::IBluetoothHci::BnBluetoothHci;
use binder::BinderFeatures;

const INSTANCE: &str = "android.hardware.bluetooth.IBluetoothHci/default";

fn main() {
    match guest::version(module::BLUETOOTH) {
        Ok(v) if v >= bluetooth::VERSION => {}
        other => {
            eprintln!("bluetooth: host module `bluetooth` unavailable or too old: {other:?}");
            std::process::exit(1);
        }
    }
    binder::ProcessState::set_thread_pool_max_thread_count(0);
    let hci = BnBluetoothHci::new_binder(service::Hci::default(), BinderFeatures::default());
    if let Err(e) = binder::add_service(INSTANCE, hci.as_binder()) {
        eprintln!("bluetooth: cannot register {INSTANCE}: {e:?}");
        std::process::exit(1);
    }
    eprintln!("bluetooth: registered {INSTANCE}");
    binder::ProcessState::join_thread_pool();
}
