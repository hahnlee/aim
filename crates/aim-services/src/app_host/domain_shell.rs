//! Pinned domain shell: original Java parser/debugger, native captured state.
use super::{AppHost, domain_verification::DomainVerification};
use crate::package::{
    domain_verification::{
        collector::{self, Kind},
        names,
        owner::Owner,
    },
    info::array_order,
    scan_snapshot::query_state::Capture,
};
use aim_binder_host::{
    local::{Call, Reply},
    parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel},
};
use aim_service_aidl::{WriteParcelable, dev_aim_server_iapphostdomainverification as api};
use std::{collections::BTreeMap, sync::Arc};

struct Descriptor(aim_binder_driver::File);
impl WriteParcelable for Descriptor {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0);
        p.write_file(self.0.clone());
    }
}
fn failure(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}
fn strings(p: &mut Parcel, values: &[String]) {
    aim_service_aidl::write_string_list(
        p,
        Some(&values.iter().cloned().map(Some).collect::<Vec<_>>()),
    );
}
impl DomainVerification {
    pub(super) fn shell(&self, call: &mut Call<'_>) -> Reply {
        let mut command = crate::shell::ShellCommand::read(&self.host.process, &mut call.data)?;
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        if !command.has_output() {
            return Ok(Parcel::new());
        }
        let mut files = Vec::new();
        for index in 0..3 {
            let fd = match index {
                0 => command.input(),
                1 => command.output(),
                _ => command.error(),
            };
            files.push(
                fd.ok_or(BAD_VALUE)?
                    .try_clone()
                    .map_err(|_| BAD_VALUE)?
                    .into_file_owner(),
            );
        }
        let args = Some(command.args.iter().cloned().map(Some).collect());
        let status = run(&self.host, call, false, args, files.try_into().unwrap());
        let mut reply = Parcel::new();
        match status {
            Ok(status) => {
                command.finish(status);
                reply.write_no_exception();
            }
            Err(error) => reply.write_exception(&error),
        }
        Ok(reply)
    }
}
pub(super) fn run(
    host: &Arc<AppHost>,
    call: &Call<'_>,
    package_shell: bool,
    arguments: Option<Vec<Option<String>>>,
    files: [aim_binder_driver::File; 3],
) -> Result<i32, Exception> {
    if !matches!(call.sender_euid, 0 | 2000)
        || host.process.authenticated_inbound_identity()
            != Some((call.sender_pid, call.sender_euid))
    {
        return Err(Exception::security(
            "Shell commands are only callable by ADB",
        ));
    }
    let owner = host
        .domain_frontend
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| failure("original domain shell unavailable (#2187)"))?;
    let [input, output, error] = files.map(Descriptor);
    let mut request = Parcel::new();
    api::RunShellCommand {
        package_shell,
        arguments,
        input: Some(input),
        output: Some(output),
        error: Some(error),
    }
    .write(&mut request);
    let answer = match owner.as_ref() {
        crate::system::ServiceOwner::Remote(owner) => {
            host.process
                .transact_preserving_inbound(owner.handle, api::RUN_SHELL_COMMAND, &request)
        }
        crate::system::ServiceOwner::Local(owner) => {
            owner.transact(api::RUN_SHELL_COMMAND, &request, false)
        }
    }
    .map_err(|status| failure(format!("domain shell transport: {status}")))?;
    let mut reader = answer.reader();
    let result = api::read_run_shell_command_reply(&mut reader)
        .map_err(|status| failure(format!("domain shell reply: {status}")))??;
    if reader.remaining() != 0 {
        return Err(failure("domain shell reply tail"));
    }
    Ok(result)
}

