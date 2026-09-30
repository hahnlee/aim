//! Host-call module [`aim_hostcall::module::LOCATION`]: the Mac's
//! location from CoreLocation, for the guest `android.hardware.gnss`
//! service.
//!
//! CoreLocation delivers updates on the run loop of the thread that created
//! its `CLLocationManager`. The module therefore owns one such thread,
//! started on the first [`FN_START`]: it starts and stops updates when the
//! guest asks, runs its run loop while updates are on, and keeps the latest
//! fix, which [`FN_READ`] copies. No delegate is needed: the manager's
//! `location` property holds the latest fix.
//!
//! The first start asks for authorization, so macOS shows its Location
//! Services prompt once. Nothing here changes system settings: with
//! Location Services off or access denied, the guest gets no fix and the
//! reason in [`Fix::authorization`].

use std::ffi::{c_char, c_void};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aim_hostcall::location::{FN_READ, FN_START, FN_STOP, Fix, VERSION, authorization};
use aim_hostcall::{HostModule, args_mut, errno, module};

pub static MODULE: HostModule = HostModule {
    id: module::LOCATION,
    name: "location",
    version: VERSION,
    call,
};

unsafe fn call(func: u32, args: u64, len: u64) -> i64 {
    match func {
        FN_START | FN_STOP if len != 0 => -(errno::EINVAL as i64),
        FN_START => {
            set_updates(true);
            0
        }
        FN_STOP => {
            set_updates(false);
            0
        }
        // SAFETY: the registry passes the guest's argument block.
        FN_READ => match unsafe { args_mut::<Fix>(args, len) } {
            Ok(out) => {
                *out = read();
                0
            }
            Err(e) => e,
        },
        _ => -(errno::ENOSYS as i64),
    }
}

type Id = *mut c_void;
type Sel = *const c_void;

aim_hostcall::dylib! {
    static CORE_LOCATION = c"/System/Library/Frameworks/CoreLocation.framework/CoreLocation" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        static objc_msgSend: c_void;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
}

aim_hostcall::dylib! {
    static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: bool) -> i32;
    }
}

/// `objc_msgSend` typed as `fn(receiver, selector) -> R`.
///
/// # Safety
/// `receiver` must answer `selector`, which takes no argument and returns
/// an `R` by the AAPCS64 rules.
unsafe fn send<R>(receiver: Id, selector: &std::ffi::CStr) -> R {
    // SAFETY: caller contract; objc_msgSend has the callee's signature.
    unsafe {
        let f: unsafe extern "C" fn(Id, Sel) -> R = std::mem::transmute(objc_msgSend());
        f(receiver, sel_registerName(selector.as_ptr()))
    }
}

fn manager_class() -> Id {
    // SAFETY: CoreLocation is linked.
    unsafe { objc_getClass(c"CLLocationManager".as_ptr()) }
}

/// The app's authorization, from the class methods (callable on any
/// thread).
fn current_authorization() -> u32 {
    let class = manager_class();
    // SAFETY: `+locationServicesEnabled` returns BOOL and
    // `+authorizationStatus` a CLAuthorizationStatus (int).
    unsafe {
        if !send::<bool>(class, c"locationServicesEnabled") {
            return authorization::SERVICES_OFF;
        }
        match send::<i32>(class, c"authorizationStatus") {
            0 => authorization::NOT_DETERMINED,
            1 => authorization::RESTRICTED,
            2 => authorization::DENIED,
            _ => authorization::AUTHORIZED,
        }
    }
}

struct State {
    /// Updates requested by the guest.
    wanted: Mutex<bool>,
    changed: Condvar,
    fix: Mutex<Option<Fix>>,
}

fn state() -> &'static State {
    static STATE: OnceLock<State> = OnceLock::new();
    STATE.get_or_init(|| State {
        wanted: Mutex::new(false),
        changed: Condvar::new(),
        fix: Mutex::new(None),
    })
}

fn set_updates(on: bool) {
    static THREAD: OnceLock<()> = OnceLock::new();
    let s = state();
    *s.wanted.lock().unwrap() = on;
    s.changed.notify_all();
    if on {
        THREAD.get_or_init(|| {
            std::thread::Builder::new()
                .name("corelocation".into())
                .spawn(run)
                .expect("spawn the CoreLocation thread");
        });
    }
}

