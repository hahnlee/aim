use super::*;

#[test]
fn simultaneous_requests_reserve_exactly_one_instance() {
    let services = Arc::new(RuntimeServiceState::default());
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let workers = (0..8)
        .map(|_| {
            let services = Arc::clone(&services);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                services.reserve(&request(ReadinessMask::BINDER)).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|(_, fresh)| *fresh).count(), 1);
    assert!(
        results
            .iter()
            .all(|(instance, _)| Arc::ptr_eq(instance, &results[0].0))
    );
}
use crate::runtime_service_protocol::RuntimeKey;
use std::ffi::OsString;
use std::sync::Arc;
use std::time::Duration;

fn request(required: ReadinessMask) -> StartRuntimeRequest {
    StartRuntimeRequest {
        package: "org.example.runtime".into(),
        key: RuntimeKey([0x11; 32]),
        required,
        arguments: vec![OsString::from("runtime")],
        environment: Vec::new(),
    }
}

#[test]
fn exact_reservations_coalesce_and_key_or_mask_conflicts_fail() {
    let state = RuntimeServiceState::default();
    let first_request = request(ReadinessMask::BINDER);
    let (first, fresh) = state.reserve(&first_request).unwrap();
    assert!(fresh);
    let (same, fresh) = state.reserve(&first_request).unwrap();
    assert!(!fresh);
    assert!(Arc::ptr_eq(&first, &same));

    let mut different_key = first_request.clone();
    different_key.key = RuntimeKey([0x22; 32]);
    assert!(state.reserve(&different_key).is_err());

    let mut different_mask = first_request;
    different_mask.required = ReadinessMask::COMPOSITOR;
    assert!(state.reserve(&different_mask).is_err());
}

#[test]
fn conflicting_instance_identifies_only_a_live_different_generation() {
    let state = RuntimeServiceState::default();
    let original = request(ReadinessMask::BINDER);
    let (instance, _) = state.reserve(&original).unwrap();
    assert!(state.conflicting_instance(&original).unwrap().is_none());

    let mut replacement = original.clone();
    replacement.key = RuntimeKey([0x22; 32]);
    assert!(Arc::ptr_eq(
        &instance,
        &state.conflicting_instance(&replacement).unwrap().unwrap()
    ));
    instance.exited().unwrap();
    assert!(state.conflicting_instance(&replacement).unwrap().is_none());
}

#[test]
fn replacement_wait_observes_supervisor_exit_publication() {
    let state = RuntimeServiceState::default();
    let original = request(ReadinessMask::BINDER);
    let (instance, _) = state.reserve(&original).unwrap();
    let publisher = Arc::clone(&instance);
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        publisher.exited().unwrap();
    });
    instance.wait_exited(Duration::from_secs(1)).unwrap();
    worker.join().unwrap();
}

#[test]
fn failed_instance_remains_reserved_until_explicit_exit() {
    let state = RuntimeServiceState::default();
    let request = request(ReadinessMask::BINDER);
    let (instance, fresh) = state.reserve(&request).unwrap();
    assert!(fresh);
    instance.fail("spawn failed").unwrap();
    assert!(instance.snapshot().unwrap().is_failed);
    assert!(state.reserve(&request).is_err());

    instance.exited().unwrap();
    let (replacement, fresh) = state.reserve(&request).unwrap();
    assert!(fresh);
    assert!(!Arc::ptr_eq(&instance, &replacement));
}

