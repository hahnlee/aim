//! ConnectivityDiagnosticsManagerTest's registration/permission contracts and native quota owner.
use super::*;
use aim_binder_host::parcel::EX_SECURITY;
use std::sync::mpsc;

struct Bridge {
    tx: mpsc::Sender<(u32, i32)>,
}
impl Service for Bridge {
    fn descriptor(&self) -> &str {
        activity::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.sender_euid, SYSTEM_UID as u32);
        let mut p = Parcel::new();
        match call.code {
            activity::ON_DEFAULT_NETWORK_CHANGED => {
                activity::OnDefaultNetworkChanged::read(&mut call.data)?;
            }
            activity::ON_DIAGNOSTICS_NETWORK_CHANGED => {
                call.data.enforce_interface(activity::DESCRIPTOR)?;
                let network = aim_service_aidl::read_typed::<Network>(&mut call.data)?;
                assert_eq!(call.data.read_i32()?, i32::from(network.is_some()));
                let end = call.data.position() + call.data.remaining();
                call.data.set_position(end - 8);
                let timestamp = call.data.read_i64()?;
                if let Some(net) = network {
                    assert!(timestamp > 0);
                    self.tx.send((call.code, net.0)).unwrap();
                }
            }
            activity::REGISTER_DIAGNOSTICS => {
                let a = activity::RegisterDiagnostics::<NetworkCapabilities>::read(&mut call.data)?;
                assert_eq!((a.uid, a.pid), (10100, 42));
                let caps = a.request.unwrap();
                assert_eq!(caps.owner_uid, INVALID_UID);
                assert!(caps.administrator_uids.is_empty());
                assert_eq!(caps.requestor_package.as_deref(), Some("p"));
                assert_eq!(caps.requestor_uid, a.uid);
                assert!(a.callback.is_some());
                self.tx.send((call.code, a.request_id)).unwrap();
                activity::write_register_diagnostics_reply(&mut p);
            }
            activity::UNREGISTER_DIAGNOSTICS => {
                let a = activity::UnregisterDiagnostics::read(&mut call.data)?;
                self.tx.send((call.code, a.uid)).unwrap();
                activity::write_unregister_diagnostics_reply(&mut p);
            }
            activity::ON_NETWORK_CONNECTIVITY_REPORTED => {
                let a = activity::OnNetworkConnectivityReported::<Network>::read(&mut call.data)?;
                assert_eq!(a.revalidating, !a.connected);
                self.tx.send((call.code, a.uid)).unwrap();
            }
            _ => panic!("unexpected diagnostics call {}", call.code),
        }
        Ok(p)
    }
}
fn call(service: &dyn Service, uid: u32, code: u32, p: &Parcel) -> Result<(), Exception> {
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 42,
            sender_euid: uid,
            data: p.reader(),
        })
        .unwrap()
        .reader()
        .read_exception()
        .unwrap()
}

