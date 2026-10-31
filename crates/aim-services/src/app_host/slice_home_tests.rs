//! SliceManagerService.getDefaultHome / ComputerEngine.getHomeActivitiesAsUser,
//! android-16.0.0_r1: a non-home caller still passes through the home lookup.
use super::*;
use crate::package::{
    intent::ComponentName,
    intent_filter::{IntentFilter, MATCH_CATEGORY_EMPTY, ParsedIntentInfo},
    pkg::{Activity, Provider, booleans},
    preferred::{Preferred, PreferredActivity, Selection, registry::*},
    query::preferred::Owner,
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Identities;
impl IdentityProvider for Identities {
    fn allocate(
        &self,
        _: crate::package::preferred::records::Record<'_>,
    ) -> std::result::Result<Identity, String> {
        Ok(Identity {
            hash: 1,
            lease: Arc::new(()),
        })
    }
}

struct Home {
    host: Weak<AppHost>,
    registry: Arc<Registry>,
    role: Option<String>,
    role_error: Option<Exception>,
    commit_error: Option<Exception>,
    commits: AtomicUsize,
}

macro_rules! unexpected {
    ($name:ident($($arg:ident: $ty:ty),* $(,)?) -> $out:ty) => {
        fn $name(&self, $(_: $ty),*) -> std::result::Result<$out, ActionError> {
            panic!(concat!("unexpected ", stringify!($name)))
        }
    };
}
impl Actions for Home {
    fn default_home(&self, user: i32) -> std::result::Result<Option<String>, ActionError> {
        assert_eq!(user, 0);
        match &self.role_error {
            Some(error) => Err(ActionError::Exception(error.clone())),
            None => Ok(self.role.clone()),
        }
    }
    fn commit_home_selection(
        &self,
        user: i32,
        generation: u64,
        selection: &Selection,
    ) -> std::result::Result<(), ActionError> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = &self.commit_error {
            return Err(ActionError::Exception(error.clone()));
        }
        let stage = self
            .registry
            .prepare_selection(user, generation, selection)
            .unwrap()
            .unwrap();
        self.registry.publish(stage).unwrap();
        let host = self.host.upgrade().unwrap();
        let system = host.package_system().unwrap();
        let capture = host.package_capture().unwrap();
        let mut context = (**capture.context()).clone();
        let actions = context
            .system
            .preferred_owner
            .as_ref()
            .unwrap()
            .actions
            .clone();
        context.system.preferred_owner = Some(Arc::new(Handle::new(
            Arc::new(self.registry.capture().unwrap()),
            Arc::new(|_, _, _| panic!("home uses memory selection")),
            actions,
        )));
        context.scan_version = capture.scan().version() + 1;
        system
            .publish_package_scan_with_queries(
                &system.package_bootstrap().unwrap(),
                Some(capture.scan()),
                capture.scan().owner().clone(),
                capture.scan().usage().clone(),
                context,
            )
            .unwrap();
        Ok(())
    }
    unexpected!(context_permission(uid: i32, name: &str) -> bool);
    unexpected!(
        commit_after_selection(
            user: i32,
            mutation: &crate::package::preferred::Mutation,
            uid: i32,
        ) -> bool
    );
    unexpected!(
        commit_mutation(
            user: i32,
            generation: u64,
            mutation: &crate::package::preferred::Mutation,
            uid: i32,
        ) -> bool
    );
    unexpected!(cross_access(uid: i32, source: i32, target: i32, adding: bool) -> i32);
    unexpected!(cross_accessible(uid: i32, source: i32, target: i32) -> bool);
    unexpected!(enforce_shell_restriction(uid: i32, user: i32) -> ());
    unexpected!(reconcile_home(user: i32, uid: i32) -> bool);
    unexpected!(default_browser(user: i32) -> Option<String>);
    unexpected!(restore_browser(user: i32, package: &str, installed: bool) -> ());
    unexpected!(default_preferences(user: i32) -> Vec<PreferredActivity>);
}