#[test]
fn readiness_requires_token_and_attached_process_identity() {
    let state = RuntimeServiceState::default();
    let request = request(ReadinessMask::BINDER);
    let (instance, _) = state.reserve(&request).unwrap();
    let pid = std::process::id();
    let incarnation = ProcessIncarnation::read(pid).unwrap();
    instance.attach(pid, incarnation).unwrap();

    let bad_token = ReadyRequest {
        token: InstanceToken([0; 16]),
        readiness: ReadinessMask::BINDER,
    };
    assert!(state.publish_ready(pid, incarnation, bad_token).is_err());
    assert!(
        state
            .publish_ready(
                pid,
                ProcessIncarnation::fixture(99),
                ReadyRequest {
                    token: instance.token(),
                    readiness: ReadinessMask::BINDER,
                },
            )
            .is_err()
    );
    assert!(
        state
            .publish_ready(
                pid,
                incarnation,
                ReadyRequest {
                    token: instance.token(),
                    readiness: ReadinessMask::BINDER,
                },
            )
            .is_ok()
    );
}

#[test]
fn required_readiness_loss_fails_authenticated_live_instance_idempotently() {
    let state = RuntimeServiceState::default();
    let request = request(ReadinessMask::BINDER);
    let (instance, _) = state.reserve(&request).unwrap();
    let pid = std::process::id();
    let incarnation = ProcessIncarnation::read(pid).unwrap();
    instance.attach(pid, incarnation).unwrap();
    state
        .publish_ready(
            pid,
            incarnation,
            ReadyRequest {
                token: instance.token(),
                readiness: ReadinessMask::BINDER,
            },
        )
        .unwrap();

    let loss = LossRequest {
        token: instance.token(),
        lost: ReadinessMask::BINDER,
    };
    state.publish_runtime_lost(pid, incarnation, loss).unwrap();
    assert!(instance.snapshot().unwrap().is_failed);
    // A duplicate report from the same authenticated child is harmless while
    // the supervisor is arranging kill/reap.
    state.publish_runtime_lost(pid, incarnation, loss).unwrap();
    assert!(
        state
            .publish_runtime_lost(pid, ProcessIncarnation::fixture(99), loss)
            .is_err()
    );
}

#[test]
fn required_readiness_loss_while_starting_unblocks_as_failure() {
    let state = RuntimeServiceState::default();
    let request = request(ReadinessMask::COMPOSITOR);
    let (instance, _) = state.reserve(&request).unwrap();
    let pid = std::process::id();
    let incarnation = ProcessIncarnation::read(pid).unwrap();
    instance.attach(pid, incarnation).unwrap();
    state
        .publish_runtime_lost(
            pid,
            incarnation,
            LossRequest {
                token: instance.token(),
                lost: ReadinessMask::COMPOSITOR,
            },
        )
        .unwrap();
    let snapshot = instance.snapshot().unwrap();
    assert!(!snapshot.is_ready);
    assert!(snapshot.is_failed);
    assert!(!snapshot.is_exited);
}

#[test]
fn optional_readiness_loss_does_not_change_required_ready_state() {
    let state = RuntimeServiceState::default();
    let request = request(ReadinessMask::BINDER);
    let (instance, _) = state.reserve(&request).unwrap();
    let pid = std::process::id();
    let incarnation = ProcessIncarnation::read(pid).unwrap();
    instance.attach(pid, incarnation).unwrap();
    state
        .publish_ready(
            pid,
            incarnation,
            ReadyRequest {
                token: instance.token(),
                readiness: ReadinessMask::BINDER,
            },
        )
        .unwrap();
    state
        .publish_runtime_lost(
            pid,
            incarnation,
            LossRequest {
                token: instance.token(),
                lost: ReadinessMask::COMPOSITOR,
            },
        )
        .unwrap();
    let snapshot = instance.snapshot().unwrap();
    assert!(snapshot.is_ready);
    assert!(!snapshot.is_failed);
}

#[test]
fn readiness_loss_is_rejected_after_reap() {
    let state = RuntimeServiceState::default();
    let request = request(ReadinessMask::BINDER);
    let (instance, _) = state.reserve(&request).unwrap();
    let pid = std::process::id();
    let incarnation = ProcessIncarnation::read(pid).unwrap();
    instance.attach(pid, incarnation).unwrap();
    instance.exited().unwrap();
    assert!(
        state
            .publish_runtime_lost(
                pid,
                incarnation,
                LossRequest {
                    token: instance.token(),
                    lost: ReadinessMask::BINDER,
                },
            )
            .is_err()
    );
}

