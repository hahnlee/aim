//! Android scheduling-group membership, separate from the Mach mechanism.
//! Root membership is SP_FOREGROUND, as in AOSP get_sched_policy_from_group("").
//! Darwin latency/throughput tiers are an adapter policy, not Linux CPU sets
//! or a promise of real-time scheduling. CPU affinity is not implemented here.
use crate::scheduling::{Thread, policy, thread};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Group(i32);
impl Group {
    fn parse(value: i32) -> Result<Self, i32> {
        match value {
            -1 => Ok(Self(1)),
            0..=8 => Ok(Self(value)),
            _ => Err(22),
        }
    }
    fn tier(self) -> i32 {
        match self.0 {
            0 | 7 => 5, // background / restricted
            2 => 4,     // system-background
            1 => 3,     // foreground
            8 => 1,     // foreground-window
            _ => 0,     // audio / top-app / RT-app (group, not SCHED_FIFO)
        }
    }
}
struct Member {
    capability: Thread,
    group: Group,
}
static MEMBERS: LazyLock<Mutex<HashMap<u64, Member>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn get(tid: i32) -> Result<i32, i32> {
    let target = thread(tid)?;
    let id = target.id()?;
    let mut members = MEMBERS.lock().map_err(|_| 5)?;
    members.retain(|_, member| member.capability.id().is_ok());
    Ok(members.get(&id).map_or(1, |member| member.group.0))
}
fn set(tid: i32, group: i32) -> Result<(), i32> {
    let group = Group::parse(group)?;
    let target = thread(tid)?;
    let id = target.id()?;
    let mut members = MEMBERS.lock().map_err(|_| 5)?;
    members.retain(|_, member| member.capability.id().is_ok());
    // SDK thread_policy.h: LATENCY=7 / THROUGHPUT=8, one integer each.
    let old_latency = policy(&target, 7)?;
    let old_throughput = policy(&target, 8)?;
    target.set_policy(7, (0xff << 16) | (group.tier() + 1))?;
    if let Err(error) = target.set_policy(8, (0xfe << 16) | (group.tier() + 1)) {
        target.set_policy(7, old_latency)?;
        target.set_policy(8, old_throughput)?;
        return Err(error);
    }
    members.insert(
        id,
        Member {
            capability: target,
            group,
        },
    );
    Ok(())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_thread_get_group(tid: i32, output: *mut i32) -> i32 {
    if output.is_null() {
        return 22;
    }
    match get(tid) {
        Ok(group) => {
            unsafe {
                output.write(group);
            }
            0
        }
        Err(error) => error,
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_thread_set_group(tid: i32, group: i32) -> i32 {
    set(tid, group).err().unwrap_or(0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn groups_apply_kernel_policy_and_reject_invalid_threads() {
        let tid = std::thread::spawn(|| {
            let target = thread(0).unwrap();
            assert_eq!(get(0), Ok(1));
            for group in [0, 1, 2, 3, 4, 5, 6, 7, 8, -1] {
                assert_eq!(set(0, group), Ok(()));
                let parsed = Group::parse(group).unwrap();
                assert_eq!(get(0), Ok(parsed.0));
                assert_eq!(policy(&target, 7), Ok((0xff << 16) | (parsed.tier() + 1)));
                assert_eq!(policy(&target, 8), Ok((0xfe << 16) | (parsed.tier() + 1)));
            }
            assert_eq!(set(0, 9), Err(22));
            assert_eq!(get(0), Ok(1));
            target.id().unwrap() as i32
        })
        .join()
        .unwrap();
        assert_eq!(get(tid), Err(3));
        assert_eq!(get(i32::MAX), Err(3));
    }
}
