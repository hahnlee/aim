//! Binder package endpoints for M4 C. They consume an owned scan snapshot,
//! never the original PMS's shadow feed. These endpoints are not yet
//! registered by guest-init: native scanning, mutation side effects and
//! the SystemServer facade must pass the C gates first (#798).

use std::sync::Arc;
#[cfg(test)]
use std::sync::RwLock;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{EX_UNSUPPORTED_OPERATION, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;
use aim_service_aidl::android_content_pm_ipackagemanagernative as native;

use super::apps_filter::NotModelled;
#[cfg(test)]
use super::model::State;
use super::query::Query;
use super::resolve::{QueryError, Resolver};

/// Both interfaces share the owner's published state. Each transaction
/// captures one immutable snapshot; publication does not invalidate
/// Binder references already held by clients.
pub struct PackageQueries {
    source: Source,
    resolver: Arc<Resolver>,
    native: bool,
    system: Option<std::sync::Weak<crate::system::System>>,
}

enum Source {
    Native(
        Arc<
            dyn Fn() -> Result<Arc<super::scan_snapshot::query_state::Capture>, Exception>
                + Send
                + Sync,
        >,
    ),
    #[cfg(test)]
    Fixture(Arc<RwLock<Arc<State>>>),
}
impl PackageQueries {
    pub fn from_system(system: &Arc<crate::system::System>) -> (Arc<Self>, Arc<Self>) {
        let system = Arc::downgrade(system);
        let runtime_system = system.clone();
        let source = Arc::new(move || {
            system
                .upgrade()
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "native system owner is unavailable",
                    )
                })?
                .capture_package_queries()
        });
        let resolver = Arc::new(Resolver::default());
        (
            Arc::new(Self {
                source: Source::Native(source.clone()),
                resolver: resolver.clone(),
                native: false,
                system: Some(runtime_system.clone()),
            }),
            Arc::new(Self {
                source: Source::Native(source),
                resolver,
                native: true,
                system: Some(runtime_system),
            }),
        )
    }
    #[cfg(test)]
    pub fn new(state: Arc<RwLock<Arc<State>>>) -> (Arc<Self>, Arc<Self>) {
        let resolver = Arc::new(Resolver::default());
        let package = Arc::new(Self {
            source: Source::Fixture(state.clone()),
            resolver: resolver.clone(),
            native: false,
            system: None,
        });
        let package_native = Arc::new(Self {
            source: Source::Fixture(state),
            resolver,
            native: true,
            system: None,
        });
        (package, package_native)
    }
}

impl PackageQueries {
    fn mutation(
        &self,
        call: &mut Call<'_>,
        capture: Option<&Arc<super::scan_snapshot::query_state::Capture>>,
        query: &Query<'_>,
    ) -> Option<Result<Parcel,QueryError>> {
        let request=super::write::mutation::Request::read(call.code,call.sender_euid,&mut call.data)?;
        Some((|| {
            let request=request.map_err(QueryError::Transport)?;
            let plan=match request.decide(query,call.sender_pid).map_err(QueryError::NotModelled)? {
                Ok(plan)=>plan,
                Err(error)=> {let mut reply=Parcel::new();reply.write_exception(&error);return Ok(reply);}
            };
            let result=(|| {
                let system=self.system.as_ref().and_then(|system|system.upgrade()).ok_or_else(||
                    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"native mutation owner unavailable"))?;
                let capture=capture.ok_or_else(||Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"native mutation capture unavailable"))?;
                system.commit_package_mutation(capture,&plan).map_err(|error|Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,error.to_string()))
            })();
            let mut reply=Parcel::new();
            match result {Ok(())=>reply.write_no_exception(),Err(error)=>reply.write_exception(&error)}
            Ok(reply)
        })())
    }

    fn runtime_version(
        &self,
        call: &mut Call<'_>,
        capture: Option<&Arc<super::scan_snapshot::query_state::Capture>>,
        query: &Query<'_>,
    ) -> Result<Parcel, QueryError> {
        let (user, version) = if call.code == pm::GET_RUNTIME_PERMISSIONS_VERSION {
            let args = pm::GetRuntimePermissionsVersion::read(&mut call.data)
                .map_err(QueryError::Transport)?;
            (args.user_id, None)
        } else {
            let args = pm::SetRuntimePermissionsVersion::read(&mut call.data)
                .map_err(QueryError::Transport)?;
            (args.user_id, Some(args.version))
        };
        if call.data.remaining() != 0 {
            return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        let result = (|| {
            if user < 0 || version.is_some_and(|value| value < 0) {
                return Ok(Err(Exception::illegal_argument(
                    "runtime version and user must be nonnegative",
                )));
            }
            let uid = query.calling_uid;
            if super::apps_filter::is_isolated(uid) {
                return Ok(Err(Exception::security(
                    "isolated callers cannot change runtime permission policy",
                )));
            }
            if uid % 100_000 != 0
                && uid % 100_000 != 1000
                && !query.uid_has_permission(
                    uid,
                    "android.permission.ADJUST_RUNTIME_PERMISSIONS_POLICY",
                )?
                && !query
                    .uid_has_permission(uid, "android.permission.UPGRADE_RUNTIME_PERMISSIONS")?
            {
                return Ok(Err(Exception::security(
                    "runtime permission version requires policy adjustment or upgrade permission",
                )));
            }
            let Some(system) = self.system.as_ref().and_then(|system| system.upgrade()) else {
                return Ok(Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "runtime metadata system owner is unavailable",
                )));
            };
            let Some(capture) = capture else {
                return Ok(Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native runtime metadata capture is unavailable",
                )));
            };
            Ok(
                system.with_runtime_permission_metadata(capture, |metadata| {
                    if let Some(version) = version {
                        metadata.set_version(user, version);
                    }
                    metadata.version(user)
                }),
            )
        })()
        .map_err(QueryError::NotModelled)?;
        let mut reply = Parcel::new();
        match result {
            Err(error) => reply.write_exception(&error),
            Ok(value) => {
                if version.is_some() {
                    pm::write_set_runtime_permissions_version_reply(&mut reply);
                } else {
                    pm::write_get_runtime_permissions_version_reply(&mut reply, value);
                }
            }
        }
        Ok(reply)
    }
}

