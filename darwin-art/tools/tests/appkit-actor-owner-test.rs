//! Actual actor translation unit with a value-only worker and static test pump.
//! No VM/image retirement, APK or framework-service acceptance is claimed.
#![allow(dead_code)]

extern crate self as darwin_art_engine_sys;
pub type AppKitPumpEventsFn = unsafe extern "C" fn(f64) -> i32;

mod config {
    #[derive(Debug)]
    pub enum HostError {
        HostService(String),
        SurfaceFailed {
            operation: &'static str,
            status: i32,
        },
    }
    impl std::fmt::Display for HostError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{self:?}")
        }
    }
    #[derive(Debug)]
    pub struct HostOutcome(pub u64);
}
pub use config::{HostError, HostOutcome};

#[path = "../../crates/darwin-art-host/src/appkit_actor.rs"]
mod appkit_actor;
#[path = "../../crates/darwin-art-host/src/main_actor_lease.rs"]
mod main_actor_lease;
#[path = "../../crates/darwin-art-host/src/process_completion.rs"]
mod process_completion;

// The native process-exit adapter is substituted, not the production actor.
mod process_exit {
    unsafe extern "C" {
        fn _exit(status: i32) -> !;
        fn fflush(stream: *mut std::ffi::c_void) -> i32;
    }
    pub fn exit_android_process(status: i32) -> ! {
        unsafe {
            fflush(std::ptr::null_mut());
            _exit(status)
        }
    }
}

use appkit_actor::WorkerMessage;
use std::sync::atomic::{AtomicUsize, Ordering};

static PUMPS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" {
    fn pthread_main_np() -> i32;
}

unsafe extern "C" fn pump(_: f64) -> i32 {
    assert_eq!(unsafe { pthread_main_np() }, 1);
    PUMPS.fetch_add(1, Ordering::SeqCst);
    0
}
unsafe extern "C" fn failed_pump(_: f64) -> i32 {
    19
}

fn main() {
    if let Some(mode) = std::env::args().nth(1) {
        let fail = mode == "failed-pump";
        let _ = appkit_actor::run(true, move |sender| {
            sender
                .send(WorkerMessage::AppKitPump(if fail {
                    failed_pump
                } else {
                    pump
                }))
                .unwrap();
            let (request, wait) = process_completion::actor_completion_gate();
            sender
                .send(WorkerMessage::ProcessCompletion(request))
                .unwrap();
            wait.wait().unwrap();
            eprintln!("authorized-result-handoff");
            process_exit::exit_android_process(0);
        });
        panic!("one-shot actor unexpectedly returned");
    }
    let wrong_thread = std::thread::spawn(|| {
        appkit_actor::run(false, |_| panic!("must reject before starting owner"))
    })
    .join()
    .unwrap();
    assert!(
        matches!(wrong_thread, Err(HostError::HostService(text)) if text.contains("NotMainThread"))
    );

    let outcome = appkit_actor::run(false, |sender| {
        assert_eq!(unsafe { pthread_main_np() }, 0);
        sender.send(WorkerMessage::AppKitPump(pump)).unwrap();
        Ok(HostOutcome(7))
    })
    .unwrap();
    assert_eq!(outcome.0, 7);
    assert!(PUMPS.load(Ordering::SeqCst) > 0);
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {})); // Expected worker failure only.
    let panicked = appkit_actor::run(false, |_| panic!("test worker panic"));
    std::panic::set_hook(hook);
    assert!(matches!(panicked, Err(HostError::HostService(_))));
    assert_eq!(
        appkit_actor::run(false, |_| Ok(HostOutcome(8))).unwrap().0,
        8
    );

    for (mode, status, handoff) in [("success", 0, true), ("failed-pump", 1, false)] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg(mode)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(status), "{mode}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr.contains("authorized-result-handoff"),
            handoff,
            "{mode}"
        );
    }
    println!("actual AppKit actor: admission, owner sequence, panic, sealed process handoff PASS");
}
