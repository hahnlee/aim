//! DomainVerificationService verifier writes and paired queries, android-16.0.0_r1.
use super::*;
use crate::package::domain_verification::{State, collector, owner};
use aim_binder_host::parcel::{Binder, EX_SECURITY, EX_UNSUPPORTED_OPERATION};

fn invoke(service: &dyn Service, uid: u32, code: u32, request: &Parcel) -> Parcel {
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 77,
            sender_euid: uid,
            data: request.reader(),
        })
        .unwrap()
}

struct Frontend;
impl Service for Frontend {
    fn descriptor(&self) -> &str {
        original::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // The native call clears the app's Binder identity. Only the explicit
        // authenticated UID reaches the original Java permission enforcer.
        assert_eq!(call.sender_euid, 1000);
        let mut reply = Parcel::new();
        match call.code {
            original::QUERY_VALID_VERIFICATION_PACKAGE_NAMES => {
                let a = original::QueryValidVerificationPackageNames::read(&mut call.data)?;
                assert_eq!(a.uid, 10100);
                original::write_query_valid_verification_package_names_reply(
                    &mut reply,
                    &Some(vec![Some("p".into())]),
                );
            }
            original::GET_DOMAIN_VERIFICATION_INFO => {
                let a = original::GetDomainVerificationInfo::read(&mut call.data)?;
                assert_eq!(a.uid, 10100);
                assert_eq!(a.package_name.as_deref(), Some("p"));
                reply.write_exception(&Exception::security("original permission denial"));
            }
            original::SET_DOMAIN_VERIFICATION_STATUS => {
                let a = original::SetDomainVerificationStatus::<DomainSet>::read(&mut call.data)?;
                assert_eq!(a.uid, 10100);
                assert_eq!(
                    a.id.as_deref(),
                    Some("00000000-0000-0000-0000-000000000001")
                );
                assert_eq!(
                    a.domains
                        .unwrap()
                        .resolve(&crate::test_process(1000))
                        .unwrap(),
                    [Some("example.org".into())]
                );
                assert_eq!(a.state, 1);
                original::write_set_domain_verification_status_reply(&mut reply, 2);
            }
            _ => unreachable!(),
        }
        assert_eq!(call.data.remaining(), 0);
        Ok(reply)
    }
}
fn set_request(id: &str, hosts: &[Option<&str>], state: i32) -> Parcel {
    let mut p = Parcel::new();
    p.write_interface_token(dv::DESCRIPTOR);
    p.write_string16(Some(id));
    p.write_i32(1);
    p.write_bool(false);
    p.write_i32(hosts.len() as i32);
    for host in hosts {
        p.write_string16(*host);
    }
    p.write_i32(state);
    p
}