/// Private Parcel record consumed only by AppHostDomainShell. All fields in one
/// reply come from one immutable capture; no paths, FDs or mutable Java replica.
pub(super) fn read(host: &Arc<AppHost>, a: api::ReadShellState) -> Result<Vec<u8>, Exception> {
    let capture = host.package_capture()?;
    let native = DomainVerification::domains(&capture)?;
    let state = capture.state();
    let requested = a.package_name.as_deref();
    let names = match requested {
        Some(name) => vec![name.to_owned()],
        None => array_order(
            native.owner().attached_names().map(str::to_owned).collect(),
            |s| s,
        ),
    };
    let missing = || Exception {
        code: aim_binder_host::parcel::EX_SERVICE_SPECIFIC,
        message: String::new(),
        service_specific: 1,
        parcelable: None,
    };
    let mut p = Parcel::new();
    if a.kind == 0 {
        let names: Vec<_> = names
            .into_iter()
            .filter(|name| {
                requested.is_some() || state.packages.get(name).is_some_and(|s| s.pkg.is_some())
            })
            .collect();
        p.write_i32(names.len() as i32);
        for name in names {
            let package = native.owner().package(&name).ok_or_else(missing)?;
            let code = state
                .packages
                .get(&name)
                .and_then(|s| s.pkg.as_deref())
                .ok_or_else(missing)?;
            p.write_string16(Some(&name));
            p.write_string16(Some(&package.id));
            aim_service_aidl::write_byte_array(
                &mut p,
                Some(&code.to_cache_entry().map_err(failure)?.bytes),
            );
            let policy = native.collector_policy(&name).map_err(failure)?;
            for kind in [Kind::ValidAutoVerify, Kind::InvalidAutoVerify, Kind::Web] {
                strings(&mut p, &collector::collect(code, policy, kind));
            }
            p.write_i32(package.domains.len() as i32);
            for (domain, value) in &package.domains {
                p.write_string16(domain.as_deref());
                p.write_i32(*value);
            }
            p.write_i32(package.users.len() as i32);
            for user in &package.users {
                p.write_i32(user.id);
                p.write_bool(user.allow_link_handling);
                strings(&mut p, &user.enabled_hosts);
            }
        }
    } else if matches!(a.kind, 1 | 2) {
        let bridge = host.package_system()?.package_bootstrap()?;
        let users: Vec<_> = if !a.has_user || a.user == -1 {
            state.users.keys().copied().collect()
        } else {
            vec![a.user]
        };
        let rows = if a.kind == 1 {
            names.into_iter().map(Some).collect()
        } else {
            vec![None]
        };
        p.write_i32(rows.len() as i32);
        for name in rows {
            let domains = match &name {
                Some(name) => {
                    let code = state
                        .packages
                        .get(name)
                        .and_then(|s| s.pkg.as_deref())
                        .ok_or_else(missing)?;
                    collector::collect(
                        code,
                        native.collector_policy(name).map_err(failure)?,
                        Kind::Web,
                    )
                }
                None => a
                    .domains
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|d| d.ok_or_else(|| failure("null shell domain")))
                    .collect::<Result<Vec<_>, _>>()?,
            };
            p.write_string16(name.as_deref());
            p.write_i32(domains.len() as i32);
            for domain in domains {
                p.write_string16(Some(&domain));
                p.write_i32(users.len() as i32);
                for user in &users {
                    p.write_i32(*user);
                    let mut levels: BTreeMap<i32, Vec<(String, i64)>> = BTreeMap::new();
                    for name in array_order(
                        native.owner().attached_names().map(str::to_owned).collect(),
                        |s| s,
                    ) {
                        let Some(ps) = state.packages.get(&name) else {
                            continue;
                        };
                        let Some(code) = ps.pkg.as_deref() else {
                            continue;
                        };
                        let policy = native.collector_policy(&name).map_err(failure)?;
                        let selected = capture
                            .scan()
                            .owner()
                            .scanned_user_states(&name)
                            .and_then(|s| s.get(user));
                        let mut level =
                            if collector::collect(code, policy, Kind::Web).contains(&domain) {
                                native
                                    .owner()
                                    .approval(
                                        &name,
                                        code,
                                        selected,
                                        *user,
                                        bridge
                                            .domain_verification_settings_v2(
                                                &name,
                                                code.target_sdk_version,
                                            )
                                            .map_err(|e| failure(format!("{e:?}")))?,
                                        policy,
                                        &domain,
                                    )
                                    .map_err(failure)?
                            } else {
                                -2
                            };
                        if level == 0 {
                            let installed = selected.is_some_and(|s| s.installed);
                            let enabled = selected.is_some_and(|s| match s.enabled {
                                1 => true,
                                2..=4 => false,
                                _ => code.is(crate::package::pkg::booleans::ENABLED),
                            });
                            level = if !installed {
                                -4
                            } else if !enabled {
                                -3
                            } else if collector::collect(code, policy, Kind::ValidAutoVerify)
                                .contains(&domain)
                            {
                                -1
                            } else {
                                0
                            };
                        }
                        levels
                            .entry(level)
                            .or_default()
                            .push((name, selected.map_or(0, |s| s.first_install_time)));
                    }
                    p.write_i32(levels.len() as i32);
                    for (level, owners) in levels {
                        let order = crate::package::timsort::sort(owners.len(), |i, j| {
                            Ok(if owners[i].1 == owners[j].1 {
                                names::compare(&owners[i].0, &owners[j].0)
                            } else {
                                (owners[i].1.wrapping_sub(owners[j].1) as i32).cmp(&0)
                            })
                        })
                        .map_err(failure)?;
                        p.write_i32(level);
                        strings(
                            &mut p,
                            &order
                                .into_iter()
                                .map(|i| owners[i].0.clone())
                                .collect::<Vec<_>>(),
                        );
                    }
                }
            }
        }
    } else {
        return Err(Exception::illegal_argument("unknown domain shell read"));
    }
    Ok(p.data().to_vec())
}

