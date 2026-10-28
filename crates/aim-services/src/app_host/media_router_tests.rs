//! Pinned MediaRouterService/MediaRouter2ServiceImpl query boundaries; Java policy
//! is executed separately by tests/system_route_queries.rs, without a guest boot.
use super::*;
use aim_binder_host::parcel::{EX_NULL_POINTER, EX_SECURITY, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_isystemprocess as sp;

#[derive(Debug, PartialEq)]
enum Query {
    Routes(i32, i32, Option<String>, bool),
    Session(i32, i32, Option<String>, Option<String>, bool),
}
struct Original(Arc<Mutex<Vec<Query>>>);
impl Service for Original {
    fn descriptor(&self) -> &str {
        queries::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.sender_euid, 1000);
        assert_eq!(call.flags & 1, 0);
        let mut p = Parcel::new();
        match call.code {
            queries::GET_SYSTEM_ROUTES => {
                let a = queries::GetSystemRoutes::read(&mut call.data)?;
                self.0.lock().unwrap().push(Query::Routes(
                    a.uid,
                    a.pid,
                    a.caller_package_name,
                    a.is_proxy_router,
                ));
                queries::write_get_system_routes_reply(&mut p, &Some(vec![0, 0, 0, 0]));
            }
            queries::GET_SYSTEM_SESSION_INFO => {
                let a = queries::GetSystemSessionInfo::read(&mut call.data)?;
                let target = a.target_package_name.clone();
                self.0.lock().unwrap().push(Query::Session(
                    a.uid,
                    a.pid,
                    a.caller_package_name,
                    a.target_package_name,
                    a.set_device_route_selected,
                ));
                match target.as_deref() {
                    Some("denied") => p.write_exception(&Exception::security("original denial")),
                    Some("transport") => return Err(UNKNOWN_TRANSACTION),
                    Some("null") => queries::write_get_system_session_info_reply(&mut p, &None),
                    _ => queries::write_get_system_session_info_reply(
                        &mut p,
                        &Some(vec![0, 0, 0, 0]),
                    ),
                }
            }
            _ => panic!("unexpected query"),
        }
        Ok(p)
    }
}
fn call(service: &impl Service, code: u32, data: &Parcel, uid: u32) -> Parcel {
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 71,
            sender_euid: uid,
            data: data.reader(),
        })
        .unwrap()
}

#[test]
fn queries_validate_packages_forward_caller_and_keep_original_payloads_and_errors() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let service = MediaRouter::new(host.clone());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let owner = export(Arc::new(Original(calls.clone())));
        let system = super::super::system_process::SystemProcess::new(host.clone());
        let mut attach = Parcel::new();
        sp::AttachSystemRouteQueries {
            queries: Some(owner.binder()),
        }
        .write(&mut attach);
        sp::read_attach_system_route_queries_reply(
            &mut call(&system, sp::ATTACH_SYSTEM_ROUTE_QUERIES, &attach, 1000).reader(),
        )
        .unwrap()
        .unwrap();
        for proxy in [false, true] {
            for (uid, package) in [(10100, None), (10100, Some("q")), (10101, Some("p"))] {
                let mut data = Parcel::new();
                mr::GetSystemRoutes {
                    caller_package_name: package.map(str::to_owned),
                    is_proxy_router: proxy,
                }
                .write(&mut data);
                assert_eq!(
                    call(&service, mr::GET_SYSTEM_ROUTES, &data, uid)
                        .reader()
                        .read_exception()
                        .unwrap()
                        .unwrap_err()
                        .code,
                    EX_SECURITY
                );
            }
            let mut data = Parcel::new();
            mr::GetSystemRoutes {
                caller_package_name: Some("p".into()),
                is_proxy_router: proxy,
            }
            .write(&mut data);
            let response = call(&service, mr::GET_SYSTEM_ROUTES, &data, 10100);
            let mut r = response.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 0);
            assert_eq!(r.remaining(), 0);
        }
        let mut data = Parcel::new();
        mr::GetSystemSessionInfo {}.write(&mut data);
        let response = call(&service, mr::GET_SYSTEM_SESSION_INFO, &data, 10100);
        let mut r = response.reader();
        r.read_exception().unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.remaining(), 0);
        // The paired proxy query checks ownership before dispatch too.
        let mut data = Parcel::new();
        mr::GetSystemSessionInfoForPackage {
            caller_package_name: Some("q".into()),
            target_package_name: Some("p".into()),
        }
        .write(&mut data);
        assert_eq!(
            call(
                &service,
                mr::GET_SYSTEM_SESSION_INFO_FOR_PACKAGE,
                &data,
                10100
            )
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err()
            .code,
            EX_SECURITY
        );
        assert_eq!(
            *calls.lock().unwrap(),
            [
                Query::Routes(10100, 71, Some("p".into()), false),
                Query::Routes(10100, 71, Some("p".into()), true),
                Query::Session(10100, 71, None, None, false),
            ]
        );
        for target in [
            None,
            Some("target"),
            Some("denied"),
            Some("transport"),
            Some("null"),
        ] {
            let mut data = Parcel::new();
            mr::GetSystemSessionInfoForPackage {
                caller_package_name: Some("p".into()),
                target_package_name: target.map(str::to_owned),
            }
            .write(&mut data);
            let response = call(
                &service,
                mr::GET_SYSTEM_SESSION_INFO_FOR_PACKAGE,
                &data,
                10100,
            );
            let error = response.reader().read_exception().unwrap();
            match target {
                Some("denied") => {
                    assert_eq!(error.unwrap_err(), Exception::security("original denial"))
                }
                Some("transport" | "null") => assert!(error.is_err()),
                _ => error.unwrap(),
            }
        }
        assert_eq!(
            calls.lock().unwrap().last(),
            Some(&Query::Session(
                10100,
                71,
                Some("p".into()),
                Some("null".into()),
                false
            ))
        );
        *host.system_route_queries.lock().unwrap() = None;
        let mut data = Parcel::new();
        mr::GetSystemSessionInfo {}.write(&mut data);
        let response = call(&service, mr::GET_SYSTEM_SESSION_INFO, &data, 10100);
        let error = response.reader().read_exception().unwrap().unwrap_err();
        assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
        assert!(error.message.contains("#1663"));
        // Out-of-scope manager operations remain explicit.
        let response = call(&service, mr::REGISTER_MANAGER, &Parcel::new(), 10100);
        let error = response.reader().read_exception().unwrap().unwrap_err();
        assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
        assert!(error.message.contains("#1663"));
    });
}

#[test]
fn attachment_is_system_uid_only_and_requires_a_live_binder() {
    let host = AppHost::new(crate::test_process(1000));
    let system = super::super::system_process::SystemProcess::new(host);
    let mut data = Parcel::new();
    sp::AttachSystemRouteQueries { queries: None }.write(&mut data);
    for (uid, expected) in [
        (0, EX_SECURITY),
        (2000, EX_SECURITY),
        (10100, EX_SECURITY),
        (1000, EX_NULL_POINTER),
    ] {
        assert_eq!(
            call(&system, sp::ATTACH_SYSTEM_ROUTE_QUERIES, &data, uid)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            expected
        );
    }
}