fn home_filter() -> IntentFilter {
    let mut filter = IntentFilter::default();
    filter.add_action("android.intent.action.MAIN");
    filter.add_category("android.intent.category.HOME");
    filter.add_category("android.intent.category.DEFAULT");
    filter
}

fn install_home_owner(
    host: &Arc<AppHost>,
    publish: &mut dyn FnMut(&mut dyn FnMut(&mut crate::package::pkg::AndroidPackage)),
    homes: usize,
    system_home: bool,
    stale: bool,
    role: Option<&str>,
    role_error: Option<Exception>,
    commit_error: Option<Exception>,
) -> Arc<Home> {
    publish(&mut |package| {
        if package.package_name != "p" {
            return;
        }
        if system_home {
            package.booleans |= booleans::SYSTEM;
        }
        package.activities.clear();
        for index in 0..homes {
            let mut activity = Activity::default();
            activity.max_aspect_ratio = Some(0.0);
            activity.min_aspect_ratio = Some(0.0);
            activity.main.component.package_name = "p".into();
            activity.main.component.name = format!("p.Home{index}");
            activity.main.component.intents = vec![ParsedIntentInfo {
                filter: home_filter(),
                has_default: true,
                ..Default::default()
            }];
            activity.main.enabled = true;
            activity.main.exported = true;
            activity.main.direct_boot_aware = true;
            package.activities.push(activity);
        }
        let mut provider = Provider::default();
        provider.main.component.package_name = "p".into();
        provider.main.component.name = "p.Slices".into();
        provider.main.enabled = true;
        provider.main.exported = true;
        provider.main.direct_boot_aware = true;
        provider.authority = Some("p.slices".into());
        package.providers = vec![provider];
    });
    let mut preferred = Preferred::default();
    if stale {
        preferred.add_preferred(
            PreferredActivity::new(
                home_filter(),
                MATCH_CATEGORY_EMPTY,
                None,
                ComponentName {
                    package: "p".into(),
                    class: "p.Vanished".into(),
                },
                true,
            ),
            false,
        );
    }
    let registry = Arc::new(Registry::new(Arc::new(Identities)));
    registry.insert_user(0, preferred).unwrap();
    let actions = Arc::new(Home {
        host: Arc::downgrade(host),
        registry: registry.clone(),
        role: role.map(str::to_owned),
        role_error,
        commit_error,
        commits: AtomicUsize::new(0),
    });
    let owner = Arc::new(Handle::new(
        Arc::new(registry.capture().unwrap()),
        Arc::new(|_, _, _| panic!("home uses memory selection")),
        actions.clone(),
    ));
    let system = host.package_system().unwrap();
    let capture = host.package_capture().unwrap();
    let mut context = (**capture.context()).clone();
    context.system.preferred_owner = Some(owner);
    context.platform.device_provisioned = true;
    context.scan_version = capture.scan().version() + 1;
    system
        .publish_package_scan_with_queries(
            &system.package_bootstrap().unwrap(),
            Some(capture.scan()),
            capture.scan().owner().clone(),
            capture.scan().usage().clone(),
            context,
        )
        .unwrap();
    actions
}

fn pin(service: &SliceManager) -> std::result::Result<(), Exception> {
    let mut request = Parcel::new();
    sm::PinSlice::<SliceUri, Spec> {
        pkg: Some("q".into()),
        uri: Some(SliceUri(Uri::parse("content://p.slices/wifi"))),
        specs: Some(vec![]),
        token: Some(Binder::Local(77)),
    }
    .write(&mut request);
    let reply = service
        .transact(&mut Call {
            code: sm::PIN_SLICE,
            flags: 0,
            sender_pid: 77,
            sender_euid: 10101,
            data: request.reader(),
        })
        .unwrap();
    let result = sm::read_pin_slice_reply(&mut reply.reader()).unwrap();
    result
}