/// Internal shell setters: Java retains assertInternal and user translation;
/// this system-UID backend publishes through the existing durable domain owner.
pub(super) fn write_user(
    host: &Arc<AppHost>,
    name: Option<&str>,
    user: i32,
    enabled: bool,
    hosts: Option<Vec<Option<String>>>,
    selection: bool,
) -> Result<(), Exception> {
    use crate::package::domain_verification::owner::{ApprovalInput, approval_eligible};
    write(host, |capture, owner, bridge| {
        let native = DomainVerification::domains(capture)?;
        let missing = || Exception {
            code: aim_binder_host::parcel::EX_SERVICE_SPECIFIC,
            message: String::new(),
            service_specific: 1,
            parcelable: None,
        };
        if (selection && name.is_none())
            || name.is_some_and(|n| native.owner().package(n).is_none())
        {
            return Err(missing());
        }
        let users = if user == -1 {
            capture.state().users.keys().copied().collect::<Vec<_>>()
        } else {
            vec![user]
        };
        if selection {
            let name = name.unwrap();
            let state = capture.state();
            let code = state
                .packages
                .get(name)
                .and_then(|p| p.pkg.as_deref())
                .ok_or_else(missing)?;
            let declared = collector::collect(
                code,
                native.collector_policy(name).map_err(failure)?,
                Kind::Web,
            );
            let requested = hosts.as_ref().map_or_else(
                || declared.iter().cloned().collect(),
                |hosts| {
                    hosts
                        .iter()
                        .flatten()
                        .filter(|h| declared.contains(h))
                        .cloned()
                        .collect()
                },
            );
            for user in users {
                owner
                    .select_hosts(name, user, &requested, enabled, true, |name| {
                        let Some(code) = state.packages.get(name).and_then(|p| p.pkg.as_deref())
                        else {
                            return Ok(None);
                        };
                        let selected = capture
                            .scan()
                            .owner()
                            .scanned_user_states(name)
                            .and_then(|s| s.get(&user));
                        if !approval_eligible(code, selected) {
                            return Ok(None);
                        }
                        Ok(Some(ApprovalInput {
                            code,
                            user: selected,
                            settings_v2: bridge
                                .domain_verification_settings_v2(name, code.target_sdk_version)
                                .map_err(|e| format!("{e:?}"))?,
                            policy: native.collector_policy(name)?,
                        }))
                    })
                    .map_err(failure)?;
            }
        } else {
            owner
                .set_link_handling_internal(name, enabled, user, &users)
                .map_err(failure)?;
        }
        Ok(())
    })
}

