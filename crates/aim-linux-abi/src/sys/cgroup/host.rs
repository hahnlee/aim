//! Vendor scheduling policy, derived from controller values, never group names.

use super::*;
use std::sync::atomic::Ordering::SeqCst;

const BACKGROUND: i32 = 9;
const UTILITY: i32 = 17;
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

/// Ancestors constrain utilization hints and fair-class CPU weights.
/// Placement/frequency limits must not lower a thread's scheduling class.
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
            // Uclamp caps performance points, not scheduling priority. A
            // low maximum can remove our boost, never demote Linux priority
            // or bring Darwin's background disk/timer throttling with it.
            let cap = if max <= 50.0 { DEFAULT } else { INTERACTIVE };
            p.max = p.max.min(cap);
            // cpu.idle and CFS shares affect only fair-class threads;
            // utilization hints must preserve real-time scheduling priority.
            p.fair_max = p.fair_max.min(if number(dir, "cpu.idle")? != 0.0 {
                BACKGROUND
            } else if number(dir, "cpu.shares")? < 1024.0 {
                UTILITY
            } else {
                INTERACTIVE
            });
        }
        // A cpuset restricts placement, not priority or I/O. Darwin has no
        // arm64 CPU binding API; Background is not an equivalent policy.
        Controller::Cpuset => {}
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
    // MIG's mach_msg restarts interrupted IPC. Its result is a Mach code,
    // not errno (in particular, 4 is KERN_INVALID_ARGUMENT, not EINTR).
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
    super::super::process::task_priority_changed();
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
        assert_eq!(p.max, DEFAULT);
        assert_eq!(p.min, 0);
        assert_eq!(qos(DEFAULT, p.min, p.max) as i32, DEFAULT);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cpuset_preserves_priority_and_io_values_have_host_effects() {
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
        assert_eq!(p, Policy::default());
        cgroup_v1::populate(Controller::Blkio, &io, None).unwrap();
        for (value, want) in [("promote-to-rt", 1), ("restrict-to-be", 4), ("idle", 3)] {
            std::fs::write(io.join("blkio.prio.class"), value).unwrap();
            constrain(&mut p, Controller::Blkio, &io, &io).unwrap();
            assert_eq!(p.io, want);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn audio_profiles_keep_linux_scheduling_class() {
        // android-16.0.0_r1 audioserver.rc and our audio HAL's rc both use
        // ProcessCapacityHigh + HighPerformance. task_profiles.json joins
        // cpuset/foreground and cpu/foreground; init.rc copies the root CPU
        // mask and leaves shares=1024, idle=0, uclamp=0/max. Neither profile
        // joins blkio/background (restrict-to-be); the root is no-change.
        for rc in [
            include_str!(
                "../../../../aim-android-init/tests/golden/image/system/etc/init/audioserver.rc"
            ),
            include_str!("../../../../../hal/audio/android.hardware.audio.service-aidl.aim.rc"),
        ] {
            assert!(rc.lines().any(|line| {
                line.trim() == "task_profiles ProcessCapacityHigh HighPerformance"
            }));
        }
        let root = std::env::temp_dir().join(format!("aim-297-audio-profiles-{}", me()));
        for c in Controller::ALL {
            std::fs::create_dir_all(root.join(c.name())).unwrap();
            cgroup_v1::populate(c, &root.join(c.name()), None).unwrap();
        }
        // Native init parses task_profiles but does not attach them at
        // launch yet; also check the root memberships before a guest move.
        let mut initial = Policy::default();
        for c in Controller::ALL {
            let dir = root.join(c.name());
            constrain(&mut initial, c, &dir, &dir).unwrap();
        }
        assert_eq!(initial, Policy::default());
        let cpu = root.join("cpu");
        let cpuset = root.join("cpuset");
        for c in [Controller::Cpu, Controller::Cpuset] {
            let parent = root.join(c.name());
            let group = parent.join("foreground");
            std::fs::create_dir_all(&group).unwrap();
            cgroup_v1::populate(c, &group, Some(&parent)).unwrap();
        }
        let foreground = cpu.join("foreground");
        let mask = cpuset.join("foreground/cpus");
        std::fs::copy(cpuset.join("cpus"), &mask).unwrap();
        let policy = || {
            let mut p = Policy {
                min: INTERACTIVE,
                ..Policy::default()
            };
            constrain(&mut p, Controller::Cpu, &foreground, &cpu).unwrap();
            constrain(&mut p, Controller::Cpu, &cpu, &cpu).unwrap();
            constrain(
                &mut p,
                Controller::Cpuset,
                &cpuset.join("foreground"),
                &cpuset,
            )
            .unwrap();
            constrain(
                &mut p,
                Controller::Blkio,
                &root.join("blkio"),
                &root.join("blkio"),
            )
            .unwrap();
            p.min = p.min.min(p.max);
            p
        };
        assert_eq!(policy(), Policy::default());

        // Device CPU masks and utilization caps can change without changing
        // these profiles. They cannot demote normal or urgent audio threads,
        // nor request task suppression (max/fair_max == BACKGROUND).
        std::fs::write(cpuset.join("cpus"), "0-3\n").unwrap();
        std::fs::write(&mask, "0-1\n").unwrap();
        for max in ["max", "50", "25", "0"] {
            std::fs::write(foreground.join("cpu.uclamp.max"), max).unwrap();
            let p = policy();
            assert!(p.max >= DEFAULT && p.fair_max >= DEFAULT, "{max}: {p:?}");
            assert_eq!(p.io, 0, "{max}");
            for (nice, sched) in [
                (0, 0),
                (-19, 0),
                (0, 1),
                (0, 2),
                (10, 0),
                (19, 0),
                (0, 3),
                (0, 5),
            ] {
                let qos = super::super::super::process::host_qos(nice, sched) as i32;
                assert_eq!(super::qos(qos, p.min, p.max) as i32, qos, "{max}");
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
