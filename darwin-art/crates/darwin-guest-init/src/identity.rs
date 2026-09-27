//! The Linux credentials of a service process.
//!
//! Darwin processes all run as the host user, so init's `setuid`,
//! `setgroups` and `capset` in the child become a per-process identity file
//! that the syscall layer reads at startup (`linux-run --identity FILE`)
//! and reports from `getuid`, `getresuid`, `getgroups`, `capget` and
//! friends. The format is in `docs/guest-init-contract.md`, "Identity".

use std::fmt::Write as _;

use darwin_android_init::rc::Rlimit;

/// All capabilities up to `CAP_LAST_CAP` (40, `CAP_CHECKPOINT_RESTORE`).
pub const ALL_CAPABILITIES: u64 = (1u64 << 41) - 1;

/// `AID_ROOT`.
pub const AID_ROOT: u32 = 0;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub effective: u64,
    pub permitted: u64,
    pub inheritable: u64,
    pub ambient: u64,
    pub bounding: u64,
}

impl Capabilities {
    /// What init leaves a service with (`SetUpCapabilities` /
    /// `SetCapsForExec`): an explicit `capabilities` line becomes the
    /// permitted, effective, inheritable and ambient sets with the bounding
    /// set reduced to it; root without one keeps every capability; any
    /// other uid has none.
    pub fn for_service(uid: u32, capabilities: Option<u64>) -> Self {
        match capabilities {
            Some(mask) => Self {
                effective: mask,
                permitted: mask,
                inheritable: mask,
                ambient: mask,
                bounding: mask,
            },
            None if uid == AID_ROOT => Self {
                effective: ALL_CAPABILITIES,
                permitted: ALL_CAPABILITIES,
                inheritable: 0,
                ambient: 0,
                bounding: ALL_CAPABILITIES,
            },
            None => Self {
                bounding: ALL_CAPABILITIES,
                ..Self::default()
            },
        }
    }
}

/// One process's guest credentials and the attributes init applies before
/// `execv`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The service (or `exec N (...)` name) it was started for.
    pub service: String,
    pub uid: u32,
    pub gid: u32,
    /// Supplementary groups in `setgroups` order.
    pub groups: Vec<u32>,
    pub capabilities: Capabilities,
    /// SELinux label (`seclabel`), empty when init would compute it from
    /// the file context (permissive: reported as-is).
    pub seclabel: String,
    pub priority: i32,
    pub oom_score_adjust: i32,
    /// init's own `setrlimit`s, then the service's `rlimit` lines.
    pub rlimits: Vec<Rlimit>,
}

impl Identity {
    pub fn to_file_text(&self) -> String {
        let mut out = String::from("# darwin-guest-init identity v1\n");
        let _ = writeln!(out, "service\t{}", self.service);
        let _ = writeln!(out, "uid\t{}", self.uid);
        let _ = writeln!(out, "gid\t{}", self.gid);
        let groups: Vec<String> = self.groups.iter().map(u32::to_string).collect();
        let _ = writeln!(out, "groups\t{}", groups.join(" "));
        let caps = &self.capabilities;
        let _ = writeln!(out, "cap_effective\t{:#x}", caps.effective);
        let _ = writeln!(out, "cap_permitted\t{:#x}", caps.permitted);
        let _ = writeln!(out, "cap_inheritable\t{:#x}", caps.inheritable);
        let _ = writeln!(out, "cap_ambient\t{:#x}", caps.ambient);
        let _ = writeln!(out, "cap_bounding\t{:#x}", caps.bounding);
        let _ = writeln!(out, "seclabel\t{}", self.seclabel);
        let _ = writeln!(out, "priority\t{}", self.priority);
        let _ = writeln!(out, "oom_score_adj\t{}", self.oom_score_adjust);
        for limit in &self.rlimits {
            let _ = writeln!(
                out,
                "rlimit\t{}\t{}\t{}",
                limit.resource,
                limit_text(limit.soft),
                limit_text(limit.hard)
            );
        }
        out
    }

    pub fn parse_file_text(text: &str) -> Result<Self, String> {
        let mut identity = Identity {
            service: String::new(),
            uid: 0,
            gid: 0,
            groups: Vec::new(),
            capabilities: Capabilities::default(),
            seclabel: String::new(),
            priority: 0,
            oom_score_adjust: 0,
            rlimits: Vec::new(),
        };
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('\t').ok_or(format!("bad line '{line}'"))?;
            let num = |v: &str| -> Result<u64, String> {
                let parsed = match v.strip_prefix("0x") {
                    Some(hex) => u64::from_str_radix(hex, 16),
                    None => v.parse(),
                };
                parsed.map_err(|e| format!("{key}: {e}"))
            };
            match key {
                "service" => identity.service = value.to_string(),
                "uid" => identity.uid = num(value)? as u32,
                "gid" => identity.gid = num(value)? as u32,
                "groups" => {
                    identity.groups = value
                        .split(' ')
                        .filter(|s| !s.is_empty())
                        .map(|g| num(g).map(|g| g as u32))
                        .collect::<Result<_, _>>()?
                }
                "cap_effective" => identity.capabilities.effective = num(value)?,
                "cap_permitted" => identity.capabilities.permitted = num(value)?,
                "cap_inheritable" => identity.capabilities.inheritable = num(value)?,
                "cap_ambient" => identity.capabilities.ambient = num(value)?,
                "cap_bounding" => identity.capabilities.bounding = num(value)?,
                "seclabel" => identity.seclabel = value.to_string(),
                "priority" => identity.priority = value.parse().map_err(|e| format!("{e}"))?,
                "oom_score_adj" => {
                    identity.oom_score_adjust = value.parse().map_err(|e| format!("{e}"))?
                }
                "rlimit" => {
                    let fields: Vec<&str> = value.split('\t').collect();
                    let [resource, soft, hard] = fields[..] else {
                        return Err(format!("bad rlimit '{value}'"));
                    };
                    let limit = |v: &str| {
                        if v == "unlimited" {
                            Ok(u64::MAX)
                        } else {
                            num(v)
                        }
                    };
                    identity.rlimits.push(Rlimit {
                        resource: num(resource)? as u32,
                        soft: limit(soft)?,
                        hard: limit(hard)?,
                    });
                }
                _ => {}
            }
        }
        Ok(identity)
    }
}

fn limit_text(value: u64) -> String {
    if value == u64::MAX {
        "unlimited".to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_file_round_trips() {
        let identity = Identity {
            service: "logd".into(),
            uid: 1036,
            gid: 1036,
            groups: vec![1036, 1000, 1032, 3009],
            capabilities: Capabilities::for_service(1036, Some((1 << 34) | (1 << 30))),
            seclabel: String::new(),
            priority: 10,
            oom_score_adjust: -1000,
            rlimits: vec![Rlimit {
                resource: 7,
                soft: 32768,
                hard: u64::MAX,
            }],
        };
        let text = identity.to_file_text();
        assert!(text.contains("cap_ambient\t0x440000000\n"), "{text}");
        assert_eq!(Identity::parse_file_text(&text).unwrap(), identity);
    }

    #[test]
    fn capability_defaults_follow_init() {
        assert_eq!(
            Capabilities::for_service(0, None).effective,
            ALL_CAPABILITIES
        );
        assert_eq!(Capabilities::for_service(1000, None).effective, 0);
        assert_eq!(Capabilities::for_service(1000, Some(0)).permitted, 0);
        assert_eq!(Capabilities::for_service(1000, Some(0)).bounding, 0);
    }
}