#[test]
fn original_frontend_receives_authenticated_uid_and_preserves_replies() {
    let process = crate::test_process(1000);
    let host = AppHost::new(process.clone());
    let public = DomainVerification::new(host.clone());
    let mut names = Parcel::new();
    dv::QueryValidVerificationPackageNames {}.write(&mut names);
    assert_eq!(
        invoke(
            &public,
            10100,
            dv::QUERY_VALID_VERIFICATION_PACKAGE_NAMES,
            &names
        )
        .reader()
        .read_exception()
        .unwrap()
        .unwrap_err()
        .code,
        EX_ILLEGAL_STATE
    );
    let Binder::Local(ptr) = process.add_service(Arc::new(Frontend)) else {
        unreachable!()
    };
    *host.domain_frontend.lock().unwrap() = Some(Arc::new(crate::system::ServiceOwner::Local(
        process.local_service(ptr).unwrap(),
    )));
    let reply = invoke(
        &public,
        10100,
        dv::QUERY_VALID_VERIFICATION_PACKAGE_NAMES,
        &names,
    );
    assert_eq!(
        dv::read_query_valid_verification_package_names_reply(&mut reply.reader())
            .unwrap()
            .unwrap(),
        Some(vec![Some("p".into())])
    );
    let mut info = Parcel::new();
    dv::GetDomainVerificationInfo {
        package_name: Some("p".into()),
    }
    .write(&mut info);
    let reply = invoke(&public, 10100, dv::GET_DOMAIN_VERIFICATION_INFO, &info);
    assert_eq!(
        reply.reader().read_exception().unwrap().unwrap_err().code,
        EX_SECURITY
    );
    let set = set_request(
        "00000000-0000-0000-0000-000000000001",
        &[Some("example.org")],
        1,
    );
    assert_eq!(
        dv::read_set_domain_verification_status_reply(
            &mut invoke(&public, 10100, dv::SET_DOMAIN_VERIFICATION_STATUS, &set).reader()
        )
        .unwrap()
        .unwrap(),
        2
    );
    let backend = Backend::new(host);
    for (code, request) in [
        (dv::QUERY_VALID_VERIFICATION_PACKAGE_NAMES, names),
        (dv::GET_DOMAIN_VERIFICATION_INFO, info),
        (dv::SET_DOMAIN_VERIFICATION_STATUS, set),
    ] {
        assert_eq!(
            invoke(&backend, 10100, code, &request)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
    }
    let error = invoke(
        &public,
        10100,
        dv::SET_DOMAIN_VERIFICATION_LINK_HANDLING_ALLOWED,
        &Parcel::new(),
    )
    .reader()
    .read_exception()
    .unwrap()
    .unwrap_err();
    assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
    assert!(error.message.contains("#1763"));
}

#[test]
fn uri_bundle_is_forwarded_opaque_and_paired_query_keeps_authenticated_caller() {
    use aim_service_aidl::{ReadParcelable, WriteParcelable};
    struct UriFrontend(Vec<u8>);
    impl Service for UriFrontend {
        fn descriptor(&self) -> &str {
            original::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.sender_euid, 1000);
            let mut reply = Parcel::new();
            match call.code {
                original::SET_URI_RELATIVE_FILTER_GROUPS => {
                    let a =
                        original::SetUriRelativeFilterGroups::<RawBundle>::read(&mut call.data)?;
                    assert_eq!(a.uid, 10100);
                    assert_eq!(a.package_name.as_deref(), Some("p"));
                    let mut bundle = Parcel::new();
                    a.groups.unwrap().write_to(&mut bundle);
                    assert_eq!(bundle.data(), self.0);
                    // Original Java errors must reach the app unchanged.
                    reply.write_exception(&Exception::security("URI agent permission denied"));
                }
                original::GET_URI_RELATIVE_FILTER_GROUPS => {
                    let a = original::GetUriRelativeFilterGroups::read(&mut call.data)?;
                    assert_eq!(a.uid, 10100);
                    assert_eq!(a.package_name.as_deref(), Some("p"));
                    assert_eq!(a.domains, Some(vec![Some("example.org".into()), None]));
                    original::write_get_uri_relative_filter_groups_reply(
                        &mut reply,
                        Some(&RawBundle::empty()),
                    );
                }
                _ => unreachable!(),
            }
            assert_eq!(call.data.remaining(), 0);
            Ok(reply)
        }
    }
    let process = crate::test_process(1000);
    let host = AppHost::new(process.clone());
    let public = DomainVerification::new(host.clone());
    let mut bundle = Parcel::new();
    // A lazy Serializable with deliberately invalid stream bytes. Rust must
    // forward it without inspecting the class or attempting deserialization.
    crate::bundle::write(
        &mut bundle,
        &[(
            "example.org",
            crate::bundle::Value::Serializable {
                class: "java.lang.String".into(),
                data: vec![0xff, 0, 1],
            },
        )],
    );
    let Binder::Local(ptr) = process.add_service(Arc::new(UriFrontend(bundle.data().to_vec())))
    else {
        unreachable!()
    };
    *host.domain_frontend.lock().unwrap() = Some(Arc::new(crate::system::ServiceOwner::Local(
        process.local_service(ptr).unwrap(),
    )));
    let mut set = Parcel::new();
    dv::SetUriRelativeFilterGroups {
        package_name: Some("p".into()),
        domain_to_groups_bundle: Some(RawBundle::read_from(&mut bundle.reader()).unwrap()),
    }
    .write(&mut set);
    let error = invoke(&public, 10100, dv::SET_URI_RELATIVE_FILTER_GROUPS, &set)
        .reader()
        .read_exception()
        .unwrap()
        .unwrap_err();
    assert_eq!(error, Exception::security("URI agent permission denied"));
    let mut get = Parcel::new();
    dv::GetUriRelativeFilterGroups {
        package_name: Some("p".into()),
        domains: Some(vec![Some("example.org".into()), None]),
    }
    .write(&mut get);
    let response = invoke(&public, 10100, dv::GET_URI_RELATIVE_FILTER_GROUPS, &get);
    let result = dv::read_get_uri_relative_filter_groups_reply::<RawBundle>(&mut response.reader())
        .unwrap()
        .unwrap()
        .unwrap();
    let mut parcel = Parcel::new();
    result.write_to(&mut parcel);
    assert_eq!(parcel.data(), &[0, 0, 0, 0]);
    let backend = Backend::new(host);
    for code in [
        original::SET_URI_RELATIVE_FILTER_GROUPS,
        original::GET_URI_RELATIVE_FILTER_GROUPS,
        original::SET_URI_RELATIVE_FILTER_GROUP,
        original::GET_DOMAIN_VERIFICATION_PACKAGE_ID,
    ] {
        assert_eq!(
            invoke(&backend, 10100, code, &Parcel::new())
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
    }
}

#[test]
fn verifier_status_publishes_paired_reads_and_persists_without_mutating_retained_capture() {
    crate::system::with_discovery_session(|host, publish, _| {
        publish(&mut |package| {
            let mut filter = crate::package::intent_filter::IntentFilter {
                auto_verify: true,
                ..Default::default()
            };
            filter.add_action("android.intent.action.VIEW");
            filter.add_category("android.intent.category.BROWSABLE");
            filter.add_category("android.intent.category.DEFAULT");
            filter.add_data_scheme("https");
            filter.add_data_authority("example.org", None);
            package.activities[0].main.component.intents.push(
                crate::package::intent_filter::ParsedIntentInfo {
                    filter,
                    ..Default::default()
                },
            );
        });
        let system = host.package_system().unwrap();
        let base = host.package_capture().unwrap();
        let scan = base.scan().owner();
        let config = crate::package::system_config::SystemConfig::default();
        let mut domains = owner::Owner::new(State::default(), Default::default());
        let mut policies = std::collections::BTreeMap::new();
        for setting in &scan.settings.packages {
            domains
                .add(
                    owner::Input {
                        id: setting.domain_set_id.as_deref().unwrap(),
                        name: &setting.name,
                        code: Some(scan.loaded_packages()[&setting.name].runtime_package()),
                        signatures: &[],
                        system: false,
                        restrict_domains: true,
                        pre_verified: None,
                    },
                    &config,
                )
                .unwrap();
            policies.insert(setting.name.clone(), true);
        }
        let mut context = (**base.context())
            .clone()
            .attach_boot_domains(
                scan,
                owner::Boot {
                    owner: domains,
                    changes: vec![],
                },
                &config,
                &policies,
            )
            .unwrap();
        context.scan_version += 1;
        let bridge = system.package_bootstrap().unwrap();
        let current = system
            .publish_package_scan_with_queries(
                &bridge,
                Some(base.scan()),
                scan.clone(),
                base.scan().usage().clone(),
                context,
            )
            .unwrap();
        let (_, disk) = system.package_persistence_owner(&current).unwrap();
        system
            .commit_package_domains(
                &bridge,
                current
                    .prepare_domain_update(current.domains().unwrap().owner().clone())
                    .unwrap(),
                &mut disk.lock().unwrap(),
            )
            .unwrap();
        let retained = host.package_capture().unwrap();
        let id = retained
            .domains()
            .unwrap()
            .owner()
            .package("p")
            .unwrap()
            .id
            .clone();
        let backend = Backend::new(host.clone());
        let mut names = Parcel::new();
        original::QueryValidVerificationPackageNames { uid: 10100 }.write(&mut names);
        let response = invoke(
            &backend,
            1000,
            original::QUERY_VALID_VERIFICATION_PACKAGE_NAMES,
            &names,
        );
        let names = dv::read_query_valid_verification_package_names_reply(&mut response.reader())
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(names.contains(&Some("p".into())));
        let status = |id: &str, hosts: &[Option<&str>], state| {
            let response = invoke(
                &backend,
                1000,
                original::SET_DOMAIN_VERIFICATION_STATUS,
                &backend_set_request(id, hosts, state),
            );
            dv::read_set_domain_verification_status_reply(&mut response.reader()).unwrap()
        };
        assert_eq!(status(&id, &[Some("example.org")], 1).unwrap(), 0);
        let current = host.package_capture().unwrap();
        let code = current.state().packages["p"].pkg.clone().unwrap();
        assert_eq!(
            retained
                .domains()
                .unwrap()
                .owner()
                .verification("p", &code, true, &config)
                .unwrap()
                .unwrap()
                .1,
            [("example.org".into(), 0)]
        );
        assert_eq!(
            current
                .domains()
                .unwrap()
                .verification("p", &code)
                .unwrap()
                .unwrap()
                .1,
            [("example.org".into(), 1)]
        );
        let mut info = Parcel::new();
        original::GetDomainVerificationInfo {
            uid: 10100,
            package_name: Some("p".into()),
        }
        .write(&mut info);
        invoke(
            &backend,
            1000,
            original::GET_DOMAIN_VERIFICATION_INFO,
            &info,
        )
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
        let saved = crate::package::settings::Settings::parse(
            &aim_android_xml::read(
                &std::fs::read(host.data().unwrap().join("system/packages.xml")).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            saved.domain_verification,
            current.domains().unwrap().owner().persisted()
        );
        for (hosts, expected) in [(vec![Some("other.org")], 2), (vec![None], 2)] {
            assert_eq!(status(&id, &hosts, 1).unwrap(), expected);
        }
        assert!(status(&id, &[], 1).is_err());
        assert!(status(&id, &[Some("example.org")], 0).is_err());
        assert_eq!(
            status("00000000-0000-0000-0000-000000000099", &[], 1).unwrap(),
            1
        );
        assert!(Arc::ptr_eq(&current, &host.package_capture().unwrap()));
        assert_eq!(status(&id, &[Some("example.org")], 1024).unwrap(), 0);
        let current = host.package_capture().unwrap();
        assert_eq!(
            current
                .domains()
                .unwrap()
                .verification("p", &code)
                .unwrap()
                .unwrap()
                .1,
            [("example.org".into(), 1024)]
        );
        // The mutation uses the same collector policy as the paired Info read.
        assert!(
            collector::collect(
                &code,
                current.domains().unwrap().collector_policy("p").unwrap(),
                collector::Kind::ValidAutoVerify
            )
            .contains(&"example.org".into())
        );
        // URI groups use the same attached native owner, but unlike verifier
        // status, original DVS never schedules persistence from this setter.
        let before = host.package_capture().unwrap();
        let disk_path = host.data().unwrap().join("system/packages.xml");
        let disk_bytes = std::fs::read(&disk_path).unwrap();
        let set_group = |name: &str, groups: Vec<Option<Group>>| {
            let mut request = Parcel::new();
            original::SetUriRelativeFilterGroup {
                package_name: Some(name.into()),
                id: Some(id.clone()),
                domain: Some("example.org".into()),
                groups: Some(groups),
            }
            .write(&mut request);
            invoke(
                &backend,
                1000,
                original::SET_URI_RELATIVE_FILTER_GROUP,
                &request,
            )
            .reader()
            .read_exception()
            .unwrap()
        };
        set_group(
            "p",
            vec![Some(Group {
                action: 1,
                filters: Some(vec![Some(uri_parcel::Filter {
                    uri_part: 0,
                    pattern_type: 1,
                    filter: Some("/path".into()),
                })]),
            })],
        )
        .unwrap();
        let current = host.package_capture().unwrap();
        assert!(
            before
                .domains()
                .unwrap()
                .owner()
                .uri_groups("p", &["example.org".into()])
                .is_empty()
        );
        assert_eq!(
            current
                .domains()
                .unwrap()
                .owner()
                .uri_groups("p", &["example.org".into()])[0]
                .1[0]
                .action,
            1
        );
        let mut get = Parcel::new();
        original::GetUriRelativeFilterGroups {
            uid: 10100,
            package_name: Some("p".into()),
            domains: Some(vec![Some("example.org".into()), Some("missing.org".into())]),
        }
        .write(&mut get);
        let response = invoke(
            &backend,
            1000,
            original::GET_URI_RELATIVE_FILTER_GROUPS,
            &get,
        );
        let groups = original::read_get_uri_relative_filter_groups_reply::<
            crate::package::domain_verification::uri_bundle::Bundle,
        >(&mut response.reader())
        .unwrap()
        .unwrap()
        .unwrap();
        let entries = groups.entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key.as_deref(), Some("example.org"));
        assert_eq!(
            entries[0].groups().unwrap().unwrap()[0]
                .as_ref()
                .unwrap()
                .action,
            1
        );
        // Retired/missing attachments cannot apply a delayed Java update.
        set_group("missing", vec![]).unwrap();
        let mut stale = Parcel::new();
        original::SetUriRelativeFilterGroup {
            package_name: Some("p".into()),
            id: Some("00000000-0000-0000-0000-000000000099".into()),
            domain: Some("example.org".into()),
            groups: Some(Vec::<Option<Group>>::new()),
        }
        .write(&mut stale);
        invoke(
            &backend,
            1000,
            original::SET_URI_RELATIVE_FILTER_GROUP,
            &stale,
        )
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
        assert!(Arc::ptr_eq(&current, &host.package_capture().unwrap()));
        set_group("p", vec![]).unwrap();
        assert!(
            host.package_capture()
                .unwrap()
                .domains()
                .unwrap()
                .owner()
                .uri_groups("p", &["example.org".into()])
                .is_empty()
        );
        assert!(
            !current
                .domains()
                .unwrap()
                .owner()
                .uri_groups("p", &["example.org".into()])
                .is_empty()
        );
        assert_eq!(std::fs::read(disk_path).unwrap(), disk_bytes);
    });
}

#[test]
fn only_system_uid_can_attach_frontend_get_backend_or_query_agent_identity() {
    use aim_service_aidl::dev_aim_server_isystemprocess as sp;
    let host = AppHost::new(crate::test_process(1000));
    let system = super::super::system_process::SystemProcess::new(host.clone());
    let mut requests = Vec::new();
    let mut p = Parcel::new();
    sp::GetDomainVerificationBackend {}.write(&mut p);
    requests.push((sp::GET_DOMAIN_VERIFICATION_BACKEND, p));
    let mut p = Parcel::new();
    sp::AttachDomainVerification { frontend: None }.write(&mut p);
    requests.push((sp::ATTACH_DOMAIN_VERIFICATION, p));
    let mut p = Parcel::new();
    sp::IsDomainVerifier { uid: 1000 }.write(&mut p);
    requests.push((sp::IS_DOMAIN_VERIFIER, p));
    for (code, request) in requests {
        let error = invoke(&system, 10100, code, &request)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code, EX_SECURITY);
    }
    assert!(host.domain_frontend.lock().unwrap().is_none());
}

