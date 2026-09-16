//! Property version queries, sharing the same owner as find/read callbacks.
use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_process_property_area_serial_core() -> u32 {
    let Some(snapshot) = active_snapshot() else {
        missing_snapshot();
        return 0;
    };
    snapshot.properties.area_serial()
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_process_property_serial_core(
    property: *const std::ffi::c_void,
) -> u32 {
    let Some(snapshot) = active_snapshot() else {
        missing_snapshot();
        return 0;
    };
    match snapshot.properties.serial(property) {
        Ok(Some(serial)) => serial,
        Ok(None) => {
            set_errno(ANDROID_EFAULT);
            0
        }
        Err(_) => {
            CAPABILITY_FAILURE.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    unsafe extern "C" {
        fn darwin_art_bionic___system_property_serial(property: *const std::ffi::c_void) -> u32;
        fn darwin_art_bionic___system_property_area_serial() -> u32;
        fn darwin_art_bionic___system_property_wait(
            property: *const std::ffi::c_void,
            old: u32,
            output: *mut u32,
            timeout: *const crate::property_wait_abi::WaitTimeout,
        ) -> bool;
        fn __error() -> *mut i32;
    }
    #[test]
    fn c_versions_share_updates_and_preserve_host_errno() {
        let _test_owner_lock = crate::test_process_owner_guard();
        let snapshot = Arc::new(
            Snapshot::new(
                vec![],
                vec![(b"cache.key".to_vec(), b"first".to_vec())],
                AuxSnapshot {
                    page_size: 16384,
                    hwcap: 3,
                    hwcap2: 0,
                    secure: false,
                    random: [0; 16],
                },
            )
            .unwrap(),
        );
        let active = snapshot.activate().unwrap();
        let token = snapshot.properties.find(b"cache.key").unwrap();
        unsafe {
            *__error() = 12345;
            let before = darwin_art_bionic___system_property_serial(token);
            let area = darwin_art_bionic___system_property_area_serial();
            assert_eq!(*__error(), 12345);
            let mut output = 777;
            let zero = crate::property_wait_abi::WaitTimeout {
                seconds: 0,
                nanoseconds: 0,
            };
            assert!(!darwin_art_bionic___system_property_wait(
                token,
                before,
                &mut output,
                &zero
            ));
            assert_eq!(output, 777);
            assert_eq!(*__error(), 12345);
            snapshot.properties.update(b"cache.key", b"next").unwrap();
            assert_eq!(
                darwin_art_bionic___system_property_area_serial(),
                area.wrapping_add(1)
            );
            assert_ne!(darwin_art_bionic___system_property_serial(token), before);
            assert!(darwin_art_bionic___system_property_wait(
                token,
                before,
                &mut output,
                &zero
            ));
            assert_eq!(output, darwin_art_bionic___system_property_serial(token));
            let invalid = crate::property_wait_abi::WaitTimeout {
                seconds: 0,
                nanoseconds: -1,
            };
            output = 777;
            assert!(!darwin_art_bionic___system_property_wait(
                token,
                before,
                &mut output,
                &invalid
            ));
            assert_eq!(output, 777);
            assert_eq!(
                darwin_art_bionic___system_property_serial(token),
                snapshot.properties.read(token).unwrap().unwrap().serial
            );
        }
        let token_address = token as usize;
        assert_eq!(
            std::thread::spawn(move || unsafe {
                darwin_art_bionic___system_property_serial(token_address as *const _)
            })
            .join()
            .unwrap(),
            snapshot.properties.serial(token).unwrap().unwrap()
        );
        drop(active);
    }
}