/// How long the run loop runs between checks of the guest's request.
const TICK: f64 = 0.5;

/// `kCFRunLoopRunFinished`: the mode has no sources or timers.
const RUN_FINISHED: i32 = 1;

fn run() {
    let s = state();
    // SAFETY: a new manager, owned by this thread for the process's life;
    // every message below is one CLLocationManager answers.
    let manager: Id = unsafe {
        let m: Id = send(manager_class(), c"alloc");
        let m: Id = send(m, c"init");
        send::<()>(m, c"requestWhenInUseAuthorization");
        m
    };
    loop {
        let mut wanted = s.wanted.lock().unwrap();
        while !*wanted {
            wanted = s.changed.wait(wanted).unwrap();
        }
        drop(wanted);
        // SAFETY: as above.
        unsafe { send::<()>(manager, c"startUpdatingLocation") };
        while *s.wanted.lock().unwrap() {
            // SAFETY: this thread's run loop, in the default mode.
            let r = unsafe { CFRunLoopRunInMode(*kCFRunLoopDefaultMode(), TICK, false) };
            let fix = latest(manager);
            *s.fix.lock().unwrap() = fix;
            if r == RUN_FINISHED {
                // The run loop has no source (the manager added none to
                // this thread's): it returns at once, so wait out the tick
                // here instead of spinning.
                let wanted = s.wanted.lock().unwrap();
                if *wanted {
                    drop(s.changed.wait_timeout(wanted, Duration::from_secs_f64(TICK)));
                }
            }
        }
        // SAFETY: as above.
        unsafe { send::<()>(manager, c"stopUpdatingLocation") };
    }
}

#[repr(C)]
struct Coordinate {
    latitude: f64,
    longitude: f64,
}

/// The manager's latest fix, if it has one.
fn latest(manager: Id) -> Option<Fix> {
    // SAFETY: `location` is a CLLocation or nil; each property below
    // returns a double, `coordinate` a CLLocationCoordinate2D and
    // `timestamp` an NSDate. The pool frees the autoreleased objects.
    unsafe {
        let pool = objc_autoreleasePoolPush();
        let l: Id = send(manager, c"location");
        let fix = (!l.is_null()).then(|| {
            let c: Coordinate = send(l, c"coordinate");
            let date: Id = send(l, c"timestamp");
            let unix: f64 = send(date, c"timeIntervalSince1970");
            Fix {
                valid: 1,
                latitude: c.latitude,
                longitude: c.longitude,
                altitude: send(l, c"altitude"),
                horizontal_accuracy: send(l, c"horizontalAccuracy"),
                vertical_accuracy: send(l, c"verticalAccuracy"),
                speed: send(l, c"speed"),
                speed_accuracy: send(l, c"speedAccuracy"),
                course: send(l, c"course"),
                course_accuracy: send(l, c"courseAccuracy"),
                unix_ms: (unix * 1e3) as i64,
                ..Fix::default()
            }
        });
        objc_autoreleasePoolPop(pool);
        fix
    }
}

/// The latest fix, with its age as of now, and the current authorization.
pub fn read() -> Fix {
    let mut fix = state().fix.lock().unwrap().unwrap_or_default();
    if fix.valid != 0 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        let at = Duration::from_millis(fix.unix_ms.max(0) as u64);
        fix.age_ns = now.saturating_sub(at).as_nanos() as u64;
    }
    fix.authorization = current_authorization();
    fix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_without_updates_reports_authorization_only() {
        // Reading never starts updates, so it shows no prompt.
        let f = read();
        eprintln!("{f:?}");
        assert!(f.authorization <= authorization::SERVICES_OFF);
        assert_eq!(f.valid, 0);
    }

    #[test]
    fn call_checks_the_argument_block() {
        let mut f = Fix::default();
        let p = &mut f as *mut Fix as u64;
        let einval = -(errno::EINVAL as i64);
        // SAFETY: a live, correctly sized block; the bad calls never touch it.
        unsafe {
            assert_eq!(call(FN_READ, p, 95), einval);
            assert_eq!(call(FN_START, p, 96), einval);
            assert_eq!(call(FN_STOP, p, 8), einval);
            assert_eq!(call(99, 0, 0), -(errno::ENOSYS as i64));
            assert_eq!(call(FN_READ, p, 96), 0);
        }
    }
}
