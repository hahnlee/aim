//! Darwin thread capabilities. A guest TID is never used as a host process PID.
use libc::*;
use std::mem::{size_of, zeroed};
unsafe extern "C" {
    fn mach_port_deallocate(task: mach_port_t, name: mach_port_t) -> kern_return_t;
}

pub(crate) struct Thread(thread_t);
impl Thread {
    pub(crate) fn id(&self) -> Result<u64, i32> {
        let mut info: thread_identifier_info = unsafe { zeroed() };
        let mut words = THREAD_IDENTIFIER_INFO_COUNT;
        if unsafe {
            thread_info(
                self.0,
                THREAD_IDENTIFIER_INFO as _,
                (&mut info as *mut thread_identifier_info).cast(),
                &mut words,
            )
        } != KERN_SUCCESS
        {
            return Err(3);
        }
        Ok(info.thread_id)
    }
    pub(crate) fn set_policy(&self, flavor: i32, mut value: i32) -> Result<(), i32> {
        if unsafe { thread_policy_set(self.0, flavor as _, &mut value, 1) } == KERN_SUCCESS {
            Ok(())
        } else {
            Err(1)
        }
    }
}
impl Drop for Thread {
    fn drop(&mut self) {
        unsafe {
            mach_port_deallocate(mach_task_self(), self.0);
        }
    }
}

pub(crate) fn thread(tid: i32) -> Result<Thread, i32> {
    if tid < 0 {
        return Err(22);
    }
    let mut current = 0;
    if tid == 0 {
        if unsafe { pthread_threadid_np(0, &mut current) } != 0 {
            return Err(5);
        }
    } else {
        current = tid as u64;
    }
    let mut ports = std::ptr::null_mut();
    let mut count = 0;
    let task = unsafe { mach_task_self() };
    if unsafe { task_threads(task, &mut ports, &mut count) } != KERN_SUCCESS {
        return Err(5);
    }
    let mut selected = None;
    for index in 0..count as usize {
        let port = Thread(unsafe { *ports.add(index) });
        let mut info: thread_identifier_info = unsafe { zeroed() };
        let mut words = THREAD_IDENTIFIER_INFO_COUNT;
        let status = unsafe {
            thread_info(
                port.0,
                THREAD_IDENTIFIER_INFO as _,
                (&mut info as *mut thread_identifier_info).cast(),
                &mut words,
            )
        };
        if status == KERN_SUCCESS && info.thread_id == current {
            selected = Some(port);
        }
    }
    unsafe {
        vm_deallocate(
            task,
            ports as vm_address_t,
            count as vm_size_t * size_of::<thread_t>() as vm_size_t,
        );
    }
    selected.ok_or(3)
}

pub(crate) fn policy(thread: &Thread, flavor: i32) -> Result<i32, i32> {
    let mut value = 0;
    let mut count = 1;
    let mut default = 0;
    let status =
        unsafe { thread_policy_get(thread.0, flavor as _, &mut value, &mut count, &mut default) };
    if status != KERN_SUCCESS || default != 0 || count != 1 {
        return Err(95);
    }
    Ok(value)
}

// Return status separately: -1 is a valid Android nice value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_thread_get_nice(tid: i32, output: *mut i32) -> i32 {
    if output.is_null() {
        return 22;
    }
    match thread(tid).and_then(|thread| policy(&thread, THREAD_PRECEDENCE_POLICY)) {
        Ok(importance) => {
            unsafe {
                output.write(-importance);
            }
            0
        }
        Err(error) => error,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_thread_set_nice(tid: i32, nice: i32) -> i32 {
    if !(-20..=19).contains(&nice) {
        return 22;
    }
    let thread = match thread(tid) {
        Ok(thread) => thread,
        Err(error) => return error,
    };
    let mut importance = -nice;
    let status =
        unsafe { thread_policy_set(thread.0, THREAD_PRECEDENCE_POLICY as _, &mut importance, 1) };
    if status == KERN_SUCCESS { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn own_thread_capability_and_priority() {
        std::thread::spawn(|| {
            let mut tid = 0;
            assert_eq!(unsafe { pthread_threadid_np(0, &mut tid) }, 0);
            let mut old = 0;
            assert_eq!(
                unsafe { darwin_art_thread_get_nice(tid as i32, &mut old) },
                0
            );
            assert_eq!(darwin_art_thread_set_nice(tid as i32, 4), 0);
            let mut actual = 0;
            assert_eq!(unsafe { darwin_art_thread_get_nice(0, &mut actual) }, 0);
            assert_eq!(actual, 4);
            assert_eq!(darwin_art_thread_set_nice(0, old), 0);
            assert_eq!(darwin_art_thread_set_nice(-1, 0), 22);
            assert_eq!(darwin_art_thread_set_nice(i32::MAX, 0), 3);
            assert_eq!(darwin_art_thread_set_nice(0, 20), 22);
            let mut unchanged = 123;
            assert_eq!(
                unsafe { darwin_art_thread_get_nice(i32::MAX, &mut unchanged) },
                3
            );
            assert_eq!(unchanged, 123);
            assert_eq!(
                unsafe { darwin_art_thread_get_nice(0, std::ptr::null_mut()) },
                22
            );
        })
        .join()
        .unwrap();
    }

    #[test]
    fn android_nice_range_is_supported_for_the_current_thread() {
        std::thread::spawn(|| {
            let mut original = 0;
            assert_eq!(unsafe { darwin_art_thread_get_nice(0, &mut original) }, 0);
            for nice in -20..=19 {
                assert_eq!(
                    darwin_art_thread_set_nice(0, nice),
                    0,
                    "Android nice value {nice} must be representable"
                );
            }
            assert_eq!(darwin_art_thread_set_nice(0, original), 0);
        })
        .join()
        .unwrap();
    }
}