#[test]
fn original_bridge_retains_large_domain_set_and_info_file_descriptors() {
    use std::os::fd::AsFd;
    struct Files(Arc<aim_binder_host::local::LocalProcess>);
    impl Service for Files {
        fn descriptor(&self) -> &str {
            original::DESCRIPTOR
        }
        fn accepts_fds(&self) -> bool {
            true
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            let mut p = Parcel::new();
            if call.code == original::SET_DOMAIN_VERIFICATION_STATUS {
                let a = original::SetDomainVerificationStatus::<DomainSet>::read(&mut call.data)?;
                assert_eq!(a.uid, 1000);
                let hosts = a.domains.unwrap().resolve(&self.0)?;
                assert_eq!(hosts.len(), 4000);
                assert!(hosts.contains(&Some("h3999.example".into())));
                original::write_set_domain_verification_status_reply(&mut p, 0);
            } else {
                original::GetDomainVerificationInfo::read(&mut call.data)?;
                let states = (0..4000)
                    .map(|i| (format!("h{i}.example"), 1))
                    .collect::<Vec<_>>();
                let info =
                    parcels::Info::prepare(8, "00000000-0000-0000-0000-000000000001", "p", &states)
                        .unwrap();
                original::write_get_domain_verification_info_reply(&mut p, Some(&info));
            }
            Ok(p)
        }
    }
    let process = crate::test_process(1000);
    let host = AppHost::new(process.clone());
    let Binder::Local(ptr) = process.add_service(Arc::new(Files(process.clone()))) else {
        unreachable!()
    };
    *host.domain_frontend.lock().unwrap() = Some(Arc::new(crate::system::ServiceOwner::Local(
        process.local_service(ptr).unwrap(),
    )));
    let Binder::Local(ptr) = process.add_service(Arc::new(DomainVerification::new(host.clone())))
    else {
        unreachable!()
    };
    let public = process.local_service(ptr).unwrap();
    let mut hosts = Parcel::new();
    hosts.write_i32(4000);
    for i in 0..4000 {
        hosts.write_string16(Some(&format!("h{i}.example")));
    }
    let region = aim_ashmem::immutable_blob(hosts.data()).unwrap();
    let mut p = Parcel::new();
    p.write_interface_token(dv::DESCRIPTOR);
    p.write_string16(Some("00000000-0000-0000-0000-000000000001"));
    p.write_i32(1);
    p.write_bool(true);
    p.write_i32(hosts.data().len() as i32);
    p.write_i32(1);
    p.write_file(aim_binder_host::server::file_from_fd(region.as_fd()).unwrap());
    p.write_i32(1);
    let response = public
        .transact(dv::SET_DOMAIN_VERIFICATION_STATUS, &p, false)
        .unwrap();
    assert_eq!(
        dv::read_set_domain_verification_status_reply(&mut response.reader())
            .unwrap()
            .unwrap(),
        0
    );
    // The backend accepts the same FD-bearing request before reporting its
    // unconfigured package owner, rather than rejecting it at Binder transport.
    let Binder::Local(ptr) = process.add_service(Arc::new(Backend::new(host))) else {
        unreachable!()
    };
    let mut private = Parcel::new();
    private.write_interface_token(original::DESCRIPTOR);
    private.write_i32(1000);
    let mut reader = p.reader();
    reader.enforce_interface(dv::DESCRIPTOR).unwrap();
    let start = reader.position();
    reader.skip(reader.remaining()).unwrap();
    let (bytes, objects) = reader.since(start);
    // Rebase the FD's retained owner along with the public request body.
    let files = p
        .files()
        .iter()
        .map(|(at, file)| (*at - start as u64, file.clone()))
        .collect::<Vec<_>>();
    private.write_raw_files(bytes, &objects, &files);
    let response = process
        .local_service(ptr)
        .unwrap()
        .transact(original::SET_DOMAIN_VERIFICATION_STATUS, &private, false)
        .unwrap();
    assert_eq!(
        response
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err()
            .code,
        EX_ILLEGAL_STATE
    );
    let mut p = Parcel::new();
    dv::GetDomainVerificationInfo {
        package_name: Some("p".into()),
    }
    .write(&mut p);
    let response = public
        .transact(dv::GET_DOMAIN_VERIFICATION_INFO, &p, false)
        .unwrap();
    let mut r = response.reader();
    r.read_exception().unwrap().unwrap();
    assert_eq!(r.read_i32().unwrap(), 1);
    r.read_string16().unwrap();
    assert_eq!(r.read_string16().unwrap().as_deref(), Some("p"));
    assert!(r.read_bool().unwrap());
    let len = r.read_i32().unwrap();
    assert_eq!(r.read_i32().unwrap(), 1);
    let fd = r.read_fd().unwrap();
    let file = aim_binder_host::server::file_fd(&process.file(fd).unwrap()).unwrap();
    assert_eq!(file.metadata().unwrap().len(), len as u64);
    let mut bytes = vec![0; len as usize];
    std::os::unix::fs::FileExt::read_exact_at(&file, &mut bytes, 0).unwrap();
    assert_eq!(
        aim_binder_host::parcel::Reader::new(&bytes, &[])
            .read_i32()
            .unwrap(),
        4000
    );
}

fn backend_set_request(id: &str, hosts: &[Option<&str>], state: i32) -> Parcel {
    let mut p = Parcel::new();
    original::SetDomainVerificationStatus {
        uid: 10100,
        id: Some(id.into()),
        domains: Some(
            crate::package::domain_verification::domain_set::Forward::Hosts(
                hosts.iter().map(|host| host.map(str::to_owned)).collect(),
            ),
        ),
        state,
    }
    .write(&mut p);
    p
}
