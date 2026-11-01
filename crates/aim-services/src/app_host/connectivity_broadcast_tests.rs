//! ConnectivityService legacy broadcasts and SyncManager's active-network read.
use super::super::{broadcasts, kept::RawBundle};
use super::*;
use crate::{bundle::Value as Extra, package::intent_filter::IntentFilter};
use aim_service_aidl::{ReadParcelable, android_app_iapplicationthread as thread};
use std::sync::mpsc;

const ACTION: &str = "android.net.conn.CONNECTIVITY_CHANGE";

pub(in crate::app_host) fn lease(last: u8) -> Option<(Uplink, Vec<Ipv4Addr>)> {
    Some((
        Uplink {
            index: 4,
            addr: [192, 0, 2, last].into(),
            prefix: 24,
            gateway: [192, 0, 2, 1].into(),
            mtu: 1500,
        },
        vec![],
    ))
}
pub(in crate::app_host) struct Receiver(pub(in crate::app_host) mpsc::Sender<(RawIntent, bool)>);
impl Service for Receiver {
    fn descriptor(&self) -> &str {
        thread::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, thread::SCHEDULE_REGISTERED_RECEIVER);
        let a = thread::ScheduleRegisteredReceiver::<RawIntent, RawBundle>::read(&mut call.data)?;
        assert!(!a.ordered && a.assume_delivered);
        assert_eq!(a.sending_user, -1);
        self.0.send((a.intent.unwrap(), a.sticky)).unwrap();
        Ok(Parcel::new())
    }
}
pub(in crate::app_host) fn register(
    host: &Arc<AppHost>,
    binder: Binder,
    receiver: bool,
) -> Option<RawIntent> {
    let mut filter = IntentFilter::default();
    filter.add_action(ACTION);
    let mut p = Parcel::new();
    filter.write(&mut p);
    host.broadcasts
        .register(
            host,
            &Call {
                code: 0,
                flags: 0,
                sender_pid: 1000,
                sender_euid: 1000,
                data: Parcel::new().reader(),
            },
            broadcasts::Registration {
                caller: Some(binder),
                caller_package: Some("p".into()),
                feature_id: None,
                receiver_id: None,
                receiver: receiver.then_some(binder),
                filter: Some(broadcasts::ParcelFilter::read_from(&mut p.reader()).unwrap()),
                permission: None,
                user: -1,
                flags: 2,
            },
        )
        .unwrap()
}
fn probe(
    host: &Arc<AppHost>,
    export: &dyn Fn(Arc<dyn Service>) -> Strong,
) -> (Binder, mpsc::Receiver<(RawIntent, bool)>) {
    let permissions = super::super::permission_manager::PermissionManager::with_system(
        host.clone(),
        Default::default(),
    );
    assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
    let _ = super::super::broadcasts::internal_package_tests::register_filter(
        host, export, "p", 1000, false,
    );
    let (tx, rx) = mpsc::channel();
    let probe = export(Arc::new(Receiver(tx)));
    let binder = probe.binder();
    host.apps.lock().unwrap().get_mut(&1000).unwrap().thread = Some(probe);
    (binder, rx)
}
fn check(intent: &RawIntent, connected: bool, replacement: bool) {
    assert_eq!(intent.intent.action.as_deref(), Some(ACTION));
    assert_eq!(intent.intent.flags & 0x0420_0000, 0x0420_0000);
    let extras = intent.test_extras();
    assert_eq!(extras["networkType"], Extra::Int(9));
    assert_eq!(
        extras["inetCondition"],
        Extra::Int(if connected || replacement { 100 } else { 0 })
    );
    assert_eq!(
        extras.get("noConnectivity"),
        if !connected && !replacement {
            Some(&Extra::Bool(true))
        } else {
            None
        }
    );
    assert!(
        !extras.contains_key("otherNetwork")
            && !extras.contains_key("isFailover")
            && !extras.contains_key("reason")
    );
    let extra = if connected || replacement {
        Some(HW_ADDRESS.to_string())
    } else {
        None
    };
    let expected_extra = extra.clone().map(|e| Extra::String(Some(e)));
    assert_eq!(extras.get("extraInfo"), expected_extra.as_ref());
    let Extra::Lazy { start, .. } = extras["networkInfo"] else {
        panic!("NetworkInfo parcelable")
    };
    let p = intent.parcel();
    let mut r = p.reader();
    r.set_position(start);
    assert_eq!(
        r.read_string16().unwrap().as_deref(),
        Some("android.net.NetworkInfo")
    );
    assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (9, -1));
    assert_eq!(r.read_string16().unwrap().as_deref(), Some("Ethernet"));
    assert_eq!(r.read_string16().unwrap().as_deref(), Some(""));
    let state = if connected {
        "CONNECTED"
    } else {
        "DISCONNECTED"
    };
    for _ in 0..2 {
        assert_eq!(r.read_string16().unwrap().as_deref(), Some(state));
    }
    for value in [0, 1, 0] {
        assert_eq!(r.read_i32().unwrap(), value);
    }
    assert_eq!(r.read_string16().unwrap(), None);
    assert_eq!(r.read_string16().unwrap(), extra);
}
pub(in crate::app_host) fn active(cs: &ConnectivityService) -> bool {
    let mut p = Parcel::new();
    cm::GetActiveNetworkInfo {}.write(&mut p);
    let result = cs
        .transact(&mut Call {
            code: cm::GET_ACTIVE_NETWORK_INFO,
            flags: 0,
            sender_pid: 1000,
            sender_euid: 1000,
            data: p.reader(),
        })
        .unwrap();
    let mut r = result.reader();
    r.read_exception().unwrap().unwrap();
    if r.read_i32().unwrap() == 0 {
        return false;
    }
    r.read_i32().unwrap();
    r.read_i32().unwrap();
    r.read_string16().unwrap();
    r.read_string16().unwrap();
    r.read_string16().unwrap().as_deref() == Some("CONNECTED")
}
pub(in crate::app_host) fn receive(
    rx: &mpsc::Receiver<(RawIntent, bool)>,
    connected: bool,
    replacement: bool,
    sticky: bool,
) {
    let (intent, initial) = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    check(&intent, connected, replacement);
    assert_eq!(initial, sticky);
}
#[test]
fn initial_connectivity_is_sticky_and_replayed_to_late_registered_receiver() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let cs = ConnectivityService::for_test(host.clone(), lease(2));
        cs.system_ready();
        let (binder, rx) = probe(host, export);
        check(&register(host, binder, true).unwrap(), true, false);
        receive(&rx, true, false, true);
        cs.system_ready();
        cs.follow_network(lease(2));
        assert!(rx.try_recv().is_err());
        assert!(active(&cs));
    });
}
#[test]
fn registered_receiver_tracks_disconnect_reconnect_replacement_and_suppresses_duplicates() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let cs = ConnectivityService::for_test(host.clone(), None);
        let (binder, rx) = probe(host, export);
        assert!(register(host, binder, true).is_none());
        cs.system_ready();
        cs.follow_network(None);
        assert!(rx.try_recv().is_err()); // Android has no initial loss without an agent.
        assert!(!active(&cs));
        cs.follow_network(lease(2));
        receive(&rx, true, false, false);
        assert!(active(&cs));
        cs.follow_network(lease(2));
        let mut dns = lease(2).unwrap();
        dns.1.push([1, 1, 1, 1].into());
        cs.follow_network(Some(dns));
        assert!(rx.try_recv().is_err());
        cs.follow_network(None);
        receive(&rx, false, false, false);
        assert!(!active(&cs));
        check(&register(host, binder, false).unwrap(), false, false);
        cs.follow_network(None);
        assert!(rx.try_recv().is_err());
        cs.follow_network(lease(3));
        receive(&rx, true, false, false);
        assert!(active(&cs));
        cs.follow_network(lease(4));
        receive(&rx, false, true, false);
        receive(&rx, true, false, false);
        // SyncManager must reread: even the replacement's DISCONNECTED intent
        // observes the new active network, so it does not invent an outage.
        assert!(active(&cs));
        assert_eq!(cs.default_network().unwrap().0, Network(102));
        assert!(rx.try_recv().is_err());
    });
}
#[test]
fn pre_ready_transitions_replay_only_the_last_real_event() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let cs = ConnectivityService::for_test(host.clone(), lease(2));
        let (binder, rx) = probe(host, export);
        register(host, binder, true);
        cs.follow_network(None);
        assert!(rx.try_recv().is_err());
        cs.system_ready();
        receive(&rx, false, false, false);
        assert!(!active(&cs));
        cs.system_ready();
        assert!(rx.try_recv().is_err());
    });
}
