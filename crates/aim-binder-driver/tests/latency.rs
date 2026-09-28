//! In-process round-trip latency of a small call and reply between two
//! binder processes on two OS threads, driven the way IPCThreadState
//! drives the driver:
//!
//! - client: one BINDER_WRITE_READ with [BC_FREE_BUFFER(previous reply),
//!   BC_TRANSACTION] that returns [BR_NOOP, BR_TRANSACTION_COMPLETE,
//!   BR_REPLY];
//! - server looper: a read returning BR_TRANSACTION, then [BC_REPLY] →
//!   [BR_NOOP, BR_TRANSACTION_COMPLETE], then [BC_FREE_BUFFER] + read again.
//!
//! Run with `cargo test --release --test latency -- --nocapture` for the
//! number; debug builds run a short smoke version.

mod support;

use std::thread;
use std::time::{Duration, Instant};

use aim_binder_driver::uapi::*;
use aim_binder_driver::{Device, Driver};
use support::*;

const STOP: u32 = 0xdead;

#[test]
fn small_call_round_trip_latency() {
    let iterations = if cfg!(debug_assertions) {
        2_000
    } else {
        50_000
    };
    let driver = Driver::new();
    let server = Process::open(&driver, Device::Binder, 20, 1000, 0);
    server.become_context_manager(20, 0).unwrap();
    server.flush(20, Commands::new().enter_looper());
    let client = Process::open(&driver, Device::Binder, 10, 10_001, 0);

    let looper = {
        let server = server.clone();
        thread::spawn(move || {
            let mut reply = Parcel::new();
            reply.write_i32(0).write_i32(42);
            let mut pending = Commands::new();
            loop {
                let request = server.transact(20, &pending);
                let (tr, _) = find_transaction(&request);
                let done = server.transact(20, Commands::new().reply(0, &reply));
                debug_assert_eq!(done.len(), 2);
                pending = Commands::new();
                pending.free_buffer(tr.buffer);
                if tr.code == STOP {
                    server.flush(20, &pending);
                    return;
                }
            }
        })
    };

    let mut call = Parcel::new();
    call.write_interface_token("demo.IDemo").write_i32(7);
    let mut samples = Vec::with_capacity(iterations);
    let mut previous: Option<u64> = None;
    for i in 0..iterations + 1 {
        let mut commands = Commands::new();
        if let Some(buffer) = previous {
            commands.free_buffer(buffer);
        }
        let code = if i == iterations { STOP } else { 1 };
        commands.transaction(0, code, TF_ACCEPT_FDS, &call);
        let start = Instant::now();
        let returns = client.transact(10, &commands);
        let elapsed = start.elapsed();
        let reply = find_reply(&returns);
        previous = Some(reply.buffer);
        if i < iterations {
            samples.push(elapsed);
        }
    }
    client.flush(10, Commands::new().free_buffer(previous.unwrap()));
    looper.join().unwrap();

    samples.sort();
    let pct = |p: f64| samples[((samples.len() as f64 * p) as usize).min(samples.len() - 1)];
    let mean = samples.iter().sum::<Duration>() / samples.len() as u32;
    eprintln!(
        "binder round trip ({} calls, {} build): mean {:.2} µs, p50 {:.2} µs, p90 {:.2} µs, p99 {:.2} µs",
        samples.len(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        mean.as_secs_f64() * 1e6,
        pct(0.50).as_secs_f64() * 1e6,
        pct(0.90).as_secs_f64() * 1e6,
        pct(0.99).as_secs_f64() * 1e6,
    );
}