fn service(host: &Arc<AppHost>) -> (Arc<SliceManager>, mpsc::Receiver<Work>) {
    let service = SliceManager::new(host.clone());
    // Observe the real pin path's handler work without launching providers.
    let (tx, rx) = mpsc::channel();
    *service.work.lock().unwrap() = Some(tx);
    service
        .with(|s| {
            s.store
                .grant("q", 0, Some("p"), 0, &Uri::parse("content://p.slices/wifi"))
        })
        .unwrap()
        .unwrap();
    (service, rx)
}

#[test]
fn non_home_pin_with_zero_one_two_and_stale_homes() {
    for system_home in [false, true] {
        for (homes, stale) in [(0, false), (1, false), (2, false), (2, true)] {
            crate::system::with_discovery_session(|host, publish, _| {
                let actions =
                    install_home_owner(host, publish, homes, system_home, stale, None, None, None);
                let (service, rx) = service(host);
                pin(&service).unwrap();
                assert_eq!(
                    service.default_home(0).unwrap().as_deref(),
                    (system_home && homes > 0).then_some("p")
                );
                let (state, resolution) = host.packages().unwrap();
                let query = crate::package::query::Query {
                    state: &state,
                    filter: &resolution.apps_filter,
                    calling_uid: SYSTEM_UID,
                };
                let plan = crate::package::query::preferred::home_plan(&query, 0).unwrap();
                assert_eq!(plan.candidates.len(), homes);
                assert!(plan.chosen.is_none());
                assert_eq!(plan.default_home().is_some(), homes == 1);
                if stale {
                    assert_eq!(actions.commits.load(Ordering::SeqCst), 1);
                    assert!(
                        actions
                            .registry
                            .capture()
                            .unwrap()
                            .snapshot(0)
                            .unwrap()
                            .unwrap()
                            .state
                            .preferred
                            .entries()
                            .is_empty()
                    );
                }
                assert!(!service.has_full_access("q", 0).unwrap());
                assert_eq!(
                    service.state.lock().unwrap().as_ref().unwrap().pinned.len(),
                    1
                );
                assert!(rx.try_iter().any(|work| matches!(work, Work::Usage(pkg, usage_events::SLICE_PINNED) if pkg == "p")));
                let denied = SliceManager::new(host.clone());
                let (tx, _rx) = mpsc::channel();
                *denied.work.lock().unwrap() = Some(tx);
                *denied.state.lock().unwrap() = Some(State {
                    pinned: Vec::new(),
                    store: Store::new(host.data().unwrap().join("ungranted-slices")),
                });
                assert_eq!(
                    pin(&denied).unwrap_err().code,
                    aim_binder_host::parcel::EX_SECURITY
                );
            });
        }
    }
}

#[test]
fn non_home_pin_keeps_role_and_selection_exceptions() {
    for commit in [false, true] {
        crate::system::with_discovery_session(|host, publish, _| {
            let cause = Exception::new(
                EX_ILLEGAL_STATE,
                if commit {
                    "preferred selection publication changed"
                } else {
                    "role owner unavailable"
                },
            );
            install_home_owner(
                host,
                publish,
                2,
                true,
                commit,
                None,
                (!commit).then(|| cause.clone()),
                commit.then(|| cause.clone()),
            );
            let (service, _) = service(host);
            assert_eq!(pin(&service).unwrap_err(), cause);
            assert!(
                service
                    .state
                    .lock()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .pinned
                    .is_empty()
            );
        });
    }
}