#[test]
fn early_ready_waits_for_attach_without_losing_notification() {
    let state = Arc::new(RuntimeServiceState::default());
    let request = request(ReadinessMask::BINDER);
    let (instance, _) = state.reserve(&request).unwrap();
    let pid = std::process::id();
    let incarnation = ProcessIncarnation::read(pid).unwrap();
    let ready_state = state.clone();
    let token = instance.token();
    let publisher = std::thread::spawn(move || {
        ready_state.publish_ready(
            pid,
            incarnation,
            ReadyRequest {
                token,
                readiness: ReadinessMask::BINDER,
            },
        )
    });
    std::thread::sleep(Duration::from_millis(10));
    instance.attach(pid, incarnation).unwrap();
    assert!(publisher.join().unwrap().is_ok());
    assert!(instance.snapshot().unwrap().is_ready);
}

#[test]
fn partial_readiness_is_accumulated_for_watchdog_and_waiter() {
    let state = RuntimeServiceState::default();
    let required = ReadinessMask::BINDER.union(ReadinessMask::COMPOSITOR);
    let (instance, _) = state.reserve(&request(required)).unwrap();
    let pid = std::process::id();
    let incarnation = ProcessIncarnation::read(pid).unwrap();
    instance.attach(pid, incarnation).unwrap();

    state
        .publish_ready(
            pid,
            incarnation,
            ReadyRequest {
                token: instance.token(),
                readiness: ReadinessMask::BINDER,
            },
        )
        .unwrap();
    let snapshot = instance.snapshot().unwrap();
    assert!(!snapshot.is_ready);
    assert!(!snapshot.is_failed);
    assert!(!snapshot.is_exited);
    assert_eq!(snapshot.pid, Some(pid));
    assert!(instance.wait_ready(Duration::ZERO).is_err());

    state
        .publish_ready(
            pid,
            incarnation,
            ReadyRequest {
                token: instance.token(),
                readiness: ReadinessMask::COMPOSITOR,
            },
        )
        .unwrap();
    assert!(instance.snapshot().unwrap().is_ready);
    let response = instance.wait_ready(Duration::from_millis(50)).unwrap();
    assert_eq!(response.pid, pid);
    assert_eq!(response.token, instance.token());
}

#[test]
fn ready_response_rechecks_kernel_incarnation() {
    let state = RuntimeServiceState::default();
    let request = request(ReadinessMask::BINDER);
    let (instance, _) = state.reserve(&request).unwrap();
    let pid = std::process::id();
    let incarnation = ProcessIncarnation::read(pid).unwrap();
    instance.attach(pid, incarnation).unwrap();
    // The fixture identity is intentionally not the kernel identity.  The
    // readiness state is accepted only after attach identity authentication,
    // while wait_ready must still reject a stale incarnation.
    instance
        .publish_ready(
            pid,
            incarnation,
            ReadyRequest {
                token: instance.token(),
                readiness: ReadinessMask::BINDER,
            },
        )
        .unwrap();
    assert!(instance.wait_ready(Duration::from_millis(50)).is_ok());

    instance.exited().unwrap();
    let (stale, _) = state.reserve(&request).unwrap();
    stale.attach(pid, ProcessIncarnation::fixture(1)).unwrap();
    stale
        .publish_ready(
            pid,
            ProcessIncarnation::fixture(1),
            ReadyRequest {
                token: stale.token(),
                readiness: ReadinessMask::BINDER,
            },
        )
        .unwrap();
    assert!(stale.wait_ready(Duration::from_millis(50)).is_err());
}

