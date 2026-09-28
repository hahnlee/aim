//! `IThermal` over the host's thermal state. Android semantics stay here;
//! the host module only reports what macOS knows
//! ([`aim_hostcall::thermal`]).
//!
//! - macOS summarizes its thermal pressure in one state, which is the
//!   throttling status of the `SKIN` temperature, the one the framework
//!   watches. The Mac has no skin sensor we can read, so its value is NaN,
//!   as `Temperature` specifies for an unavailable value.
//! - `CPU` (the hottest die sensor) and `BATTERY` report real values when
//!   the host has them, with no throttling status of their own.
//! - There are no thresholds (macOS keeps its own) and no cooling devices
//!   (fans are the SMC's); forecasting is unsupported, as in AOSP's default
//!   implementation.

use std::sync::{Arc, Mutex};

use aim_hostcall::guest;
use aim_hostcall::thermal::{self, state};
use android_hardware_thermal::aidl::android::hardware::thermal::{
    CoolingDevice::CoolingDevice, CoolingType::CoolingType,
    ICoolingDeviceChangedCallback::ICoolingDeviceChangedCallback, IThermal::IThermal,
    IThermalChangedCallback::IThermalChangedCallback, Temperature::Temperature,
    TemperatureThreshold::TemperatureThreshold, TemperatureType::TemperatureType,
    ThrottlingSeverity::ThrottlingSeverity,
};
use binder::{ExceptionCode, Interface, Status, Strong};

fn exception<T>(code: ExceptionCode) -> binder::Result<T> {
    Err(Status::new_exception(code, None))
}

/// The throttling status Android gives macOS's thermal state.
pub fn severity(s: u32) -> ThrottlingSeverity {
    match s {
        state::NOMINAL => ThrottlingSeverity::NONE,
        state::FAIR => ThrottlingSeverity::LIGHT,
        state::SERIOUS => ThrottlingSeverity::SEVERE,
        _ => ThrottlingSeverity::CRITICAL,
    }
}

/// The temperatures of one host reading.
pub fn temperatures(t: &thermal::Thermal) -> Vec<Temperature> {
    let mut list = vec![Temperature {
        r#type: TemperatureType::SKIN,
        name: "skin".into(),
        value: f32::NAN,
        throttlingStatus: severity(t.state),
    }];
    for (r#type, name, value) in [
        (TemperatureType::CPU, "cpu", t.cpu_celsius),
        (TemperatureType::BATTERY, "battery", t.battery_celsius),
    ] {
        if !value.is_nan() {
            list.push(Temperature {
                r#type,
                name: name.into(),
                value,
                throttlingStatus: ThrottlingSeverity::NONE,
            });
        }
    }
    list
}

fn read() -> binder::Result<Vec<Temperature>> {
    let t = guest::thermal().map_err(|e| {
        log::error!("host call thermal.read failed: errno {}", e.0);
        Status::new_exception(ExceptionCode::ILLEGAL_STATE, None)
    })?;
    Ok(temperatures(&t))
}

struct Callback {
    callback: Strong<dyn IThermalChangedCallback>,
    /// Only temperatures of this type, or all.
    filter: Option<TemperatureType>,
}

#[derive(Default)]
struct State {
    callbacks: Vec<Callback>,
    /// The last throttling status sent.
    status: Option<ThrottlingSeverity>,
}

#[derive(Clone, Default)]
pub struct Thermal {
    state: Arc<Mutex<State>>,
}

impl Interface for Thermal {}

