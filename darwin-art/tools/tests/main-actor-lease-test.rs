#[path = "../../crates/darwin-art-host/src/main_actor_lease.rs"]
mod main_actor_lease;

use main_actor_lease::{MainActorAdmissionError, MainActorLease};

fn main() {
    let first = MainActorLease::acquire().expect("main-thread admission");
    assert!(matches!(
        MainActorLease::acquire(),
        Err(MainActorAdmissionError::SessionAlreadyActive)
    ));
    std::thread::spawn(|| {
        assert!(matches!(
            MainActorLease::acquire(),
            Err(MainActorAdmissionError::NotMainThread)
        ));
    })
    .join()
    .unwrap();
    // Rejection must not release or steal the active actor obligation.
    assert!(matches!(
        MainActorLease::acquire(),
        Err(MainActorAdmissionError::SessionAlreadyActive)
    ));
    drop(first);
    let recovered = MainActorLease::acquire().expect("sequential session admission");
    drop(recovered);
    println!("main actor lease: main-thread, reentry rejection, exact release PASS");
}
