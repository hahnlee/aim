//! `ISensors` over the Mac's few readable sensors
//! ([`aim_hostcall::sensors`]): the ambient light sensor and, on
//! MacBooks, the lid angle as a hinge angle. Each is listed only when the
//! host has it, so a Mac without them has an empty list, which Android
//! handles. There is no motion sensor to offer.
//!
//! Both are on-change sensors: a poller reads the host while one is active
//! and writes an event to sensorservice's queue when a value changes, and
//! once on activation, as on-change sensors must. Neither is a wake-up
//! sensor, so the wake-lock queue goes unused. Direct channels and data
//! injection are unsupported, as for a HAL without them.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aim_hostcall::guest;
use aim_hostcall::sensors::{Readings, present};
use android_hardware_common_fmq::aidl::android::hardware::common::fmq::{
    MQDescriptor::MQDescriptor, SynchronizedReadWrite::SynchronizedReadWrite,
};
use android_hardware_sensors::aidl::android::hardware::sensors::{
    Event::Event,
    Event::EventPayload::EventPayload,
    Event::EventPayload::MetaData::MetaData,
    Event::EventPayload::MetaData::MetaDataEventType::MetaDataEventType,
    ISensors::ISensors,
    ISensors::{
        EVENT_QUEUE_FLAG_BITS_READ_AND_PROCESS, OperationMode::OperationMode, RateLevel::RateLevel,
        SharedMemInfo::SharedMemInfo,
    },
    ISensorsCallback::ISensorsCallback,
    SensorInfo::{SENSOR_FLAG_BITS_ON_CHANGE_MODE, SensorInfo},
    SensorType::SensorType,
};
use binder::{ExceptionCode, Interface, Status, Strong};

use crate::fmq;

/// The fastest the host is polled: its sensors change slowly, and the lid
/// report is a device request.
pub const MIN_PERIOD: Duration = Duration::from_millis(200);
const MAX_PERIOD: Duration = Duration::from_secs(1);

fn exception<T>(code: ExceptionCode) -> binder::Result<T> {
    Err(Status::new_exception(code, None))
}

/// The sensors a host with `present` sensors has.
pub fn sensor_list(present: u32) -> Vec<SensorInfo> {
    let info = |handle, r#type, name: &str, type_name: &str, max_range, resolution| SensorInfo {
        sensorHandle: handle,
        name: name.into(),
        vendor: "Apple (darwin host)".into(),
        version: 1,
        r#type,
        typeAsString: type_name.into(),
        maxRange: max_range,
        resolution,
        power: 0.1,
        minDelayUs: MIN_PERIOD.as_micros() as i32,
        fifoReservedEventCount: 0,
        fifoMaxEventCount: 0,
        requiredPermission: String::new(),
        maxDelayUs: MAX_PERIOD.as_micros() as i32,
        flags: SENSOR_FLAG_BITS_ON_CHANGE_MODE,
    };
    let mut list = Vec::new();
    if present & present::LIGHT != 0 {
        list.push(info(
            HANDLE_LIGHT,
            SensorType::LIGHT,
            "Ambient Light Sensor",
            "android.sensor.light",
            100_000.0,
            1.0,
        ));
    }
    if present & present::HINGE != 0 {
        list.push(info(
            HANDLE_HINGE,
            SensorType::HINGE_ANGLE,
            "Lid Angle Sensor",
            "android.sensor.hinge_angle",
            360.0,
            1.0,
        ));
    }
    list
}

const HANDLE_LIGHT: i32 = 1;
const HANDLE_HINGE: i32 = 2;

/// Sensor `handle`'s value in a host reading.
fn value(handle: i32, r: &Readings) -> f32 {
    if handle == HANDLE_LIGHT {
        r.light_lux
    } else {
        r.hinge_degrees
    }
}

fn event(handle: i32, r#type: SensorType, payload: EventPayload) -> Event {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid clock and output. Sensor timestamps are
    // elapsedRealtimeNanos, i.e. CLOCK_BOOTTIME.
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    Event {
        timestamp: ts.tv_sec * 1_000_000_000 + ts.tv_nsec,
        sensorHandle: handle,
        sensorType: r#type,
        payload,
    }
}

struct Sensor {
    handle: i32,
    r#type: SensorType,
    active: bool,
    period: Duration,
    /// The value last reported since activation.
    last: Option<f32>,
}

struct State {
    /// The host's sensors ([`present`] bits).
    present: u32,
    sensors: Vec<Sensor>,
    queue: Option<fmq::Writer<Event>>,
}

impl State {
    fn sensor(&mut self, handle: i32) -> binder::Result<&mut Sensor> {
        match self.sensors.iter_mut().find(|s| s.handle == handle) {
            Some(s) => Ok(s),
            None => exception(ExceptionCode::ILLEGAL_ARGUMENT),
        }
    }

    fn send(&self, events: &[Event]) {
        let Some(q) = &self.queue else { return };
        if !q.write(events) {
            log::warn!("event queue full; dropped {} events", events.len());
        }
        q.wake(EVENT_QUEUE_FLAG_BITS_READ_AND_PROCESS as u32);
    }
}