impl Thermal {
    /// Tell the callbacks when the host's thermal state has changed,
    /// dropping the ones whose process has died.
    pub fn poll(&self) {
        let Ok(list) = read() else { return };
        let skin = &list[0];
        let mut state = self.state.lock().unwrap();
        if state.status == Some(skin.throttlingStatus) {
            return;
        }
        log::info!("throttling status now {}", skin.throttlingStatus.0);
        state.status = Some(skin.throttlingStatus);
        state.callbacks.retain(|c| {
            c.filter.is_some_and(|f| f != skin.r#type)
                || !matches!(c.callback.notifyThrottling(skin),
                    Err(e) if e.transaction_error() == binder::StatusCode::DEAD_OBJECT)
        });
    }

    fn register(
        &self,
        callback: &Strong<dyn IThermalChangedCallback>,
        filter: Option<TemperatureType>,
    ) -> binder::Result<()> {
        let mut state = self.state.lock().unwrap();
        if state
            .callbacks
            .iter()
            .any(|c| c.callback.as_binder() == callback.as_binder())
        {
            return exception(ExceptionCode::ILLEGAL_ARGUMENT);
        }
        state.callbacks.push(Callback {
            callback: callback.clone(),
            filter,
        });
        Ok(())
    }
}

impl IThermal for Thermal {
    fn getCoolingDevices(&self) -> binder::Result<Vec<CoolingDevice>> {
        Ok(Vec::new())
    }

    fn getCoolingDevicesWithType(&self, _type: CoolingType) -> binder::Result<Vec<CoolingDevice>> {
        Ok(Vec::new())
    }

    fn getTemperatures(&self) -> binder::Result<Vec<Temperature>> {
        read()
    }

    fn getTemperaturesWithType(&self, r#type: TemperatureType) -> binder::Result<Vec<Temperature>> {
        Ok(read()?.into_iter().filter(|t| t.r#type == r#type).collect())
    }

    fn getTemperatureThresholds(&self) -> binder::Result<Vec<TemperatureThreshold>> {
        Ok(Vec::new())
    }

    fn getTemperatureThresholdsWithType(
        &self,
        _type: TemperatureType,
    ) -> binder::Result<Vec<TemperatureThreshold>> {
        Ok(Vec::new())
    }

    fn registerThermalChangedCallback(
        &self,
        callback: &Strong<dyn IThermalChangedCallback>,
    ) -> binder::Result<()> {
        self.register(callback, None)
    }

    fn registerThermalChangedCallbackWithType(
        &self,
        callback: &Strong<dyn IThermalChangedCallback>,
        r#type: TemperatureType,
    ) -> binder::Result<()> {
        self.register(callback, Some(r#type))
    }

    fn unregisterThermalChangedCallback(
        &self,
        callback: &Strong<dyn IThermalChangedCallback>,
    ) -> binder::Result<()> {
        let mut state = self.state.lock().unwrap();
        let before = state.callbacks.len();
        state
            .callbacks
            .retain(|c| c.callback.as_binder() != callback.as_binder());
        if state.callbacks.len() == before {
            return exception(ExceptionCode::ILLEGAL_ARGUMENT);
        }
        Ok(())
    }

    // No cooling devices, so they never change.
    fn registerCoolingDeviceChangedCallbackWithType(
        &self,
        _callback: &Strong<dyn ICoolingDeviceChangedCallback>,
        _type: CoolingType,
    ) -> binder::Result<()> {
        Ok(())
    }

    fn unregisterCoolingDeviceChangedCallback(
        &self,
        _callback: &Strong<dyn ICoolingDeviceChangedCallback>,
    ) -> binder::Result<()> {
        Ok(())
    }

    fn forecastSkinTemperature(&self, _forecast_seconds: i32) -> binder::Result<f32> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thermal_states_map_to_rising_severities() {
        let s: Vec<_> = (0..=4).map(severity).collect();
        assert_eq!(
            s,
            [
                ThrottlingSeverity::NONE,
                ThrottlingSeverity::LIGHT,
                ThrottlingSeverity::SEVERE,
                ThrottlingSeverity::CRITICAL,
                ThrottlingSeverity::CRITICAL,
            ]
        );
    }

    #[test]
    fn skin_carries_the_status_and_unknown_sensors_are_left_out() {
        let list = temperatures(&thermal::Thermal {
            state: state::SERIOUS,
            cpu_celsius: 61.5,
            battery_celsius: f32::NAN,
        });
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].r#type, TemperatureType::SKIN);
        assert!(list[0].value.is_nan());
        assert_eq!(list[0].throttlingStatus, ThrottlingSeverity::SEVERE);
        assert_eq!(list[1].r#type, TemperatureType::CPU);
        assert_eq!(list[1].value, 61.5);
        assert_eq!(list[1].throttlingStatus, ThrottlingSeverity::NONE);
    }
}
