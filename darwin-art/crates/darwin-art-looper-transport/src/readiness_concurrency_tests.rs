use crate::{Interest, Readiness, Registration, Wake};
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Barrier};

const ERRORS: Registration = Registration {
    read: false,
    write: false,
    error_only: true,
    token: 10,
};
const EMPTY: Registration = Registration {
    read: false,
    write: false,
    error_only: false,
    token: 0,
};

#[test]
fn wait_races_with_retoken_and_cancellation() {
    // A barrier starts competing wait/change operations. No assertion depends
    // on which enters the kernel first; EOF is generated only after the change.
    for cancel in [false, true] {
        for _ in 0..32 {
            let queue = Arc::new(Readiness::new().unwrap());
            let (reader, mut writer) = UnixStream::pair().unwrap();
            queue.transition(reader.as_fd(), EMPTY, ERRORS).unwrap();
            let wake = Wake::new().unwrap();
            queue.register(wake.poll_fd(), Interest::Read, 99).unwrap();
            writer.write_all(b"unread").unwrap();
            let barrier = Arc::new(Barrier::new(2));
            let waiting_queue = queue.clone();
            let waiting_barrier = barrier.clone();
            let waiter = std::thread::spawn(move || {
                waiting_barrier.wait();
                waiting_queue.wait(8, 1000).unwrap()
            });
            barrier.wait();
            let next = if cancel {
                EMPTY
            } else {
                Registration {
                    token: 11,
                    ..ERRORS
                }
            };
            queue.transition(reader.as_fd(), ERRORS, next).unwrap();
            if cancel {
                wake.signal(1).unwrap();
            } else {
                drop(writer);
            }
            let ready = waiter.join().unwrap();
            assert_eq!(ready.len(), 1);
            assert_eq!(ready[0].token, if cancel { 99 } else { 11 });
            assert_eq!(
                ready[0].interest,
                if cancel {
                    Interest::Read
                } else {
                    Interest::ErrorOnly
                }
            );
            assert_eq!(ready[0].eof, !cancel);
        }
    }
}

#[test]
fn kernel_batch_preserves_error_only_mode_after_removal() {
    let queue = Readiness::new().unwrap();
    let (reader, mut writer) = UnixStream::pair().unwrap();
    queue.transition(reader.as_fd(), EMPTY, ERRORS).unwrap();
    writer.write_all(b"unread").unwrap();
    let batch = queue.wait_once(8, 1000).unwrap();
    queue.transition(reader.as_fd(), ERRORS, EMPTY).unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].interest, Interest::ErrorOnly);
    assert!(!batch[0].eof);
    let state = queue.terminal.lock().unwrap();
    assert!(!state.contains_sequence(batch[0].token));
}