#[derive(Clone)]
pub struct Sensors {
    state: Arc<Mutex<State>>,
}

impl Interface for Sensors {}

impl Sensors {
    pub fn new(present: u32) -> Self {
        let sensors = sensor_list(present)
            .into_iter()
            .map(|info| Sensor {
                handle: info.sensorHandle,
                r#type: info.r#type,
                active: false,
                period: MAX_PERIOD,
                last: None,
            })
            .collect();
        Self {
            state: Arc::new(Mutex::new(State {
                present,
                sensors,
                queue: None,
            })),
        }
    }

    /// Reads the host once if a sensor is active and reports what changed.
    /// Returns how long to wait before the next poll.
    pub fn poll(&self) -> Duration {
        let mut state = self.state.lock().unwrap();
        let Some(period) = state
            .sensors
            .iter()
            .filter(|s| s.active)
            .map(|s| s.period)
            .min()
        else {
            return MAX_PERIOD;
        };
        let Ok(r) = guest::sensors() else {
            return period;
        };
        let mut events = Vec::new();
        for s in state.sensors.iter_mut().filter(|s| s.active) {
            let v = value(s.handle, &r);
            if s.last != Some(v) {
                s.last = Some(v);
                events.push(event(s.handle, s.r#type, EventPayload::Scalar(v)));
            }
        }
        if !events.is_empty() {
            state.send(&events);
        }
        period
    }
}

impl ISensors for Sensors {
    fn activate(&self, handle: i32, enabled: bool) -> binder::Result<()> {
        let mut state = self.state.lock().unwrap();
        let s = state.sensor(handle)?;
        s.active = enabled;
        s.last = None;
        Ok(())
    }

    fn batch(
        &self,
        handle: i32,
        sampling_period_ns: i64,
        _max_latency_ns: i64,
    ) -> binder::Result<()> {
        let mut state = self.state.lock().unwrap();
        let s = state.sensor(handle)?;
        s.period =
            Duration::from_nanos(sampling_period_ns.max(0) as u64).clamp(MIN_PERIOD, MAX_PERIOD);
        Ok(())
    }

    fn configDirectReport(
        &self,
        _handle: i32,
        _channel: i32,
        _rate: RateLevel,
    ) -> binder::Result<i32> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn flush(&self, handle: i32) -> binder::Result<()> {
        let mut state = self.state.lock().unwrap();
        state.sensor(handle)?;
        let done = event(
            handle,
            SensorType::META_DATA,
            EventPayload::Meta(MetaData {
                what: MetaDataEventType::META_DATA_FLUSH_COMPLETE,
            }),
        );
        state.send(&[done]);
        Ok(())
    }

    fn getSensorsList(&self) -> binder::Result<Vec<SensorInfo>> {
        Ok(sensor_list(self.state.lock().unwrap().present))
    }

    fn initialize(
        &self,
        events: &MQDescriptor<Event, SynchronizedReadWrite>,
        _wake_lock: &MQDescriptor<i32, SynchronizedReadWrite>,
        _callback: &Strong<dyn ISensorsCallback>,
    ) -> binder::Result<()> {
        let queue =
            fmq::Writer::new(&events.grantors, &events.handle, events.quantum).map_err(|e| {
                log::error!("cannot map the event queue: {e}");
                Status::new_exception(ExceptionCode::ILLEGAL_ARGUMENT, None)
            })?;
        let mut state = self.state.lock().unwrap();
        // A (re)initialized framework starts with every sensor off.
        for s in &mut state.sensors {
            s.active = false;
            s.last = None;
        }
        state.queue = Some(queue);
        log::info!("event queue mapped");
        Ok(())
    }

    fn injectSensorData(&self, _event: &Event) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn registerDirectChannel(&self, _mem: &SharedMemInfo) -> binder::Result<i32> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setOperationMode(&self, mode: OperationMode) -> binder::Result<()> {
        if mode == OperationMode::NORMAL {
            Ok(())
        } else {
            exception(ExceptionCode::UNSUPPORTED_OPERATION)
        }
    }

    fn unregisterDirectChannel(&self, _channel: i32) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_present_sensors_are_listed() {
        assert!(sensor_list(0).is_empty());
        let light = sensor_list(present::LIGHT);
        assert_eq!(light.len(), 1);
        assert_eq!(light[0].r#type, SensorType::LIGHT);
        let both = sensor_list(present::LIGHT | present::HINGE);
        assert_eq!(both[1].r#type, SensorType::HINGE_ANGLE);
        assert!(
            both.iter()
                .all(|s| s.flags == SENSOR_FLAG_BITS_ON_CHANGE_MODE)
        );
        assert_ne!(both[0].sensorHandle, both[1].sensorHandle);
    }

    #[test]
    fn unknown_handles_are_rejected_and_periods_clamped() {
        let s = Sensors::new(present::LIGHT);
        assert!(s.activate(HANDLE_HINGE, true).is_err());
        s.batch(HANDLE_LIGHT, 1_000, 0).unwrap();
        s.activate(HANDLE_LIGHT, true).unwrap();
        let state = s.state.lock().unwrap();
        assert_eq!(state.sensors[0].period, MIN_PERIOD);
        assert!(state.sensors[0].active);
    }
}
