//! `sensorservice`: SensorService for the app host (`android-16.0.0_r1`,
//! `ISensorServer`), as a device without a sensors HAL has it: no sensors
//! (a desktop Mac has none), in `NORMAL` mode. `SensorManager` waits for
//! it when an app first asks for sensors. Event and direct connections and
//! operation parameters are not implemented (#1692).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{PERMISSION_DENIED, Parcel};

const DESCRIPTOR: &str = "android.gui.SensorServer";
/// `ISensorServer`'s calls (`ISensorServer.cpp`), from
/// `IBinder::FIRST_CALL_TRANSACTION`.
const GET_SENSOR_LIST: u32 = 1;
const CREATE_SENSOR_EVENT_CONNECTION: u32 = 2;
const ENABLE_DATA_INJECTION: u32 = 3;
const GET_DYNAMIC_SENSOR_LIST: u32 = 4;
const CREATE_SENSOR_DIRECT_CONNECTION: u32 = 5;
const SET_OPERATION_PARAMETER: u32 = 6;
const GET_RUNTIME_SENSOR_LIST: u32 = 7;
const ENABLE_REPLAY_DATA_INJECTION: u32 = 8;
const ENABLE_HAL_BYPASS_REPLAY_DATA_INJECTION: u32 = 9;
/// `INVALID_OPERATION` (`-ENOSYS`).
const INVALID_OPERATION: i32 = -38;

pub struct SensorService;

impl Service for SensorService {
    fn descriptor(&self) -> &str {
        DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `CHECK_INTERFACE`.
        call.data
            .enforce_interface(DESCRIPTOR)
            .map_err(|_| PERMISSION_DENIED)?;
        let mut p = Parcel::new();
        match call.code {
            // No sensors, whoever asks: the list's count.
            GET_SENSOR_LIST | GET_DYNAMIC_SENSOR_LIST | GET_RUNTIME_SENSOR_LIST => p.write_u32(0),
            // `isDataInjectionEnabled` and the replay modes: the service
            // stays in `NORMAL`.
            ENABLE_DATA_INJECTION
            | ENABLE_REPLAY_DATA_INJECTION
            | ENABLE_HAL_BYPASS_REPLAY_DATA_INJECTION => p.write_i32(0),
            code @ (CREATE_SENSOR_EVENT_CONNECTION
            | CREATE_SENSOR_DIRECT_CONNECTION
            | SET_OPERATION_PARAMETER) => {
                eprintln!("app host: sensorservice#{code} (#1692) not implemented");
                return Err(INVALID_OPERATION);
            }
            code => {
                eprintln!("app host: sensorservice#{code} not known");
                return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);
            }
        }
        Ok(p)
    }
}
