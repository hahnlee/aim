//! Process-main actor scheduling and owned Android-worker result transport.
//!
//! This boundary does not select APK identity, execute Java, or own VM teardown.

use crate::config::{HostError, HostOutcome};
use crate::main_actor_lease::MainActorLease;
use crate::process_exit::exit_android_process;
use darwin_art_engine_sys::AppKitPumpEventsFn;
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::thread;
use std::time::Instant;

pub(crate) enum WorkerMessage {
    AppKitPump(AppKitPumpEventsFn),
    Finished(Result<HostOutcome, HostError>),
    ProcessCompletion(crate::process_completion::ActorCompletionRequest),
}

const PUMP_QUANTUM_SECONDS: f64 = 0.016;

pub(crate) fn run(
    android_process: bool,
    owner: impl FnOnce(&SyncSender<WorkerMessage>) -> Result<HostOutcome, HostError> + Send + 'static,
) -> Result<HostOutcome, HostError> {
    // Reject wrong-thread and overlapping sessions before starting the worker
    // or loading a runtime image. The lease stays on this admitting thread.
    let _lease = MainActorLease::acquire()
        .map_err(|error| HostError::HostService(format!("AppKit actor admission: {error:?}")))?;
    let (sender, receiver) = mpsc::sync_channel::<WorkerMessage>(2);
    let worker = thread::Builder::new()
        .name("darwin-art-ui-owner".to_owned())
        .spawn(move || {
            let result = owner(&sender);
            let _ = sender.send(WorkerMessage::Finished(result));
        })
        .map_err(|error| HostError::HostService(format!("spawn ART UI owner: {error}")))?;

    let mut appkit_pump: Option<AppKitPumpEventsFn> = None;
    let mut pump_count = 0_u64;
    let mut total_us = 0_u64;
    let mut max_us = 0_u64;
    let result = loop {
        match receiver.try_recv() {
            Ok(WorkerMessage::AppKitPump(callback)) => appkit_pump = Some(callback),
            Ok(WorkerMessage::Finished(result)) => break result,
            Ok(WorkerMessage::ProcessCompletion(request)) => {
                // Seal event processing before authorizing the owner's
                // value-only verification and OS exit.
                request.authorize();
                loop {
                    thread::park();
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                break Err(HostError::HostService(
                    "ART UI owner disconnected before completion".to_owned(),
                ));
            }
        }
        if let Some(callback) = appkit_pump {
            let started = Instant::now();
            // SAFETY: the registered engine owns the callback while running.
            // Reusable retirement/image-release synchronization is still an
            // explicit unfinished contract, not established by this split.
            let status = unsafe { callback(PUMP_QUANTUM_SECONDS) };
            let elapsed = started.elapsed().as_micros() as u64;
            pump_count += 1;
            total_us = total_us.saturating_add(elapsed);
            max_us = max_us.max(elapsed);
            if std::env::var_os("DARWIN_ART_DEBUG_FRAME_TIMING").is_some() && pump_count % 1024 == 0
            {
                eprintln!(
                    "DARWIN_ART appkit-pump count={} avg_us={} max_us={} quantum_us={}",
                    pump_count,
                    total_us / pump_count,
                    max_us,
                    (PUMP_QUANTUM_SECONDS * 1_000_000.0) as u64,
                );
            }
            if status != 0 {
                if android_process {
                    eprintln!("darwin-art-host: AppKit actor failed status={status}");
                    exit_android_process(1);
                }
                // Failure cancellation and native cleanup dispatch are not
                // yet safe for generic reusable sessions. Do not claim this
                // inherited failure path is a recovery protocol.
                break Err(HostError::SurfaceFailed {
                    operation: "appkit_main_actor_pump",
                    status,
                });
            }
        } else {
            thread::sleep(std::time::Duration::from_millis(1));
        }
    };
    if std::env::var_os("DARWIN_ART_DEBUG_FRAME_TIMING").is_some() {
        eprintln!(
            "DARWIN_ART appkit-pump count={} avg_us={} max_us={} quantum_us={}",
            pump_count,
            total_us.checked_div(pump_count).unwrap_or(0),
            max_us,
            (PUMP_QUANTUM_SECONDS * 1_000_000.0) as u64,
        );
    }
    // A panicked worker is not a successfully completed owner. Preserve an
    // already reported execution failure; otherwise report the panic.
    match (result, worker.join()) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(_)) => Err(HostError::HostService("ART UI owner panicked".into())),
        (Ok(outcome), Ok(())) => Ok(outcome),
    }
}
