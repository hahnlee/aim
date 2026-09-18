#[path = "support/main_task_drain.rs"]
mod main_task_drain;

use main_task_drain::{MainTaskDrainError, MainTaskDrainResult, drain_main_tasks};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

static MAIN_QUEUE_RAN: AtomicBool = AtomicBool::new(false);

unsafe extern "C" fn mark_main_queue(_: *mut c_void) {
    MAIN_QUEUE_RAN.store(true, Ordering::Release);
}

#[link(name = "System")]
unsafe extern "C" {
    // dispatch_get_main_queue is an SDK inline accessor, not a dylib symbol.
    // Its ABI is the address of the process-owned _dispatch_main_q object.
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        queue: *mut c_void,
        context: *mut c_void,
        work: unsafe extern "C" fn(*mut c_void),
    );
}

fn main() {
    // SAFETY: the main queue is a process-owned singleton and the callback is
    // an extern function with no borrowed context.
    unsafe {
        dispatch_async_f(
            std::ptr::addr_of!(_dispatch_main_q).cast_mut(),
            std::ptr::null_mut(),
            mark_main_queue,
        );
    }
    let secondary = thread::spawn(|| {
        assert_eq!(
            drain_main_tasks(),
            Err(MainTaskDrainError::NotMainThread),
            "secondary threads must not service the process main queue"
        );
    });
    secondary.join().expect("secondary drain thread panicked");

    let first = drain_main_tasks().expect("main-thread CFRunLoop drain failed");
    assert!(
        matches!(
            first,
            MainTaskDrainResult::SourceHandled | MainTaskDrainResult::TimedOut
        ),
        "unexpected bounded drain result: {first:?}"
    );
    assert!(
        MAIN_QUEUE_RAN.load(Ordering::Acquire),
        "dispatch_async_f callback did not execute on the process main queue"
    );
    println!("main-task-drain: PASS ({first:?})");
}