pub(super) fn write(
    host: &Arc<AppHost>,
    mut mutate: impl FnMut(
        &Arc<Capture>,
        &mut Owner,
        &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<(), Exception>,
) -> Result<(), Exception> {
    let system = host.package_system()?;
    let bridge = system.package_bootstrap()?;
    loop {
        let capture = system.capture_package_queries_for(&bridge)?;
        let mut owner = DomainVerification::domains(&capture)?.owner().clone();
        if let Err(error) = mutate(&capture, &mut owner, &bridge) {
            if owner.persisted() != DomainVerification::domains(&capture)?.owner().persisted() {
                // DVS keeps earlier in-memory changes if a later list entry throws,
                // without reaching scheduleWriteSettings.
                let update = capture
                    .prepare_runtime_domain_update(owner)
                    .map_err(failure)?;
                if system
                    .publish_runtime_package_domains(&bridge, update)?
                    .is_none()
                {
                    continue;
                }
            }
            return Err(error);
        }
        system.check_package_bootstrap(&bridge)?;
        let disk = match system.package_persistence_owner(&capture) {
            Ok((_, disk)) => disk,
            Err(error) => {
                if !Arc::ptr_eq(&capture, &system.capture_package_queries_for(&bridge)?) {
                    continue;
                }
                return Err(error);
            }
        };
        let update = capture.prepare_domain_update(owner).map_err(failure)?;
        match system.commit_package_domains_if_current(&bridge, update, &mut disk.lock().unwrap()) {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => continue,
            Err(e) => {
                return Err(failure(format!(
                    "domain write failed (committed={}): {}",
                    e.committed, e.message
                )));
            }
        }
    }
}

pub(super) fn write_status(
    host: &Arc<AppHost>,
    args: api::SetShellStatus,
) -> Result<(), Exception> {
    if !(0..=3).contains(&args.state) {
        return Err(Exception::illegal_argument(
            "State must be one of NO_RESPONSE, SUCCESS, APPROVED, or DENIED",
        ));
    }
    write(host, |capture, owner, _| {
        let native = DomainVerification::domains(capture)?;
        let names = args.package_name.as_ref().map_or_else(
            || {
                owner
                    .attached_names()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            },
            |name| vec![name.clone()],
        );
        for name in names {
            let code = capture
                .state()
                .packages
                .get(&name)
                .and_then(|p| p.pkg.as_deref());
            if owner.package(&name).is_none() || code.is_none() {
                if args.package_name.is_none() {
                    continue;
                }
                return Err(Exception {
                    code: aim_binder_host::parcel::EX_SERVICE_SPECIFIC,
                    message: String::new(),
                    service_specific: 1,
                    parcelable: None,
                });
            }
            let declared = collector::collect(
                code.unwrap(),
                native.collector_policy(&name).map_err(failure)?,
                Kind::ValidAutoVerify,
            );
            let hosts = args.domains.as_ref().map_or_else(
                || declared.clone(),
                |hosts| {
                    hosts
                        .iter()
                        .flatten()
                        .filter(|h| declared.contains(h))
                        .cloned()
                        .collect()
                },
            );
            owner.set_status_internal(&name, &hosts, args.state, args.package_name.is_some());
        }
        Ok(())
    })
}

pub(super) fn reset(host: &Arc<AppHost>, args: api::ResetShellState) -> Result<(), Exception> {
    write(host, |capture, owner, _| {
        let native = DomainVerification::domains(capture)?;
        let names = args.package_names.as_ref().map_or_else(
            || {
                owner
                    .attached_names()
                    .map(|n| Some(n.to_owned()))
                    .collect::<Vec<_>>()
            },
            Clone::clone,
        );
        for name in names {
            let code = name.as_ref().and_then(|n| capture.state().packages.get(n));
            let pkg = code.and_then(|p| p.pkg.as_deref());
            if !args.user_only && pkg.is_none() {
                continue;
            }
            let name = name.as_deref().filter(|n| owner.package(n).is_some()).ok_or_else(||
                Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, format!(
                    "Attempt to invoke virtual method '{}' on a null object reference",
                    if !args.user_only { "android.util.ArrayMap com.android.server.pm.verify.domain.models.DomainVerificationPkgState.getStateMap()" }
                    else if args.user == -1 { "void com.android.server.pm.verify.domain.models.DomainVerificationPkgState.removeAllUsers()" }
                    else { "void com.android.server.pm.verify.domain.models.DomainVerificationPkgState.removeUser(int)" })))?;
            if args.user_only {
                owner.reset_users(name, args.user);
            } else {
                let policy = native.collector_policy(name).map_err(failure)?;
                let auto = collector::collect(pkg.unwrap(), policy, Kind::ValidAutoVerify);
                owner.reset_verification(
                    name,
                    &auto,
                    policy.linked_app
                        && code
                            .unwrap()
                            .setting_flags
                            .ok_or_else(|| failure("missing current package flags"))?
                            .0
                            & crate::package::settings::FLAG_SYSTEM
                            != 0,
                );
            }
        }
        Ok(())
    })
}

pub(super) fn verify(host: &Arc<AppHost>, args: api::VerifyShellPackages) -> Result<(), Exception> {
    use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;
    let system = host.package_system()?;
    let owner = system.package_bootstrap()?;
    let capture = system.capture_package_queries_for(&owner)?;
    let names = DomainVerification::domains(&capture)?
        .owner()
        .verification_packages(args.package_names.as_deref(), args.re_verify);
    if names.is_empty() {
        return Ok(());
    }
    system.check_package_bootstrap(&owner)?;
    let mut request = Parcel::new();
    bridge::SendDomainVerificationRequest {
        package_names: Some(names.into_iter().map(Some).collect()),
    }
    .write(&mut request);
    let reply = owner
        .owner
        .transact_as_owner(bridge::SEND_DOMAIN_VERIFICATION_REQUEST, &request, false)
        .map_err(|e| failure(format!("domain verifier delivery transport: {e}")))?;
    let mut reader = reply.reader();
    bridge::read_send_domain_verification_request_reply(&mut reader)
        .map_err(|e| failure(format!("domain verifier delivery reply: {e}")))??;
    if reader.remaining() != 0 {
        return Err(failure("trailing domain verifier delivery reply"));
    }
    Ok(())
}
