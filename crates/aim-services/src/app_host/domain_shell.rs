//! Pinned domain shell: original Java parser/debugger, native captured state.
use super::{AppHost, domain_verification::DomainVerification};
use crate::package::{
    domain_verification::{
        collector::{self, Kind},
        names,
    },
    info::array_order,
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
    let system = host.package_system()?;
    let bridge = system.package_bootstrap()?;
    loop {
        let capture = system.capture_package_queries_for(&bridge)?;
        let native = DomainVerification::domains(&capture)?;
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
        let mut owner = native.owner().clone();
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
