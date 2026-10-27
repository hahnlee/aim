//! Vendor scheduling policy, derived from controller values, never group names.

use super::*;
use std::sync::atomic::Ordering::SeqCst;

const BACKGROUND: i32 = 9;
const UTILITY: i32 = 17;
#[cfg(test)]
const DEFAULT: i32 = 21;
const INITIATED: i32 = 25;
const INTERACTIVE: i32 = 33;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Policy {
    pub min: i32,
    pub max: i32,
    pub fair_max: i32,
    pub io: i32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            min: 0,
            max: INTERACTIVE,
            fair_max: INTERACTIVE,
            io: 0,
        }
    }
}

fn value(dir: &Path, name: &str) -> Result<String, Errno> {
    std::fs::read_to_string(dir.join(name)).map_err(io_error)
}

fn number(dir: &Path, name: &str) -> Result<f64, Errno> {
    let v = value(dir, name)?;
    if v.trim() == "max" {
        return Ok(100.0);
    }
    v.trim().parse().map_err(|_| EINVAL)
}

/// Linux's effective uclamp bounds are constrained by ancestors; an idle
/// ancestor or a restricted cpuset also constrains every descendant.
fn constrain(p: &mut Policy, c: Controller, dir: &Path, root: &Path) -> Result<(), Errno> {
    match c {
        Controller::Cpu if dir != root => {
            let min = number(dir, "cpu.uclamp.min")?;
            let max = number(dir, "cpu.uclamp.max")?;
            p.min = p.min.min(if min >= 50.0 {
                INTERACTIVE
            } else if min > 0.0 || number(dir, "cpu.uclamp.latency_sensitive")? != 0.0 {
                INITIATED
            } else {
                0
            });
            let cap = if max <= 25.0 {
                BACKGROUND
            } else if max <= 50.0 {
                UTILITY
            } else {
                INTERACTIVE
            };
            p.max = p.max.min(cap);
            // cpu.idle and CFS shares affect only fair-class threads;
            // utilization clamps apply to real-time threads as well.
            p.fair_max = p.fair_max.min(if number(dir, "cpu.idle")? != 0.0 {
                BACKGROUND
            } else if number(dir, "cpu.shares")? < 1024.0 {
                UTILITY
            } else {
                INTERACTIVE
            });
        }
        Controller::Cpuset if dir != root => {
            let cpus = cgroup_v1::parse_list(value(dir, "cpus")?.trim(), 32)?;
            let online = cgroup_v1::parse_list(value(root, "cpus")?.trim(), 32)?;
            // Darwin has no CPU binding API on arm64. A reduced CPU set is
            // a capacity restriction, represented by background scheduling.
            if !cpus.is_empty() && cpus.len() < online.len() {
                p.max = BACKGROUND;
            }
        }
        Controller::Blkio => {
            let io = match value(dir, "blkio.prio.class")?.trim() {
                "idle" => 3,
                "restrict-to-be" => 4,
                "promote-to-rt" | "none-to-rt" => 1,
                _ => 0,
            };
            p.io = if p.io == 3 || io == 3 {
                3
            } else {
                p.io.max(io)
            };
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn policies(root: &Path, m: &Membership, pid: i32) -> Result<Vec<(i32, Policy)>, Errno> {
    let hierarchies: Vec<_> = vfs::cgroup_v1_hierarchies()
        .into_iter()
        .map(|(c, r)| {
            let state = if r == root {
                m.clone()
            } else {
                checked_state(&r)?
            };
            Ok((c, r, state))
        })
        .collect::<Result<_, Errno>>()?;
    tids(pid)
        .into_iter()
        .filter(|tid| super::super::thread::owner(*tid) == pid)
        .map(|tid| {
            let mut p = Policy::default();
            for (c, root, state) in &hierarchies {
                let group = group_of(state, tid);
                let mut dir = root.join(group.trim_start_matches('/'));
                if *c == Controller::Cpu && dir != *root {
                    p.min = INTERACTIVE;
                }
                loop {
                    constrain(&mut p, *c, &dir, root)?;
                    if dir == *root {
                        break;
                    }
                    if !dir.pop() {
                        return Err(EINVAL);
                    }
                }
            }
            p.min = p.min.min(p.max);
            Ok((tid, p))
        })
        .collect()
}

// XNU mach/task_policy_private.h: task_suppression_policy is 16 integers,
// not the version-dependent requested/effective policy bitfields.
#[repr(C)]
#[derive(Default)]
struct Suppression {
    active: i32,
    lowpri_cpu: i32,
    timer_throttle: i32,
    disk_throttle: i32,
    cpu_limit: i32,
    suspend: i32,
    throughput_qos: i32,
    suppressed_cpu: i32,
    background_sockets: i32,
    reserved: [i32; 7],
}

unsafe extern "C" {
    static mach_task_self_: libc::mach_port_t;
    fn task_policy_set(task: libc::mach_port_t, flavor: u32, policy: *mut i32, count: u32) -> i32;
}

/// Apply in the target process, where Darwin permits thread QoS and disk
/// policy changes. Process backgrounding takes effect immediately, including
/// CPU-bound threads which have not entered the syscall layer again.
pub(crate) fn apply(policies: &[(i32, Policy)]) -> Result<Vec<(i32, Policy)>, Errno> {
    let before = super::super::thread::with_table(|threads| {
        threads
            .values()
            .map(|th| {
                (
                    th.tid,
                    Policy {
                        min: th.sched.cgroup_min.load(SeqCst),
                        max: th.sched.cgroup_max.load(SeqCst),
                        fair_max: th.sched.cgroup_fair_max.load(SeqCst),
                        io: th.sched.cgroup_io.load(SeqCst),
                    },
                )
            })
            .collect()
    });
    let all_bg = super::super::thread::with_table(|threads| {
        !threads.is_empty()
            && threads.values().all(|th| {
                let (max, fair_max) = policies.iter().find(|(tid, _)| *tid == th.tid).map_or_else(
                    || {
                        (
                            th.sched.cgroup_max.load(SeqCst),
                            th.sched.cgroup_fair_max.load(SeqCst),
                        )
                    },
                    |(_, p)| (p.max, p.fair_max),
                );
                let cap = if matches!(th.sched.policy.load(SeqCst), 1 | 2) {
                    max
                } else {
                    max.min(fair_max)
                };
                cap == BACKGROUND
            })
    });
    let mut policy = Suppression {
        active: i32::from(all_bg),
        lowpri_cpu: i32::from(all_bg),
        disk_throttle: i32::from(all_bg),
        ..Suppression::default()
    };
    // TASK_SUPPRESSION_POLICY applies the same MAXPRI_THROTTLE CPU cap to
    // this entire task without PRIO_DARWIN_PROCESS's MAC scheduling check.
    // No suspension is requested: the control agent must continue to run.
    let result = unsafe { task_policy_set(mach_task_self_, 3, (&raw mut policy).cast(), 16) };
    if result != 0 {
        return Err(match result {
            2 => EPERM,                     // KERN_PROTECTION_FAILURE
            3 => crate::errno::ENOMEM,      // KERN_RESOURCE_SHORTAGE
            4 => EINVAL,                    // KERN_INVALID_ARGUMENT
            37 => ESRCH,                    // KERN_TERMINATED
            46 => crate::errno::EOPNOTSUPP, // KERN_NOT_SUPPORTED
            _ => EIO,
        });
    }
    super::super::thread::with_table(|threads| {
        for (tid, p) in policies {
            if let Some(th) = threads.get(tid) {
                th.sched.cgroup_min.store(p.min, SeqCst);
                th.sched.cgroup_max.store(p.max, SeqCst);
                th.sched.cgroup_fair_max.store(p.fair_max, SeqCst);
                th.sched.cgroup_io.store(p.io, SeqCst);
                th.sched.host_stale.store(true, SeqCst);
            }
        }
    });
    Ok(before)
}

pub(super) fn update(
    root: &Path,
    old: &Membership,
    new: &Membership,
    controls: Option<&Path>,
) -> Result<(), Errno> {
    let affected: std::collections::BTreeSet<_> = all_tasks()
        .into_iter()
        .filter(|(_, tid)| {
            let group = group_of(new, *tid);
            if let Some(dir) = controls {
                let changed = cgroup_v1::group_path(dir, root);
                changed == "/" || group == changed || group.starts_with(&format!("{changed}/"))
            } else {
                group_of(old, *tid) != group
            }
        })
        .map(|(_, tid)| super::super::thread::owner(tid))
        .collect();
    let plans = affected
        .into_iter()
        .map(|pid| Ok((pid, policies(root, new, pid)?)))
        .collect::<Result<Vec<_>, Errno>>()?;
    let mut done: Vec<(i32, Vec<(i32, Policy)>)> = Vec::new();
    for (pid, after) in plans {
        let result = if pid == me() {
            apply(&after)
        } else {
            super::super::ptrace::remote_cgroup_policy(pid, &after)
        };
        match result {
            Ok(before) => done.push((pid, before)),
            Err(ESRCH) if !super::super::pidns::running(pid) => {}
            Err(error) => {
                for (pid, policy) in done {
                    if pid == me() {
                        apply(&policy)?;
                    } else {
                        super::super::ptrace::remote_cgroup_policy(pid, &policy)?;
                    }
                }
                return Err(error);
            }
        }
    }
    Ok(())
}

pub(crate) fn qos(qos: i32, min: i32, max: i32) -> libc::qos_class_t {
    use libc::qos_class_t::*;
    match qos.max(min).min(max) {
        BACKGROUND => QOS_CLASS_BACKGROUND,
        UTILITY => QOS_CLASS_UTILITY,
        INITIATED => QOS_CLASS_USER_INITIATED,
        INTERACTIVE => QOS_CLASS_USER_INTERACTIVE,
        _ => QOS_CLASS_DEFAULT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_is_bounded_by_group_clamps() {
        assert_eq!(qos(DEFAULT, INITIATED, INTERACTIVE) as i32, INITIATED);
        assert_eq!(qos(INTERACTIVE, 0, BACKGROUND) as i32, BACKGROUND);
        assert_eq!(qos(UTILITY, 0, INTERACTIVE) as i32, UTILITY);
    }

    #[test]
    fn values_and_ancestors_determine_policy_without_names() {
        let root = std::env::temp_dir().join(format!("aim-297-policy-{}", me()));
        std::fs::create_dir_all(root.join("arbitrary/child")).unwrap();
        cgroup_v1::populate(Controller::Cpu, &root, None).unwrap();
        let dir = root.join("arbitrary");
        let child = dir.join("child");
        cgroup_v1::populate(Controller::Cpu, &dir, Some(&root)).unwrap();
        cgroup_v1::populate(Controller::Cpu, &child, Some(&dir)).unwrap();
        std::fs::write(dir.join("cpu.uclamp.max"), "20.00\n").unwrap();
        std::fs::write(child.join("cpu.uclamp.min"), "80.00\n").unwrap();
        let mut p = Policy {
            min: INTERACTIVE,
            ..Policy::default()
        };
        constrain(&mut p, Controller::Cpu, &child, &root).unwrap();
        assert_eq!(p.min, INTERACTIVE);
        constrain(&mut p, Controller::Cpu, &dir, &root).unwrap();
        assert_eq!(p.max, BACKGROUND);
        assert_eq!(p.min, 0);
        assert_eq!(qos(DEFAULT, p.min, p.max) as i32, BACKGROUND);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cpuset_and_io_values_have_host_effects() {
        let root = std::env::temp_dir().join(format!("aim-297-controllers-{}", me()));
        let cpus = root.join("cpus");
        let child = cpus.join("any-name");
        let io = root.join("io");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&io).unwrap();
        cgroup_v1::populate(Controller::Cpuset, &cpus, None).unwrap();
        cgroup_v1::populate(Controller::Cpuset, &child, Some(&cpus)).unwrap();
        std::fs::write(cpus.join("cpus"), "0-3\n").unwrap();
        std::fs::write(child.join("cpus"), "0-1\n").unwrap();
        let mut p = Policy::default();
        constrain(&mut p, Controller::Cpuset, &child, &cpus).unwrap();
        assert_eq!(p.max, BACKGROUND);
        cgroup_v1::populate(Controller::Blkio, &io, None).unwrap();
        for (value, want) in [("promote-to-rt", 1), ("restrict-to-be", 4), ("idle", 3)] {
            std::fs::write(io.join("blkio.prio.class"), value).unwrap();
            constrain(&mut p, Controller::Blkio, &io, &io).unwrap();
            assert_eq!(p.io, want);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
