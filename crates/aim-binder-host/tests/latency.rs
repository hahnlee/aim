//! Round-trip latency of a small call and reply through the daemon, and
//! what each call costs the calling thread, between two binder files on
//! two threads of this process, driven the way IPCThreadState drives the
//! driver:
//!
//! - client: one BINDER_WRITE_READ with [BC_FREE_BUFFER(previous reply),
//!   BC_TRANSACTION] that returns [BR_NOOP, BR_TRANSACTION_COMPLETE,
//!   BR_REPLY];
//! - server looper: [BC_FREE_BUFFER] + a read returning BR_TRANSACTION,
//!   then [BC_REPLY] returning [BR_NOOP, BR_TRANSACTION_COMPLETE].
//!
//! Run with `cargo test -p aim-binder-host --release --test latency --
//! --nocapture` for the numbers; debug builds run a short smoke version.

mod support;

use std::time::{Duration, Instant};

use aim_binder_driver::uapi::*;
use aim_binder_host::client::Client;
use aim_binder_host::server::Server;
use support::*;

const STOP: u32 = 0xdead;

unsafe extern "C" {
    static mach_task_self_: u32;
    fn mach_thread_self() -> u32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
    fn thread_info(thread: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
}

/// `THREAD_BASIC_INFO` of `thread_info`.
const THREAD_BASIC_INFO: u32 = 3;

/// The calling thread's user and system CPU time.
fn thread_times() -> (Duration, Duration) {
    // time_value_t user_time, system_time, then fields we do not read.
    let mut info = [0i32; 10];
    let mut count = info.len() as u32;
    // SAFETY: thread_info into a local array of the size it is told.
    unsafe {
        let port = mach_thread_self();
        thread_info(port, THREAD_BASIC_INFO, info.as_mut_ptr(), &mut count);
        mach_port_deallocate(mach_task_self_, port);
    }
    let t = |s: i32, us: i32| Duration::new(s as u64, 0) + Duration::from_micros(us as u64);
    (t(info[0], info[1]), t(info[2], info[3]))
}

fn transaction(code: u32, data: &[u8]) -> TransactionData {
    TransactionData {
        code,
        data_size: data.len() as u64,
        buffer: data.as_ptr() as u64,
        ..Default::default()
    }
}

#[test]
fn small_call_round_trip_latency() {
    let (warmup, iterations) = if cfg!(debug_assertions) {
        (100, 1_000)
    } else {
        (2_000, 20_000)
    };
    let name = format!("dev.aim.test.binder-host.latency.{}", std::process::id());
    let _server = Server::start(&name).unwrap();
    let client = Client::connect(&name).unwrap();
    let (server_tid, client_tid) = (20, 10);
    let service = open(&client, 1000, "u:r:servicemanager:s0");
    let mut fbo = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        binder: 0x1234,
        cookie: 0x5678,
        ..Default::default()
    }
    .encode();
    service
        .ioctl(
            server_tid,
            BINDER_SET_CONTEXT_MGR_EXT,
            fbo.as_mut_ptr() as u64,
            &mut Own,
        )
        .unwrap();
    let mut enter = Vec::new();
    cmd(&mut enter, BC_ENTER_LOOPER, &[]);
    write_read(&service, server_tid, &enter, false).unwrap();

    let looper = std::thread::spawn(move || {
        let reply = 42u64.to_le_bytes();
        let mut pending = Vec::new();
        loop {
            let read = write_read(&service, server_tid, &pending, true).unwrap();
            pending.clear();
            let Some(tr) = returns(&read)
                .into_iter()
                .find_map(|(c, tr)| (c == BR_TRANSACTION).then(|| tr.unwrap()))
            else {
                continue;
            };
            let mut write = Vec::new();
            cmd(&mut write, BC_REPLY, &transaction(0, &reply).encode());
            let done = write_read(&service, server_tid, &write, true).unwrap();
            debug_assert_eq!(returns(&done)[1].0, BR_TRANSACTION_COMPLETE);
            cmd(&mut pending, BC_FREE_BUFFER, &tr.buffer.to_le_bytes());
            if tr.code == STOP {
                write_read(&service, server_tid, &pending, false).unwrap();
                return;
            }
        }
    });

    let caller = open(&client, 10_001, "u:r:untrusted_app:s0");
    let data = [7u8; 64];
    let mut samples = Vec::with_capacity(iterations);
    let mut ioctls = 0;
    let mut previous: Option<u64> = None;
    let mut cpu = (Duration::ZERO, Duration::ZERO);
    for i in 0..warmup + iterations + 1 {
        if i == warmup {
            cpu = thread_times();
        }
        let mut write = Vec::new();
        if let Some(buffer) = previous {
            cmd(&mut write, BC_FREE_BUFFER, &buffer.to_le_bytes());
        }
        let code = if i == warmup + iterations { STOP } else { 1 };
        cmd(
            &mut write,
            BC_TRANSACTION,
            &transaction(code, &data).encode(),
        );
        let start = Instant::now();
        let mut read = write_read(&caller, client_tid, &write, true).unwrap();
        ioctls += 1;
        let reply = loop {
            if let Some((_, tr)) = returns(&read).into_iter().find(|(c, _)| *c == BR_REPLY) {
                break tr.unwrap();
            }
            read = write_read(&caller, client_tid, &[], true).unwrap();
            ioctls += 1;
        };
        let elapsed = start.elapsed();
        previous = Some(reply.buffer);
        if i == warmup + iterations - 1 {
            let now = thread_times();
            cpu = (now.0 - cpu.0, now.1 - cpu.1);
        }
        if i >= warmup && i < warmup + iterations {
            samples.push(elapsed);
        }
    }
    let mut write = Vec::new();
    cmd(&mut write, BC_FREE_BUFFER, &previous.unwrap().to_le_bytes());
    write_read(&caller, client_tid, &write, false).unwrap();
    looper.join().unwrap();

    samples.sort();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let pct = |p: f64| us(samples[((samples.len() as f64 * p) as usize).min(samples.len() - 1)]);
    let per_call = |d: Duration| us(d) / iterations as f64;
    eprintln!(
        "binder round trip through the daemon ({iterations} calls, {} build): \
         p50 {:.2} µs, p90 {:.2} µs, p99 {:.2} µs; caller CPU per call: \
         user {:.2} µs, system {:.2} µs; {:.2} ioctls per call",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        pct(0.50),
        pct(0.90),
        pct(0.99),
        per_call(cpu.0),
        per_call(cpu.1),
        ioctls as f64 / (warmup + iterations + 1) as f64,
    );
}
