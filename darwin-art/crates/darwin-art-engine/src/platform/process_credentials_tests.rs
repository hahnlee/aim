use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn credentials() -> ProcessCredentialsInputs {
    ProcessCredentialsInputs {
        uid: 10123,
        euid: 10124,
        suid: 10125,
        gid: 20123,
        egid: 20124,
        sgid: 20125,
        groups: vec![3003, 3009],
        permitted: 3,
        effective: 1,
        inheritable: 2,
    }
}

unsafe extern "C" fn inspect(base: *const ProcessSnapshotConfig) -> i32 {
    let base = unsafe { &*base };
    if base.abi_version != 2 || base.struct_size != 152 {
        return -1;
    }
    let full =
        unsafe { &*(base as *const ProcessSnapshotConfig).cast::<ProcessSnapshotConfigV2>() };
    let c = &full.credentials;
    if [c.uid, c.euid, c.suid, c.gid, c.egid, c.sgid] != [10123, 10124, 10125, 20123, 20124, 20125]
        || c.group_count != 2
    {
        return -2;
    }
    if unsafe { std::slice::from_raw_parts(c.groups, c.group_count) } != [3003, 3009]
        || (c.permitted, c.effective, c.inheritable) != (3, 1, 2)
    {
        return -3;
    }
    if base.environment_count != 1 || base.property_count != 1 {
        return -4;
    }
    let environment = unsafe { &*base.environment };
    if unsafe { std::slice::from_raw_parts(environment.value, environment.value_size) } != b"value"
    {
        return -5;
    }
    0
}

#[test]
fn explicit_credentials_keep_wire_spans_alive_and_do_not_fallback() {
    let inputs = ProcessSnapshotInputs::new(
        vec![(b"key".to_vec(), b"value".to_vec())],
        vec![(b"prop".to_vec(), b"property".to_vec())],
        16384,
        3,
        0,
        false,
        [27; 16],
    );
    assert_eq!(
        inputs.install_with_credentials(&credentials(), inspect),
        Ok(())
    );
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "C" fn reject(_: *const ProcessSnapshotConfig) -> i32 {
        CALLS.fetch_add(1, Ordering::SeqCst);
        -9
    }
    assert_eq!(
        inputs.install_with_credentials(&credentials(), reject),
        Err(ProcessSnapshotError::NativeFailure(-9))
    );
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
}