#[test]
fn queued_app_pin_waits_for_package_settings_readiness_or_its_failure() {
    use super::super::{content_service::ContentService, package_boot_context::BootContext};
    use crate::package::resolve::settings::Owner as SettingsOwner;
    use crate::system::ServiceOwner;
    use std::time::Duration;

    for fail in [false, true] {
        crate::system::with_boot_discovery_binders(|host, publish, _, _| {
            install_home_owner(host, publish, 2, true, false, None, None, None);
            publish(&mut |package| {
                if package.package_name == "p" {
                    let mut settings = package.providers[0].clone();
                    settings.main.component.name = "p.Settings".into();
                    settings.authority = Some("settings".into());
                    package.providers.push(settings);
                }
            });
            host.content_service
                .get_or_init(|| Arc::new(ContentService::new(host.clone())));
            let Binder::Local(ptr) = host.process.add_service(BootContext::new(host.clone()))
            else {
                unreachable!()
            };
            let settings = SettingsOwner::new(
                Arc::new(ServiceOwner::Local(
                    host.process.local_service(ptr).unwrap(),
                )),
                false,
            );
            let system = host.package_system().unwrap();
            let base = host.package_capture().unwrap();
            let mut context = (**base.context()).clone();
            context.platform.settings_owner = Some(settings.clone());
            context.scan_version += 1;
            system
                .publish_package_scan_with_queries(
                    &system.package_bootstrap().unwrap(),
                    Some(base.scan()),
                    base.scan().owner().clone(),
                    base.scan().usage().clone(),
                    context,
                )
                .unwrap();
            // Current default grants must not bypass the package settings phase.
            let path = host
                .data()
                .unwrap()
                .join("misc_de/0/apexdata/com.android.permission/access.abx");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let fingerprint =
                super::super::package_bootstrap::build_fingerprint(|n| host.property(n));
            let access = crate::package::permissions::with_default_grant_fingerprint(
                aim_android_xml::read(b"<access/>").unwrap(),
                Some(&fingerprint),
            )
            .unwrap();
            std::fs::write(path, aim_android_xml::abx::write(&access).unwrap()).unwrap();
            assert!(
                host.permission_manager
                    .set(Arc::new(
                        super::super::permission_manager::PermissionManager::new(host.clone())
                    ))
                    .is_ok()
            );
            let service = SliceManager::new(host.clone());
            let before = pin(&service).unwrap_err();
            assert_eq!(
                before,
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "SettingsProvider phase has not reached PMS.systemReady"
                )
            );
            let phase_error = Exception::new(EX_ILLEGAL_STATE, "package prepare-ready failed");
            let (entered, queued) = mpsc::channel();
            let (sent, received) = mpsc::channel();
            let app = host.clone();
            let worker = std::thread::spawn(move || {
                entered.send(()).unwrap();
                let start = app.start_package_process("q", "q", None, false, false);
                let result = match app.processes_ready.wait().clone() {
                    Ok(()) => {
                        // No zygote in this fixture: startup has reached launch
                        // validation, so observe the app's real Binder pin next.
                        assert!(start.is_err());
                        pin(&service)
                    }
                    Err(error) => {
                        assert_eq!(start.unwrap_err(), error);
                        Err(error)
                    }
                };
                sent.send(result).unwrap();
            });
            queued.recv().unwrap();
            let mut pending = None;
            let ready = host.complete_system_ready(|| {
                pending = Some(received.recv_timeout(Duration::from_millis(50)));
                if fail {
                    return Err(phase_error.clone());
                }
                settings.begin_system_ready()?;
                settings.system_ready().map(|_| ())
            });
            let result = received.recv_timeout(Duration::from_secs(5));
            worker.join().unwrap();
            assert_eq!(pending.unwrap(), Err(mpsc::RecvTimeoutError::Timeout));
            if fail {
                assert_eq!(ready, Err(phase_error.clone()));
                assert_eq!(result.unwrap(), Err(phase_error));
            } else {
                ready.clone().unwrap();
                let denied = result.unwrap().unwrap_err();
                assert_eq!(denied.code, aim_binder_host::parcel::EX_SECURITY);
                assert!(denied.message.starts_with("Access to slice "));
            }
            // The package lifecycle is one-shot, including failed readiness.
            assert_eq!(
                host.complete_system_ready(|| panic!("repeated phase")),
                ready
            );
        });
    }
}
