//! Test-only Mach inspection; no intentional faults or crash report files.
use super::{MAPPING_TEST_LOCK, ReceiveMapping};

unsafe extern "C" {
    static mach_task_self_: u32;
    fn mach_vm_region(
        task: u32,
        address: *mut u64,
        size: *mut u64,
        flavor: i32,
        info: *mut i32,
        count: *mut u32,
        object: *mut u32,
    ) -> i32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
}

fn protection_at(request: usize) -> Option<i32> {
    let mut address = request as u64;
    let mut size = 0;
    // SDK VM_REGION_INFO_MAX=1024; BASIC_INFO_64=9. Its first integer is
    // vm_prot_t protection. Use the API's integer buffer, not a guessed struct.
    let mut info = [0i32; 1024];
    let mut count = info.len() as u32;
    let mut object = 0;
    // SAFETY: query own task, all output buffers have the declared capacity.
    let task = unsafe { mach_task_self_ };
    let result = unsafe {
        mach_vm_region(
            task,
            &mut address,
            &mut size,
            9,
            info.as_mut_ptr(),
            &mut count,
            &mut object,
        )
    };
    if object != 0 {
        // SAFETY: release exactly the send right returned by this query.
        assert_eq!(unsafe { mach_port_deallocate(task, object) }, 0);
    }
    if result == 1 {
        return None;
    } // KERN_INVALID_ADDRESS: no following region.
    assert_eq!(result, 0);
    assert!(count > 0);
    if address > request as u64 {
        None
    } else {
        Some(info[0])
    }
}

#[test]
fn actual_vm_protection_and_owner_drop() {
    // Any other test thread may mmap the released range before the check
    // (#21), so inspect in a copy of this binary running only this test.
    const ISOLATED: &str = "DARWIN_ART_MAPPING_PROTECTION_ISOLATED";
    if std::env::var_os(ISOLATED).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "mapping::tests::protection::actual_vm_protection_and_owner_drop",
                "--exact",
                "--test-threads=1",
            ])
            .env(ISOLATED, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let _guard = MAPPING_TEST_LOCK.lock().unwrap();
    let map = ReceiveMapping::new(32768).unwrap();
    let reader = map.client_address();
    let writer = map.writer.address.as_ptr() as usize;
    assert_eq!(protection_at(reader), Some(libc::PROT_READ));
    assert_eq!(
        protection_at(writer),
        Some(libc::PROT_READ | libc::PROT_WRITE)
    );
    drop(map);
    // No heap allocations between Drop and inspection, and no other test
    // runs in this process.
    assert_eq!(protection_at(reader), None);
    assert_eq!(protection_at(writer), None);
}
