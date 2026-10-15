use aim_binder_host::parcel::{Parcel, Reader};
use aim_service_aidl::android_os_iinstalld as installd;

#[test]
fn app_size_long_arrays_round_trip() {
    let call = installd::GetAppSize {
        uuid: None,
        package_names: Some(vec![Some("org.example.app".into())]),
        user_id: 10,
        flags: 2,
        app_id: 10042,
        ce_data_inodes: Some(vec![i64::MAX, -1]),
        code_paths: Some(vec![None]),
    };
    let mut parcel = Parcel::new();
    call.write(&mut parcel);
    let read = installd::GetAppSize::read(&mut Reader::new(parcel.data(), parcel.objects()))
        .unwrap();
    assert_eq!(read.package_names, call.package_names);
    assert_eq!(read.ce_data_inodes, call.ce_data_inodes);
    assert_eq!(read.code_paths, call.code_paths);

    for expected in [None, Some(vec![]), Some(vec![0, i64::MAX, i64::MIN])] {
        let mut parcel = Parcel::new();
        installd::write_get_app_size_reply(&mut parcel, &expected);
        let read = installd::read_get_app_size_reply(&mut Reader::new(
            parcel.data(),
            parcel.objects(),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(read, expected);
    }
}
