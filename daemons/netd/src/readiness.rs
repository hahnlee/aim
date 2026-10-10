use std::time::Duration;

fn wait_for_property<S>(mut snapshot: impl FnMut() -> (bool, S), mut changed: impl FnMut(S, Duration)) {
    let mut seconds = 5;
    loop {
        let (ready, serial) = snapshot();
        if ready {
            return;
        }
        changed(serial, Duration::from_secs(seconds));
        seconds = (seconds * 2).min(60);
    }
}

unsafe extern "C" {
    fn __system_property_find(name: *const libc::c_char) -> *const libc::c_void;
    fn __system_property_get(name: *const libc::c_char, value: *mut libc::c_char) -> libc::c_int;
    fn __system_property_serial(property: *const libc::c_void) -> u32;
    fn __system_property_area_serial() -> u32;
    fn __system_property_wait(property: *const libc::c_void, old_serial: u32, new_serial: *mut u32, timeout: *const libc::timespec) -> bool;
}

pub fn wait() {
    wait_for_property(
        || unsafe {
            let property = __system_property_find(c"bpf.progs_loaded".as_ptr());
            let serial = if property.is_null() { __system_property_area_serial() } else { __system_property_serial(property) };
            let mut value = [0 as libc::c_char; 92];
            let length = __system_property_get(c"bpf.progs_loaded".as_ptr(), value.as_mut_ptr());
            (length == 1 && value[0] == b'1' as libc::c_char, (property, serial))
        },
        |(property, serial), duration| unsafe {
            let timeout = libc::timespec { tv_sec: duration.as_secs() as _, tv_nsec: 0 };
            let mut next = serial;
            __system_property_wait(property, serial, &mut next, &timeout);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_property_does_not_wait() {
        wait_for_property(|| (true, 7), |_, _| panic!("ready property waited"));
    }

    #[test]
    fn serial_changes_recheck_readiness_and_timeout_is_capped() {
        let serial = std::cell::Cell::new(0);
        let mut waits = Vec::new();
        wait_for_property(
            || (serial.get() == 7, serial.get()),
            |old, timeout| { assert_eq!(old, serial.get()); waits.push(timeout.as_secs()); serial.set(old + 1); },
        );
        assert_eq!(waits, [5, 10, 20, 40, 60, 60, 60]);
    }
}
