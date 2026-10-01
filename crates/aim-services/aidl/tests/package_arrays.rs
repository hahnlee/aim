use aim_binder_host::parcel::{Parcel, Reader, Result};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;
use aim_service_aidl::android_content_pm_ipackagemanagernative as native;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

#[derive(Debug, PartialEq)]
struct Item(i32);

impl ReadParcelable for Item {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        Ok(Item(r.read_i32()?))
    }
}

impl WriteParcelable for Item {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.0);
    }
}

#[test]
fn package_intent_array_call_round_trips() {
    let call = pm::QueryIntentActivityOptions {
        caller: Some(Item(4)),
        specifics: Some(vec![Some(Item(5)), None]),
        specific_types: Some(vec![Some("text/plain".into()), None]),
        intent: Some(Item(6)),
        resolved_type: None,
        flags: 7,
        user_id: 0,
    };
    let mut parcel = Parcel::new();
    call.write(&mut parcel);
    let read = pm::QueryIntentActivityOptions::<Item, Item>::read(&mut Reader::new(
        parcel.data(),
        parcel.objects(),
    ))
    .unwrap();
    assert_eq!(read.caller, call.caller);
    assert_eq!(read.specifics, call.specifics);
    assert_eq!(read.specific_types, call.specific_types);
    assert_eq!(read.intent, call.intent);
    assert_eq!(read.flags, call.flags);
}

#[test]
fn native_array_replies_round_trip() {
    let mut bools = Parcel::new();
    native::write_is_audio_playback_capture_allowed_reply(&mut bools, &Some(vec![true, false]));
    let read = native::read_is_audio_playback_capture_allowed_reply(&mut Reader::new(
        bools.data(),
        bools.objects(),
    ))
    .unwrap()
    .unwrap();
    assert_eq!(read, Some(vec![true, false]));

    let mut apex = Parcel::new();
    native::write_get_staged_apex_infos_reply(&mut apex, Some(&[Some(Item(8)), None]));
    let read = native::read_get_staged_apex_infos_reply::<Item>(&mut Reader::new(
        apex.data(),
        apex.objects(),
    ))
    .unwrap()
    .unwrap();
    assert_eq!(read, Some(vec![Some(Item(8)), None]));
}

#[test]
fn package_output_lists_and_string_map_round_trip() {
    let call = pm::QuerySyncProviders {
        out_names: Some(vec![Some("before".into())]),
        out_info: Some(vec![Some(Item(1))]),
    };
    let mut data = Parcel::new();
    call.write(&mut data);
    let read = pm::QuerySyncProviders::<Item>::read(&mut Reader::new(data.data(), data.objects()))
        .unwrap();
    assert_eq!(read.out_names, call.out_names);
    assert_eq!(read.out_info, call.out_info);

    let reply = pm::GetPreferredActivitiesReply {
        result: 2,
        out_filters: Some(vec![Some(Item(3))]),
        out_activities: Some(vec![Some(Item(4)), None]),
    };
    let mut parcel = Parcel::new();
    pm::write_get_preferred_activities_reply(&mut parcel, &reply);
    let read = pm::read_get_preferred_activities_reply::<Item, Item>(&mut Reader::new(
        parcel.data(),
        parcel.objects(),
    ))
    .unwrap()
    .unwrap();
    assert_eq!(read.result, reply.result);
    assert_eq!(read.out_filters, reply.out_filters);
    assert_eq!(read.out_activities, reply.out_activities);

    let home = pm::GetHomeActivitiesReply {
        result: Some(Item(5)),
        out_home_candidates: Some(vec![Some(Item(6))]),
    };
    let mut parcel = Parcel::new();
    pm::write_get_home_activities_reply(&mut parcel, &home);
    let read = pm::read_get_home_activities_reply::<Item, Item>(&mut Reader::new(
        parcel.data(),
        parcel.objects(),
    ))
    .unwrap()
    .unwrap();
    assert_eq!(read.result, home.result);
    assert_eq!(read.out_home_candidates, home.out_home_candidates);

    let pairs = Some(vec![(Some("a".into()), Some("b".into())), (None, None)]);
    let mut parcel = Parcel::new();
    pm::write_get_system_shared_library_names_and_paths_reply(&mut parcel, &pairs);
    let read = pm::read_get_system_shared_library_names_and_paths_reply(&mut Reader::new(
        parcel.data(),
        parcel.objects(),
    ))
    .unwrap()
    .unwrap();
    assert_eq!(read, pairs);
}
