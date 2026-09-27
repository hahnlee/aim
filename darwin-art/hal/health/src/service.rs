//! `IHealth` over the host's battery. Android semantics stay here; the host
//! module only reports what macOS knows ([`darwin_hostcall::health`]).
//! What the Mac cannot report (storage, disk stats, charging policy,
//! battery health data, hinges) is `UNSUPPORTED_OPERATION`, as in AOSP's
//! default implementation on a device without the sysfs node.

use std::sync::{Arc, Mutex};

use android_hardware_health::aidl::android::hardware::health::{
    BatteryCapacityLevel::BatteryCapacityLevel, BatteryChargingPolicy::BatteryChargingPolicy,
    BatteryHealth::BatteryHealth, BatteryHealthData::BatteryHealthData,
    BatteryStatus::BatteryStatus, DiskStats::DiskStats, HealthInfo::HealthInfo,
    HingeInfo::HingeInfo, IHealth::IHealth, IHealthInfoCallback::IHealthInfoCallback,
    StorageInfo::StorageInfo,
};
use binder::{ExceptionCode, Interface, Status, Strong};
use darwin_hostcall::guest;
use darwin_hostcall::health::Battery;

fn unsupported<T>() -> binder::Result<T> {
    Err(Status::new_exception(
        ExceptionCode::UNSUPPORTED_OPERATION,
        None,
    ))
}

/// The host's battery as `HealthInfo`.
pub fn health_info() -> binder::Result<HealthInfo> {
    let b = guest::battery().map_err(|e| {
        log::error!("host call health.battery failed: errno {}", e.0);
        Status::new_exception(ExceptionCode::ILLEGAL_STATE, None)
    })?;
    Ok(to_health_info(&b))
}

fn to_health_info(b: &Battery) -> HealthInfo {
    let present = b.present != 0;
    HealthInfo {
        chargerAcOnline: b.ac_online != 0,
        batteryStatus: BatteryStatus(b.status as i32),
        batteryHealth: BatteryHealth(b.health as i32),
        batteryPresent: present,
        batteryLevel: b.level_percent,
        batteryVoltageMillivolts: b.voltage_millivolts,
        batteryTemperatureTenthsCelsius: b.temperature_tenths_celsius,
        batteryCurrentMicroamps: b.current_microamps,
        batteryCurrentAverageMicroamps: b.current_microamps,
        batteryCycleCount: b.cycle_count,
        batteryFullChargeUah: b.full_charge_uah,
        batteryChargeCounterUah: b.charge_counter_uah,
        batteryFullChargeDesignCapacityUah: b.full_charge_design_uah,
        batteryTechnology: if present {
            "Li-ion".into()
        } else {
            String::new()
        },
        batteryCapacityLevel: BatteryCapacityLevel::UNSUPPORTED,
        batteryChargeTimeToFullNowSeconds: b.time_to_full_seconds,
        ..Default::default()
    }
}

#[derive(Clone, Default)]
pub struct Health {
    callbacks: Arc<Mutex<Vec<Strong<dyn IHealthInfoCallback>>>>,
}

impl Interface for Health {}

impl Health {
    /// Send the current state to every registered callback, dropping the
    /// ones whose process has died.
    pub fn notify(&self) {
        let Ok(info) = health_info() else { return };
        self.callbacks.lock().unwrap().retain(|cb| {
            !matches!(cb.healthInfoChanged(&info),
                Err(e) if e.transaction_error() == binder::StatusCode::DEAD_OBJECT)
        });
    }
}

impl IHealth for Health {
    fn registerCallback(&self, callback: &Strong<dyn IHealthInfoCallback>) -> binder::Result<()> {
        self.callbacks.lock().unwrap().push(callback.clone());
        if let Ok(info) = health_info() {
            let _ = callback.healthInfoChanged(&info);
        }
        Ok(())
    }

    fn unregisterCallback(&self, callback: &Strong<dyn IHealthInfoCallback>) -> binder::Result<()> {
        let mut callbacks = self.callbacks.lock().unwrap();
        let before = callbacks.len();
        callbacks.retain(|cb| cb.as_binder() != callback.as_binder());
        if callbacks.len() == before {
            return Err(Status::new_exception(ExceptionCode::ILLEGAL_ARGUMENT, None));
        }
        Ok(())
    }

    fn update(&self) -> binder::Result<()> {
        self.notify();
        Ok(())
    }

    fn getChargeCounterUah(&self) -> binder::Result<i32> {
        match health_info()?.batteryChargeCounterUah {
            0 => unsupported(),
            v => Ok(v),
        }
    }

    fn getCurrentNowMicroamps(&self) -> binder::Result<i32> {
        Ok(health_info()?.batteryCurrentMicroamps)
    }

    fn getCurrentAverageMicroamps(&self) -> binder::Result<i32> {
        Ok(health_info()?.batteryCurrentAverageMicroamps)
    }

    fn getCapacity(&self) -> binder::Result<i32> {
        Ok(health_info()?.batteryLevel)
    }

    fn getEnergyCounterNwh(&self) -> binder::Result<i64> {
        unsupported()
    }

    fn getChargeStatus(&self) -> binder::Result<BatteryStatus> {
        Ok(health_info()?.batteryStatus)
    }

    fn getStorageInfo(&self) -> binder::Result<Vec<StorageInfo>> {
        unsupported()
    }

    fn getDiskStats(&self) -> binder::Result<Vec<DiskStats>> {
        unsupported()
    }

    fn getHealthInfo(&self) -> binder::Result<HealthInfo> {
        health_info()
    }

    fn setChargingPolicy(&self, _policy: BatteryChargingPolicy) -> binder::Result<()> {
        unsupported()
    }

    fn getChargingPolicy(&self) -> binder::Result<BatteryChargingPolicy> {
        unsupported()
    }

    fn getBatteryHealthData(&self) -> binder::Result<BatteryHealthData> {
        unsupported()
    }

    fn getHingeInfo(&self) -> binder::Result<Vec<HingeInfo>> {
        unsupported()
    }
}