#[test]
fn child_watchdog_state_is_monotonic_and_exit_unblocks_waiters() {
    let state = RuntimeServiceState::default();
    let request = request(ReadinessMask::BINDER);
    let (instance, _) = state.reserve(&request).unwrap();
    assert_eq!(
        instance.snapshot().unwrap(),
        RuntimeSnapshot {
            is_ready: false,
            is_failed: false,
            is_exited: false,
            pid: None,
        }
    );
    instance.fail("child exited before ready").unwrap();
    assert_eq!(instance.snapshot().unwrap().is_failed, true);
    assert!(instance.wait_ready(Duration::ZERO).is_err());
    instance.exited().unwrap();
    assert!(instance.snapshot().unwrap().is_exited);
    assert!(
        instance
            .wait_ready(Duration::ZERO)
            .unwrap_err()
            .to_string()
            .contains("child exited before ready")
    );
    instance.exited().unwrap();
    assert!(
        instance
            .wait_ready(Duration::ZERO)
            .unwrap_err()
            .to_string()
            .contains("child exited before ready")
    );
    assert!(instance.fail("late failure").is_err());
}

#[test]
fn system_launch_template_is_owned_by_the_live_system_reservation() {
    let state = RuntimeServiceState::default();
    let mut system = request(ReadinessMask::BINDER);
    system.package = "android.system".into();
    system.arguments.push("--system-template-marker".into());
    system
        .environment
        .push(("DARWIN_ART_TEMPLATE_MARKER".into(), "trusted".into()));
    let (instance, fresh) = state.reserve(&system).unwrap();
    assert!(fresh);
    let incarnation = ProcessIncarnation::fixture(1);
    assert!(state.system_launch_template(42, incarnation).is_err());
    instance.attach(42, incarnation).unwrap();
    assert_eq!(
        state.system_launch_template(42, incarnation).unwrap(),
        system
    );
    assert!(state.system_launch_template(43, incarnation).is_err());
    assert!(
        state
            .system_launch_template(42, ProcessIncarnation::fixture(2))
            .is_err()
    );

    instance.exited().unwrap();
    assert!(state.system_launch_template(42, incarnation).is_err());
}

#[test]
fn application_runtime_cannot_be_used_as_the_system_launch_template() {
    let state = RuntimeServiceState::default();
    state.reserve(&request(ReadinessMask::BINDER)).unwrap();
    assert!(
        state
            .system_launch_template(42, ProcessIncarnation::fixture(1))
            .is_err()
    );
}

#[test]
fn old_system_request_cannot_use_replacement_generations_template() {
    let state = RuntimeServiceState::default();
    let mut old_request = request(ReadinessMask::BINDER);
    old_request.package = "android.system".into();
    let old_incarnation = ProcessIncarnation::fixture(1);
    let (old, _) = state.reserve(&old_request).unwrap();
    old.attach(42, old_incarnation).unwrap();
    assert_eq!(
        state.system_launch_template(42, old_incarnation).unwrap(),
        old_request
    );
    old.fail("generation replaced").unwrap();
    assert!(state.system_launch_template(42, old_incarnation).is_err());
    old.exited().unwrap();

    let mut replacement_request = old_request;
    replacement_request.key = RuntimeKey([0x22; 32]);
    let (replacement, fresh) = state.reserve(&replacement_request).unwrap();
    assert!(fresh);
    let replacement_incarnation = ProcessIncarnation::fixture(2);
    replacement.attach(42, replacement_incarnation).unwrap();
    assert!(state.system_launch_template(42, old_incarnation).is_err());
    assert_eq!(
        state
            .system_launch_template(42, replacement_incarnation)
            .unwrap(),
        replacement_request
    );
}

#[test]
fn contains_package_keeps_failed_reservations_live_until_exit() {
    let state = RuntimeServiceState::default();
    let request = request(ReadinessMask::BINDER);
    let (instance, _) = state.reserve(&request).unwrap();
    assert!(state.contains_package(&request.package));
    instance.fail("startup failure").unwrap();
    assert!(state.contains_package(&request.package));
    instance.exited().unwrap();
    assert!(!state.contains_package(&request.package));
}
