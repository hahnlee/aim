//! `input`: InputManager for the app host, what an app's process asks of
//! it (`InputManagerService` at `android-16.0.0_r1`).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Parcel};
use aim_service_aidl::android_hardware_input_iinputmanager as im;

use super::{AppHost, reply};

/// `DeviceConfig.NAMESPACE_INPUT_NATIVE_BOOT`,
/// `InputManagerService.VELOCITYTRACKER_STRATEGY_PROPERTY`.
const NAMESPACE_INPUT_NATIVE_BOOT: &str = "input_native_boot";
const VELOCITYTRACKER_STRATEGY: &str = "velocitytracker_strategy";

pub struct InputManager(pub Arc<AppHost>);

impl Service for InputManager {
    fn descriptor(&self) -> &str {
        im::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            im::GET_VELOCITY_TRACKER_STRATEGY => {
                im::GetVelocityTrackerStrategy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // The device config's, as the original read it at its start.
                let inputs = match self.0.inputs() {
                    Ok(inputs) => inputs,
                    Err(e) => return reply(Err(e)),
                };
                let strategy = inputs.settings.device_config(NAMESPACE_INPUT_NATIVE_BOOT, VELOCITYTRACKER_STRATEGY);
                im::write_get_velocity_tracker_strategy_reply(&mut p, &strategy.map(str::to_string));
            }
            code => return reply(Err(self.0.unimplemented("input", code))),
        }
        Ok(p)
    }
}
