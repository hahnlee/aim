//! End-to-end check of the sensors HAL without sensorservice: plays the
//! framework's side of `ISensors`. It makes the event queue the way libfmq
//! lays one out (in a memfd), calls `initialize`, activates every sensor,
//! waits on the queue's event flag as `EventFlag::wait` does (a
//! cross-process bitset futex wait) and prints the events it reads.
//!
//! Built by hand (`cargo build --example sensors_client`), run in the guest
//! next to the HAL and servicemanager.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use android_hardware_common::aidl::android::hardware::common::NativeHandle::NativeHandle;
use android_hardware_common_fmq::aidl::android::hardware::common::fmq::{
    GrantorDescriptor::GrantorDescriptor, MQDescriptor::MQDescriptor,
};
use android_hardware_sensors::aidl::android::hardware::sensors::{
    Event::Event,
    Event::EventPayload::EventPayload,
    ISensors::ISensors,
    ISensorsCallback::{BnSensorsCallback, ISensorsCallback},
    SensorInfo::SensorInfo,
};
use binder::{BinderFeatures, Interface, ParcelFileDescriptor};
use std::os::fd::{FromRawFd, OwnedFd};

const SLOTS: usize = 16;
const EVENT: usize = size_of::<Event>();

struct Callback;
impl Interface for Callback {}
impl ISensorsCallback for Callback {
    fn onDynamicSensorsConnected(&self, _: &[SensorInfo]) -> binder::Result<()> {
        Ok(())
    }
    fn onDynamicSensorsDisconnected(&self, _: &[i32]) -> binder::Result<()> {
        Ok(())
    }
}

fn main() {
    let sensors: binder::Strong<dyn ISensors> =
        binder::wait_for_interface("android.hardware.sensors.ISensors/default").unwrap();
    let list = sensors.getSensorsList().unwrap();
    for s in &list {
        println!("sensor {} {:?} {}", s.sensorHandle, s.r#type, s.name);
    }

    // Read counter, write counter, ring, event flag, as libfmq places them.
    let (ring_at, flag_at) = (16, 16 + SLOTS * EVENT);
    let len = flag_at + 4;
    // SAFETY: a new memfd, mapped shared for our end of the queue.
    let (fd, base) = unsafe {
        let fd = libc::memfd_create(c"sensors-client".as_ptr(), 0);
        assert!(fd >= 0 && libc::ftruncate(fd, len as i64) == 0);
        let base = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        );
        assert_ne!(base, libc::MAP_FAILED);
        (OwnedFd::from_raw_fd(fd), base.cast::<u8>())
    };
    let grantor = |offset: usize, extent: usize| GrantorDescriptor {
        fdIndex: 0,
        offset: offset as i32,
        extent: extent as i64,
    };
    let mut queue = MQDescriptor::default();
    queue.grantors = vec![
        grantor(0, 8),
        grantor(8, 8),
        grantor(ring_at, SLOTS * EVENT),
        grantor(flag_at, 4),
    ];
    queue.handle = NativeHandle {
        fds: vec![ParcelFileDescriptor::new(fd)],
        ints: Vec::new(),
    };
    queue.quantum = EVENT as i32;
    // The HAL has no wake-up sensor, so it never touches the wake-lock
    // queue; an empty descriptor stands in for it.
    let mut wake_lock = MQDescriptor::default();
    wake_lock.quantum = 4;
    let callback = BnSensorsCallback::new_binder(Callback, BinderFeatures::default());
    sensors.initialize(&queue, &wake_lock, &callback).unwrap();
    for s in &list {
        sensors.batch(s.sensorHandle, 200_000_000, 0).unwrap();
        sensors.activate(s.sensorHandle, true).unwrap();
    }
    if let Some(s) = list.first() {
        sensors.flush(s.sensorHandle).unwrap();
    }

    // SAFETY: the counters and flag are aligned words in our mapping.
    let (read, write, flag) = unsafe {
        (
            &*base.cast::<AtomicU64>(),
            &*base.add(8).cast::<AtomicU64>(),
            &*base.add(flag_at).cast::<AtomicU32>(),
        )
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = 0;
    while seen < list.len() + 1 && Instant::now() < deadline {
        // EventFlag::wait: consume the bit, else futex-wait for it.
        if flag.fetch_and(!1, Ordering::SeqCst) & 1 == 0 {
            let timeout = libc::timespec {
                tv_sec: 1,
                tv_nsec: 0,
            };
            // SAFETY: a shared futex wait on our mapping's flag word.
            unsafe {
                libc::syscall(
                    libc::SYS_futex,
                    flag.as_ptr(),
                    libc::FUTEX_WAIT_BITSET,
                    0,
                    &timeout,
                    std::ptr::null::<u32>(),
                    1u32,
                )
            };
            continue;
        }
        let (r, w) = (read.load(Ordering::Acquire), write.load(Ordering::Acquire));
        for at in (r..w).step_by(EVENT) {
            let slot = (at as usize / EVENT) % SLOTS;
            // SAFETY: a published slot of the ring holds an Event.
            let e = unsafe { &*base.add(ring_at + slot * EVENT).cast::<Event>() };
            let value = match &e.payload {
                EventPayload::Scalar(v) => format!("{v}"),
                other => format!("{other:?}"),
            };
            println!("event handle {} {:?} {value}", e.sensorHandle, e.sensorType);
            seen += 1;
        }
        read.store(w, Ordering::Release);
    }
    for s in &list {
        sensors.activate(s.sensorHandle, false).unwrap();
    }
    println!("{seen} events");
    assert!(seen > list.len(), "missing events");
}