impl Service for PackageQueries {
    fn descriptor(&self) -> &str {
        if self.native {
            native::DESCRIPTOR
        } else {
            pm::DESCRIPTOR
        }
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let methods = if self.native {
            native::METHODS
        } else {
            pm::METHODS
        };
        if !methods.iter().any(|(code, _)| *code == call.code) {
            return Err(UNKNOWN_TRANSACTION);
        }
        // Check before dispatch so an unsupported method cannot accept
        // another interface's token. Generated readers check it again.
        let position = call.data.position();
        call.data.enforce_interface(self.descriptor())?;
        call.data.set_position(position);
        let uid = call.sender_euid as i32;
        let capture = match &self.source {
            Source::Native(source) => match source() {
                Ok(capture) => Some(capture),
                Err(error) => {
                    let mut reply = Parcel::new();
                    reply.write_exception(&error);
                    return Ok(reply);
                }
            },
            #[cfg(test)]
            Source::Fixture(_) => None,
        };
        let state = match (&self.source, &capture) {
            (Source::Native(_), Some(capture)) => capture.state().clone(),
            #[cfg(test)]
            (Source::Fixture(state), _) => state.read().unwrap().clone(),
            _ => unreachable!(),
        };
        let resolved = (!self.native)
            .then(|| self.resolver.query(&state, call.code, uid, &mut call.data))
            .flatten();
        let answer = resolved.unwrap_or_else(|| {
            let resolution = match self.resolver.resolution(&state) {
                Ok(resolution) => resolution,
                Err(error) => return error.reply().map_err(QueryError::Transport),
            };
            let query = Query {
                state: &state,
                filter: &resolution.apps_filter,
                calling_uid: uid,
            };
            if !self.native
                && matches!(
                    call.code,
                    pm::GET_RUNTIME_PERMISSIONS_VERSION | pm::SET_RUNTIME_PERMISSIONS_VERSION
                )
            {
                return self.runtime_version(call, capture.as_ref(), &query);
            }
            if !self.native && matches!(call.code,pm::SET_SPLASH_SCREEN_THEME|pm::SET_USER_MIN_ASPECT_RATIO|pm::SET_UPDATE_AVAILABLE) {
                if let Some(answer)=self.mutation(call,capture.as_ref(),&query) {return answer;}
            }
            query
                .answer(self.descriptor(), call.code, &mut call.data)
                .map_err(QueryError::NotModelled)
        });
        Ok(match answer {
            Ok(reply) => reply,
            Err(QueryError::Transport(status)) => return Err(status),
            Err(QueryError::NotModelled(NotModelled(reason))) => {
                let mut reply = Parcel::new();
                reply.write_exception(&Exception::new(EX_UNSUPPORTED_OPERATION, reason));
                reply
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{PackageState, PackageUserState, User};
    use super::*;
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, Reader};

    fn endpoints() -> (Arc<PackageQueries>, Arc<PackageQueries>) {
        let state = State {
            packages: [(
                "example.app".into(),
                PackageState {
                    name: "example.app".into(),
                    app_id: 10100,
                    users: [(
                        0,
                        PackageUserState {
                            enabled: 2,
                            ..Default::default()
                        },
                    )]
                    .into(),
                    ..Default::default()
                },
            )]
            .into(),
            users: [(
                0,
                User {
                    id: 0,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        PackageQueries::new(Arc::new(RwLock::new(Arc::new(state))))
    }

    fn call(service: &PackageQueries, code: u32, uid: u32, data: &Parcel) -> Reply {
        service.transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 77,
            sender_euid: uid,
            data: Reader::new(data.data(), &[]),
        })
    }

    #[test]
    fn runtime_version_checks_arguments_before_caller_permissions() {
        let state = State {
            packages: [(
                "caller".into(),
                PackageState {
                    name: "caller".into(),
                    app_id: 10100,
                    users: [(0, PackageUserState::default())].into(),
                    ..Default::default()
                },
            )]
            .into(),
            users: [(
                0,
                User {
                    id: 0,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        let (endpoint, _) = PackageQueries::new(Arc::new(RwLock::new(Arc::new(state))));
        for user in [-1, 0] {
            let mut data = Parcel::new();
            pm::GetRuntimePermissionsVersion { user_id: user }.write(&mut data);
            let reply = call(&endpoint, pm::GET_RUNTIME_PERMISSIONS_VERSION, 10100, &data).unwrap();
            let error =
                pm::read_get_runtime_permissions_version_reply(&mut Reader::new(reply.data(), &[]))
                    .unwrap()
                    .unwrap_err();
            assert_eq!(
                error.code,
                if user < 0 {
                    EX_ILLEGAL_ARGUMENT
                } else {
                    aim_binder_host::parcel::EX_SECURITY
                }
            );
        }
        let mut isolated = Parcel::new();
        pm::GetRuntimePermissionsVersion { user_id: 0 }.write(&mut isolated);
        let reply = call(
            &endpoint,
            pm::GET_RUNTIME_PERMISSIONS_VERSION,
            99000,
            &isolated,
        )
        .unwrap();
        assert_eq!(
            pm::read_get_runtime_permissions_version_reply(&mut Reader::new(reply.data(), &[]))
                .unwrap()
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_SECURITY
        );
        let mut data = Parcel::new();
        pm::SetRuntimePermissionsVersion {
            version: -1,
            user_id: 0,
        }
        .write(&mut data);
        let reply = call(&endpoint, pm::SET_RUNTIME_PERMISSIONS_VERSION, 10100, &data).unwrap();
        assert_eq!(
            pm::read_set_runtime_permissions_version_reply(&mut Reader::new(reply.data(), &[]))
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
    }

    #[test]
    fn binder_identity_controls_package_visibility() {
        let (package, _) = endpoints();
        let mut args = Parcel::new();
        args.write_interface_token(pm::DESCRIPTOR);
        args.write_string16(Some("example.app"));
        args.write_i32(0);
        let reply = call(&package, pm::GET_APPLICATION_ENABLED_SETTING, 10100, &args).unwrap();
        let mut read = Reader::new(reply.data(), &[]);
        assert!(read.read_exception().unwrap().is_ok());
        assert_eq!(read.read_i32().unwrap(), 2);
        let reply = call(&package, pm::GET_APPLICATION_ENABLED_SETTING, 10101, &args).unwrap();
        assert_eq!(
            Reader::new(reply.data(), &[])
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
    }

    #[test]
    fn endpoints_enforce_their_own_generated_interface() {
        let (package, package_native) = endpoints();
        assert_eq!(package.descriptor(), pm::DESCRIPTOR);
        assert_eq!(package_native.descriptor(), native::DESCRIPTOR);
        let mut args = Parcel::new();
        args.write_interface_token(pm::DESCRIPTOR);
        assert!(call(&package_native, native::GET_NAMES_FOR_UIDS, 0, &args).is_err());
        assert_eq!(
            call(&package, u32::MAX, 0, &args).unwrap_err(),
            UNKNOWN_TRANSACTION
        );
        let reply = call(&package, pm::SET_APPLICATION_ENABLED_SETTING, 10100, &args).unwrap();
        assert_eq!(
            Reader::new(reply.data(), &[])
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_UNSUPPORTED_OPERATION
        );
    }

    #[test]
    fn existing_binder_endpoints_follow_owner_publication() {
        let (package, package_native) = endpoints();
        let mut names = Parcel::new();
        names.write_interface_token(native::DESCRIPTOR);
        names.write_i32(1);
        names.write_i32(10100);
        let name = || {
            let reply = call(&package_native, native::GET_NAMES_FOR_UIDS, 0, &names).unwrap();
            let mut read = Reader::new(reply.data(), &[]);
            assert!(read.read_exception().unwrap().is_ok());
            assert_eq!(read.read_i32().unwrap(), 1);
            read.read_string16().unwrap()
        };
        assert_eq!(name().as_deref(), Some("example.app"));
        let Source::Fixture(state) = &package.source else {
            unreachable!()
        };
        let mut next = (**state.read().unwrap()).clone();
        next.packages.clear();
        *state.write().unwrap() = Arc::new(next);
        assert_eq!(name().as_deref(), Some(""));
        let mut enabled = Parcel::new();
        enabled.write_interface_token(pm::DESCRIPTOR);
        enabled.write_string16(Some("example.app"));
        enabled.write_i32(0);
        let reply = call(&package, pm::GET_APPLICATION_ENABLED_SETTING, 0, &enabled).unwrap();
        assert_eq!(
            Reader::new(reply.data(), &[])
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
    }
}