#[test]
fn diagnostics_checks_package_not_access_and_shares_quota_with_network_requests() {
    use aim_service_aidl::dev_aim_server_isystemprocess as sp;
    crate::system::with_discovery_binders(|host, _, _, export| {
        let permissions = super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            Default::default(),
        );
        assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
        let cs = Arc::new(ConnectivityService::for_test(host.clone(), None));
        assert!(host.connectivity_service.set(cs.clone()).is_ok());
        let (tx, rx) = mpsc::channel();
        let owner = export(Arc::new(Bridge { tx }));
        cs.attach_activity(owner).unwrap();
        let callback = export(Arc::new(Bridge {
            tx: mpsc::channel().0,
        }));
        let mut caps = NetworkCapabilities::default();
        caps.owner_uid = 1234;
        caps.administrator_uids = vec![1234];
        let mut a = cm::RegisterConnectivityDiagnosticsCallback {
            callback: Some(callback.binder()),
            request: Some(NetworkRequest {
                caps,
                legacy_type: TYPE_NONE,
                request_id: 123,
                kind: RequestType::Listen,
            }),
            calling_package_name: Some("q".into()),
        };
        let mut p = Parcel::new();
        a.write(&mut p);
        assert_eq!(
            call(
                cs.as_ref(),
                10100,
                cm::REGISTER_CONNECTIVITY_DIAGNOSTICS_CALLBACK,
                &p
            )
            .unwrap_err()
            .code,
            EX_SECURITY
        );
        assert!(rx.try_recv().is_err());
        a.calling_package_name = Some("p".into());
        let mut p = Parcel::new();
        a.write(&mut p);
        assert!(!cs.has(10100, ACCESS_NETWORK_STATE).unwrap());
        call(
            cs.as_ref(),
            10100,
            cm::REGISTER_CONNECTIVITY_DIAGNOSTICS_CALLBACK,
            &p,
        )
        .unwrap();
        let (_, id) = rx.recv().unwrap();
        call(
            cs.as_ref(),
            10100,
            cm::REGISTER_CONNECTIVITY_DIAGNOSTICS_CALLBACK,
            &p,
        )
        .unwrap();
        let (_, duplicate_id) = rx.recv().unwrap();
        cs.release_diagnostics(duplicate_id); // Java discards the duplicate on its owning handler.
        assert_eq!(cs.shared.state.lock().unwrap().counts[&(false, 10100)], 1);
        {
            let mut state = cs.shared.state.lock().unwrap();
            for _ in 1..MAX_REQUESTS_PER_UID {
                state.admit(false, 10100).unwrap();
            }
        }
        assert_eq!(
            call(
                cs.as_ref(),
                10100,
                cm::REGISTER_CONNECTIVITY_DIAGNOSTICS_CALLBACK,
                &p
            )
            .unwrap_err()
            .service_specific,
            TOO_MANY_REQUESTS
        );
        let endpoint = super::super::system_process::SystemProcess::new(host.clone());
        let mut p = Parcel::new();
        sp::ReleaseConnectivityDiagnosticsRequest { request_id: id }.write(&mut p);
        assert_eq!(
            call(
                &endpoint,
                10100,
                sp::RELEASE_CONNECTIVITY_DIAGNOSTICS_REQUEST,
                &p
            )
            .unwrap_err()
            .code,
            EX_SECURITY
        );
        call(
            &endpoint,
            1000,
            sp::RELEASE_CONNECTIVITY_DIAGNOSTICS_REQUEST,
            &p,
        )
        .unwrap();
        assert_eq!(
            cs.shared.state.lock().unwrap().counts[&(false, 10100)],
            MAX_REQUESTS_PER_UID - 1
        );
        let mut p = Parcel::new();
        cm::UnregisterConnectivityDiagnosticsCallback {
            callback: Some(callback.binder()),
        }
        .write(&mut p);
        call(
            cs.as_ref(),
            10100,
            cm::UNREGISTER_CONNECTIVITY_DIAGNOSTICS_CALLBACK,
            &p,
        )
        .unwrap();
        assert_eq!(
            rx.recv().unwrap(),
            (activity::UNREGISTER_DIAGNOSTICS, 10100)
        );
    });
}

#[test]
fn diagnostics_network_reports_follow_acceptance_and_reporter_permission_checks() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let permissions = super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            Default::default(),
        );
        assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
        let cs = ConnectivityService::for_test(host.clone(), None);
        let (tx, rx) = mpsc::channel();
        cs.attach_activity(export(Arc::new(Bridge { tx }))).unwrap();
        let lease = (
            Uplink {
                index: 4,
                addr: Ipv4Addr::new(192, 168, 1, 2),
                prefix: 24,
                gateway: Ipv4Addr::new(192, 168, 1, 1),
                mtu: 1500,
            },
            vec![],
        );
        cs.follow_network(Some(lease.clone()));
        assert_eq!(
            rx.recv().unwrap(),
            (activity::ON_DIAGNOSTICS_NETWORK_CHANGED, MIN_NET_ID)
        );
        cs.follow_network(Some((lease.0, vec![Ipv4Addr::new(1, 1, 1, 1)])));
        assert!(rx.try_recv().is_err());
        for connected in [true, false] {
            let mut p = Parcel::new();
            cm::ReportNetworkConnectivity {
                network: Some(Network(MIN_NET_ID)),
                has_connectivity: connected,
            }
            .write(&mut p);
            assert_eq!(
                call(&cs, 10100, cm::REPORT_NETWORK_CONNECTIVITY, &p)
                    .unwrap_err()
                    .code,
                EX_SECURITY
            );
            assert!(rx.try_recv().is_err());
            call(&cs, 1000, cm::REPORT_NETWORK_CONNECTIVITY, &p).unwrap();
            assert_eq!(
                rx.recv().unwrap(),
                (activity::ON_NETWORK_CONNECTIVITY_REPORTED, 1000)
            );
            if !connected {
                assert_eq!(
                    rx.recv().unwrap(),
                    (activity::ON_DIAGNOSTICS_NETWORK_CHANGED, MIN_NET_ID)
                );
            }
        }
        cs.follow_network(None);
        let mut p = Parcel::new();
        cm::ReportNetworkConnectivity {
            network: Some(Network(MIN_NET_ID)),
            has_connectivity: true,
        }
        .write(&mut p);
        call(&cs, 1000, cm::REPORT_NETWORK_CONNECTIVITY, &p).unwrap();
        assert!(rx.try_recv().is_err());
    });
}
