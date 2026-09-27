//! futex (experiments/p0/03).
//!
//! - Private futexes (FUTEX_PRIVATE_FLAG, or a non-private op on private
//!   memory) use an in-process waiter table: exact wake counts, bitsets,
//!   requeue, wake-op and priority-inheritance locks. Waiters block on their
//!   thread's parker, so guest signals and precise deadlines interrupt them.
//! - A non-private op on MAP_SHARED memory uses Darwin's shared `__ulock`
//!   flavour, which other processes (and the host property service) wake
//!   with UL_COMPARE_AND_WAIT_SHARED. The flavour follows the mapping, as
//!   Linux keys a non-private futex by its backing object; `os_sync` keeps
//!   the shared and private flavours apart. It has no bitsets: a bitset
//!   wake wakes waiters of any bitset (spuriously, for the others), so a
//!   bitset wake of fewer than all waiters may miss its target. Cross-process
//!   bitset users (libfmq's EventFlag) wake all.
//!
//! PI futexes hand ownership to the first waiter on unlock, as Linux does;
//! there is no priority boosting (Darwin schedules the host threads).

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::SeqCst};

use super::park;
use super::signal;
use super::thread::{self, Thread};
use crate::errno::{EAGAIN, EFAULT, EINTR, EINVAL, ENOSYS, EPERM};

const FUTEX_WAIT: u64 = 0;
const FUTEX_WAKE: u64 = 1;
const FUTEX_REQUEUE: u64 = 3;
const FUTEX_CMP_REQUEUE: u64 = 4;
const FUTEX_WAKE_OP: u64 = 5;
const FUTEX_LOCK_PI: u64 = 6;
const FUTEX_UNLOCK_PI: u64 = 7;
const FUTEX_TRYLOCK_PI: u64 = 8;
const FUTEX_WAIT_BITSET: u64 = 9;
const FUTEX_WAKE_BITSET: u64 = 10;
const FUTEX_LOCK_PI2: u64 = 13;
const FUTEX_PRIVATE_FLAG: u64 = 128;
const FUTEX_CLOCK_REALTIME: u64 = 256;
const BITSET_ANY: u32 = u32::MAX;

const FUTEX_WAITERS: u32 = 0x8000_0000;
const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
const FUTEX_TID_MASK: u32 = 0x3fff_ffff;

const ETIMEDOUT: i64 = 110;
const EDEADLK: i64 = 35;

const UL_COMPARE_AND_WAIT_SHARED: u32 = 3;
const ULF_WAKE_ALL: u32 = 0x100;
const ULF_NO_ERRNO: u32 = 0x0100_0000;
/// Longest single shared wait: a guest signal that races with entering the
/// kernel wait is noticed within this time.
const SHARED_SLICE_NS: u64 = 100_000_000;

unsafe extern "C" {
    fn __ulock_wait2(
        op: u32,
        addr: *mut libc::c_void,
        value: u64,
        timeout_ns: u64,
        value2: u64,
    ) -> i32;
    fn __ulock_wake(op: u32, addr: *mut libc::c_void, wake_value: u64) -> i32;
}

fn word(addr: u64) -> &'static AtomicU32 {
    // SAFETY: an aligned guest futex word; guest memory outlives the call.
    unsafe { AtomicU32::from_ptr(addr as *mut u32) }
}

// ---- waiter table ---------------------------------------------------------------

/// A blocked thread, on its own stack, linked into its bucket.
struct Waiter {
    next: *mut Waiter,
    /// Changed by requeue under both bucket locks.
    addr: AtomicU64,
    bitset: u32,
    pi: bool,
    thread: *const Thread,
    woken: AtomicU32,
}

struct Bucket {
    lock: UnsafeCell<libc::os_unfair_lock>,
    head: UnsafeCell<*mut Waiter>,
}

// SAFETY: `head` is only touched under `lock`.
unsafe impl Sync for Bucket {}

const NBUCKETS: usize = 256;
static BUCKETS: [Bucket; NBUCKETS] = [const {
    Bucket {
        lock: UnsafeCell::new(libc::OS_UNFAIR_LOCK_INIT),
        head: UnsafeCell::new(std::ptr::null_mut()),
    }
}; NBUCKETS];

fn bucket(addr: u64) -> &'static Bucket {
    &BUCKETS[((addr >> 2).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 56) as usize % NBUCKETS]
}

/// Holds one or two bucket locks (two in address order, for requeue).
struct Locked(&'static Bucket, Option<&'static Bucket>);

impl Locked {
    fn one(b: &'static Bucket) -> Self {
        // SAFETY: a static lock.
        unsafe { libc::os_unfair_lock_lock(b.lock.get()) };
        Locked(b, None)
    }

    fn two(a: &'static Bucket, b: &'static Bucket) -> Self {
        if std::ptr::eq(a, b) {
            return Locked::one(a);
        }
        let (lo, hi) = if (a as *const Bucket) < (b as *const Bucket) {
            (a, b)
        } else {
            (b, a)
        };
        // SAFETY: static locks, taken in address order.
        unsafe {
            libc::os_unfair_lock_lock(lo.lock.get());
            libc::os_unfair_lock_lock(hi.lock.get());
        }
        Locked(lo, Some(hi))
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: locks taken in `one`/`two`.
        unsafe {
            if let Some(h) = self.1 {
                libc::os_unfair_lock_unlock(h.lock.get());
            }
            libc::os_unfair_lock_unlock(self.0.lock.get());
        }
    }
}

/// Unlinks and returns the waiters `pick` selects from `b` (under its lock).
///
/// # Safety
/// The caller holds `b`'s lock.
unsafe fn drain(b: &Bucket, mut pick: impl FnMut(&Waiter) -> bool) -> Vec<*mut Waiter> {
    let mut out = Vec::new();
    // SAFETY: caller holds the lock; waiters stay alive while linked.
    unsafe {
        let mut p = b.head.get();
        while !(*p).is_null() {
            let w = *p;
            if pick(&*w) {
                *p = (*w).next;
                out.push(w);
            } else {
                p = &mut (*w).next;
            }
        }
    }
    out
}

/// Push `w` onto `b` (under its lock).
///
/// # Safety
/// The caller holds `b`'s lock and `w` stays alive while linked.
unsafe fn link(b: &Bucket, w: *mut Waiter) {
    // SAFETY: caller contract.
    unsafe {
        (*w).next = *b.head.get();
        *b.head.get() = w;
    }
}

/// Wake unlinked waiters. Claim under the lock, unpark after: a claimed
/// waiter may return (and its stack frame die) at once, so only its
/// thread (kept alive by a reference taken under the lock) is touched.
struct Wakes(Vec<Arc<Thread>>);

impl Wakes {
    /// # Safety
    /// `w` is unlinked and alive; the caller holds its bucket's lock.
    unsafe fn claim(&mut self, w: *mut Waiter) {
        // SAFETY: caller contract; `thread` came from Arc::into_raw.
        unsafe {
            Arc::increment_strong_count((*w).thread);
            self.0.push(Arc::from_raw((*w).thread));
            (*w).woken.store(1, SeqCst);
        }
    }

    fn run(self) {
        for t in self.0 {
            t.park.unpark();
        }
    }
}

/// Block on the table until woken, the deadline, or a guest signal.
/// `check` runs under the bucket lock first: Some(r) returns r at once.
fn table_wait(
    addr: u64,
    bitset: u32,
    pi: bool,
    deadline: Option<u64>,
    check: impl FnOnce() -> Option<i64>,
) -> i64 {
    let Some(th) = thread::current() else {
        return -(EINVAL as i64);
    };
    let mut w = Waiter {
        next: std::ptr::null_mut(),
        addr: AtomicU64::new(addr),
        bitset,
        pi,
        thread: th as *const Thread,
        woken: AtomicU32::new(0),
    };
    {
        let b = bucket(addr);
        let _l = Locked::one(b);
        if let Some(r) = check() {
            return r;
        }
        // SAFETY: under the lock; `w` outlives its time in the list (it is
        // unlinked by a waker or below before this frame returns).
        unsafe { link(b, &mut w) };
    }
    let r = loop {
        if w.woken.load(SeqCst) != 0 {
            break 0;
        }
        if deadline.is_some_and(|d| park::monotonic() >= d) {
            break -ETIMEDOUT;
        }
        if signal::interrupted(th) {
            break -(EINTR as i64);
        }
        th.park.park(th.tid, deadline);
    };
    if r == 0 {
        return 0;
    }
    // Requeue may have moved us: lock whichever bucket we are in now.
    loop {
        let b = bucket(w.addr.load(SeqCst));
        let _l = Locked::one(b);
        if !std::ptr::eq(b, bucket(w.addr.load(SeqCst))) {
            continue;
        }
        if w.woken.load(SeqCst) != 0 {
            // Claimed while leaving: the wake (or PI handoff) counts.
            return 0;
        }
        let me = &mut w as *mut Waiter;
        // SAFETY: under the lock.
        unsafe { drain(b, |x| std::ptr::eq(x, me)) };
        return r;
    }
}

fn table_wake(addr: u64, n: u32, bitset: u32) -> i64 {
    let b = bucket(addr);
    let mut wakes = Wakes(Vec::new());
    {
        let _l = Locked::one(b);
        let mut left = n;
        // SAFETY: under the lock.
        for w in unsafe {
            drain(b, |w| {
                let hit =
                    left > 0 && !w.pi && w.addr.load(SeqCst) == addr && w.bitset & bitset != 0;
                left -= hit as u32;
                hit
            })
        } {
            // SAFETY: just unlinked under the lock.
            unsafe { wakes.claim(w) };
        }
    }
    let n = wakes.0.len() as i64;
    wakes.run();
    n
}

fn table_requeue(addr: u64, nwake: u32, nreq: u32, addr2: u64, cmp: Option<u32>) -> i64 {
    let (b1, b2) = (bucket(addr), bucket(addr2));
    let mut wakes = Wakes(Vec::new());
    let moved = {
        let _l = Locked::two(b1, b2);
        if cmp.is_some_and(|v| word(addr).load(SeqCst) != v) {
            return -(EAGAIN as i64);
        }
        let mut left = nwake as u64 + nreq as u64;
        // SAFETY: under both locks.
        let ws = unsafe {
            drain(b1, |w| {
                let hit = left > 0 && !w.pi && w.addr.load(SeqCst) == addr;
                left -= hit as u64;
                hit
            })
        };
        // drain collects in list order; the first `nwake` wake.
        let mut moved = 0;
        for (i, w) in ws.into_iter().enumerate() {
            if (i as u32) < nwake {
                // SAFETY: unlinked under the lock.
                unsafe { wakes.claim(w) };
            } else {
                // SAFETY: under both locks.
                unsafe {
                    (*w).addr.store(addr2, SeqCst);
                    link(b2, w);
                }
                moved += 1;
            }
        }
        moved
    };
    let n = wakes.0.len() as i64 + moved;
    wakes.run();
    n
}

/// FUTEX_WAKE_OP's encoded operation on `*addr2`; returns the old value.
fn wake_op_apply(addr2: u64, enc: u32) -> Result<u32, i64> {
    let op = (enc >> 28) & 7;
    let sext = |v: u32| ((v << 20) as i32 >> 20) as u32;
    let mut arg = sext((enc >> 12) & 0xfff);
    if enc & (8 << 28) != 0 {
        arg = 1 << (arg & 31);
    }
    let w = word(addr2);
    let f = |old: u32| match op {
        0 => Some(arg),
        1 => Some(old.wrapping_add(arg)),
        2 => Some(old | arg),
        3 => Some(old & !arg),
        4 => Some(old ^ arg),
        _ => None,
    };
    if op > 4 {
        return Err(-(ENOSYS as i64));
    }
    Ok(w.fetch_update(SeqCst, SeqCst, f).unwrap_or(0))
}

fn wake_op_cmp(old: u32, enc: u32) -> bool {
    let (old, arg) = (old as i32, ((enc & 0xfff) << 20) as i32 >> 20);
    match (enc >> 24) & 15 {
        0 => old == arg,
        1 => old != arg,
        2 => old < arg,
        3 => old <= arg,
        4 => old > arg,
        5 => old >= arg,
        _ => false,
    }
}

// ---- priority inheritance ---------------------------------------------------------

/// Hand a PI futex whose owner released (or died holding) it to the first
/// PI waiter; with none, store `none`. Caller holds the bucket lock.
///
/// # Safety
/// As `drain`.
unsafe fn pi_handoff(b: &Bucket, addr: u64, keep: u32, none: u32, wakes: &mut Wakes) {
    let mut first = true;
    // SAFETY: caller contract.
    let got = unsafe {
        drain(b, |w| {
            let hit = first && w.pi && w.addr.load(SeqCst) == addr;
            first &= !hit;
            hit
        })
    };
    let Some(&w) = got.first() else {
        word(addr).store(none, SeqCst);
        return;
    };
    let mut more = false;
    // SAFETY: under the lock.
    unsafe {
        drain(b, |x| {
            more |= x.pi && x.addr.load(SeqCst) == addr;
            false
        })
    };
    // SAFETY: `w` is unlinked and alive until claimed.
    let tid = unsafe { (*(*w).thread).tid } as u32;
    word(addr).store(tid | keep | if more { FUTEX_WAITERS } else { 0 }, SeqCst);
    // SAFETY: unlinked under the lock.
    unsafe { wakes.claim(w) };
}

fn lock_pi(addr: u64, deadline: Option<u64>, try_only: bool) -> i64 {
    let Some(th) = thread::current() else {
        return -(EINVAL as i64);
    };
    let tid = th.tid as u32;
    let w = word(addr);
    // Runs under the bucket lock, so an unlock cannot slip between setting
    // FUTEX_WAITERS and queueing.
    let acquire = || loop {
        let v = w.load(SeqCst);
        if v & FUTEX_TID_MASK == 0 {
            let nv = tid | v & (FUTEX_OWNER_DIED | FUTEX_WAITERS);
            if w.compare_exchange(v, nv, SeqCst, SeqCst).is_ok() {
                return Some(0);
            }
            continue;
        }
        if v & FUTEX_TID_MASK == tid {
            return Some(-EDEADLK);
        }
        if try_only {
            return Some(-(EAGAIN as i64));
        }
        if v & FUTEX_WAITERS != 0
            || w.compare_exchange(v, v | FUTEX_WAITERS, SeqCst, SeqCst)
                .is_ok()
        {
            return None;
        }
    };
    if try_only {
        let _l = Locked::one(bucket(addr));
        return acquire().unwrap_or(-(EAGAIN as i64));
    }
    // Woken means the unlocker handed us ownership.
    table_wait(addr, BITSET_ANY, true, deadline, acquire)
}

fn unlock_pi(addr: u64) -> i64 {
    let Some(th) = thread::current() else {
        return -(EINVAL as i64);
    };
    let b = bucket(addr);
    let mut wakes = Wakes(Vec::new());
    {
        let _l = Locked::one(b);
        if word(addr).load(SeqCst) & FUTEX_TID_MASK != th.tid as u32 {
            return -(EPERM as i64);
        }
        // SAFETY: under the lock.
        unsafe { pi_handoff(b, addr, 0, 0, &mut wakes) };
    }
    wakes.run();
    0
}

// ---- shared (cross-process) futexes ------------------------------------------------

/// Whether a non-private futex at `addr` lives in a MAP_SHARED mapping
/// (inheritance VM_INHERIT_SHARE).
fn mapping_is_shared(addr: u64) -> bool {
    use crate::patch::vm;
    let (mut a, mut size, mut obj) = (addr, 0u64, 0);
    let mut info = [0i32; 9];
    let mut count = 9u32;
    // SAFETY: VM_REGION_BASIC_INFO_64 into a 9-int buffer.
    let kr = unsafe {
        vm::mach_vm_region(
            vm::task(),
            &mut a,
            &mut size,
            vm::VM_REGION_BASIC_INFO_64,
            info.as_mut_ptr(),
            &mut count,
            &mut obj,
        )
    };
    const VM_INHERIT_SHARE: i32 = 0;
    kr == 0 && a <= addr && info[2] == VM_INHERIT_SHARE
}

fn shared_wait(addr: u64, val: u32, deadline: Option<u64>) -> i64 {
    let th = thread::current();
    if word(addr).load(SeqCst) != val {
        return -(EAGAIN as i64);
    }
    loop {
        if th.is_some_and(signal::interrupted) {
            return -(EINTR as i64);
        }
        let now = park::monotonic();
        let slice = match deadline {
            Some(d) if now >= d => return -ETIMEDOUT,
            Some(d) => (d - now).min(SHARED_SLICE_NS),
            None => SHARED_SLICE_NS,
        };
        // SAFETY: waiting on a guest futex word in shared memory.
        let r = unsafe {
            __ulock_wait2(
                UL_COMPARE_AND_WAIT_SHARED | ULF_NO_ERRNO,
                addr as *mut _,
                val as u64,
                slice,
                0,
            )
        };
        match -r {
            _ if r >= 0 => return 0,
            libc::ETIMEDOUT | libc::EINTR => {
                if word(addr).load(SeqCst) != val {
                    return 0;
                }
            }
            libc::EFAULT => return -(EFAULT as i64),
            _ => return 0,
        }
    }
}

fn shared_wake(addr: u64, n: u32) -> i64 {
    if n == 0 {
        return 0;
    }
    let one = || {
        // SAFETY: waking waiters of a guest futex word.
        unsafe { __ulock_wake(UL_COMPARE_AND_WAIT_SHARED | ULF_NO_ERRNO, addr as *mut _, 0) == 0 }
    };
    if n > 64 {
        // SAFETY: as above. Darwin does not count waiters for wake-all.
        let r = unsafe {
            __ulock_wake(
                UL_COMPARE_AND_WAIT_SHARED | ULF_WAKE_ALL | ULF_NO_ERRNO,
                addr as *mut _,
                0,
            )
        };
        return (r == 0) as i64;
    }
    (0..n).take_while(|_| one()).count() as i64
}

/// A non-private wake of one waiter (CLONE_CHILD_CLEARTID and robust lists).
pub fn wake_one(addr: u64) {
    if mapping_is_shared(addr) {
        shared_wake(addr, 1);
    } else {
        table_wake(addr, 1, BITSET_ANY);
    }
}

// ---- robust lists -----------------------------------------------------------------

const ROBUST_LIST_LIMIT: usize = 2048;

/// Linux `handle_futex_death`: a lock the dying thread held gets
/// FUTEX_OWNER_DIED and a waiter is woken (a PI waiter gets ownership).
fn owner_died(uaddr: u64, tid: i32, pi: bool) {
    if uaddr & 3 != 0 {
        return;
    }
    let w = word(uaddr);
    let Ok(old) = w.fetch_update(SeqCst, SeqCst, |v| {
        (v & FUTEX_TID_MASK == tid as u32).then_some(v & FUTEX_WAITERS | FUTEX_OWNER_DIED)
    }) else {
        return;
    };
    if old & FUTEX_WAITERS == 0 {
        return;
    }
    if pi {
        let b = bucket(uaddr);
        let mut wakes = Wakes(Vec::new());
        {
            let _l = Locked::one(b);
            // SAFETY: under the lock.
            unsafe { pi_handoff(b, uaddr, FUTEX_OWNER_DIED, FUTEX_OWNER_DIED, &mut wakes) };
        }
        wakes.run();
    } else {
        wake_one(uaddr);
    }
}

/// Walk the exiting thread's robust list (struct robust_list_head { list,
/// futex_offset, list_op_pending }). Bit 0 of an entry marks a PI futex.
pub fn exit_robust_list(head: u64, tid: i32) {
    if head == 0 {
        return;
    }
    // SAFETY: the guest-registered robust list head and its entries.
    unsafe {
        let [list, off, pending] = (head as *const [u64; 3]).read_unaligned();
        let futex = |e: u64| (e & !1).wrapping_add(off);
        let mut e = list;
        for _ in 0..ROBUST_LIST_LIMIT {
            if e == head || e & !1 == 0 {
                break;
            }
            let next = ((e & !1) as *const u64).read_unaligned();
            if e != pending {
                owner_died(futex(e), tid, e & 1 != 0);
            }
            e = next;
        }
        if pending != 0 {
            owner_died(futex(pending), tid, pending & 1 != 0);
        }
    }
}

// ---- fork -------------------------------------------------------------------------

/// Hold every bucket lock across a fork (see `thread::fork_prepare`).
pub fn fork_lock() {
    for b in &BUCKETS {
        // SAFETY: static locks, always taken in this order when all are.
        unsafe { libc::os_unfair_lock_lock(b.lock.get()) };
    }
}

/// Release what [`fork_lock`] took. In the child the locks are reset
/// instead (an `os_unfair_lock` records its owner's thread port, which the
/// child's thread no longer has), and the waiters, other threads' stack
/// frames the child does not have, are dropped.
pub fn fork_unlock(child: bool) {
    for b in &BUCKETS {
        // SAFETY: this thread holds every bucket lock (`fork_lock`); in the
        // child it is the only thread.
        unsafe {
            if child {
                *b.head.get() = std::ptr::null_mut();
                *b.lock.get() = libc::OS_UNFAIR_LOCK_INIT;
            } else {
                libc::os_unfair_lock_unlock(b.lock.get());
            }
        }
    }
}

// ---- the syscall -------------------------------------------------------------------

fn deadline(ts: u64, relative: bool, realtime: bool) -> Result<Option<u64>, i64> {
    if ts == 0 {
        return Ok(None);
    }
    let ns = park::read_timespec(ts)?;
    Ok(Some(if relative {
        park::after(ns)
    } else {
        park::absolute(ns, realtime)
    }))
}

/// futex(uaddr, op, val, timeout | val2, uaddr2, val3).
pub fn futex(a: [u64; 6]) -> i64 {
    let (uaddr, op_full, val, ts, uaddr2, val3) =
        (a[0], a[1], a[2] as u32, a[3], a[4], a[5] as u32);
    let op = op_full & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
    let realtime = op_full & FUTEX_CLOCK_REALTIME != 0;
    if uaddr & 3 != 0 {
        return -(EINVAL as i64);
    }
    let shared = || op_full & FUTEX_PRIVATE_FLAG == 0 && mapping_is_shared(uaddr);
    match op {
        FUTEX_WAIT | FUTEX_WAIT_BITSET => {
            let bitset = if op == FUTEX_WAIT { BITSET_ANY } else { val3 };
            if bitset == 0 {
                return -(EINVAL as i64);
            }
            let d = match deadline(ts, op == FUTEX_WAIT, realtime) {
                Ok(d) => d,
                Err(e) => return e,
            };
            if shared() {
                // Darwin's shared wait has no bitset, so any wake of the
                // word wakes this waiter. That is a spurious wakeup, which
                // futex callers must tolerate (libfmq's EventFlag rechecks
                // its bits and waits again).
                shared_wait(uaddr, val, d)
            } else {
                let check = || (word(uaddr).load(SeqCst) != val).then_some(-(EAGAIN as i64));
                table_wait(uaddr, bitset, false, d, check)
            }
        }
        FUTEX_WAKE | FUTEX_WAKE_BITSET => {
            let bitset = if op == FUTEX_WAKE { BITSET_ANY } else { val3 };
            if bitset == 0 {
                return -(EINVAL as i64);
            }
            if shared() {
                // Waiters' bitsets are unknown here: a bitset wake wakes
                // them all, so the one whose bits match is among them.
                shared_wake(uaddr, if bitset == BITSET_ANY { val } else { u32::MAX })
            } else {
                table_wake(uaddr, val, bitset)
            }
        }
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            let (nwake, nreq) = (val as i32, ts as i32);
            if nwake < 0 || nreq < 0 || uaddr2 & 3 != 0 {
                return -(EINVAL as i64);
            }
            let cmp = (op == FUTEX_CMP_REQUEUE).then_some(val3);
            if shared() {
                // Darwin cannot move waiters: wake them all (spuriously).
                if cmp.is_some_and(|v| word(uaddr).load(SeqCst) != v) {
                    return -(EAGAIN as i64);
                }
                shared_wake(uaddr, u32::MAX)
            } else {
                table_requeue(uaddr, nwake as u32, nreq as u32, uaddr2, cmp)
            }
        }
        FUTEX_WAKE_OP => {
            if uaddr2 & 3 != 0 {
                return -(EINVAL as i64);
            }
            let old = match wake_op_apply(uaddr2, val3) {
                Ok(o) => o,
                Err(e) => return e,
            };
            let private = op_full & FUTEX_PRIVATE_FLAG != 0;
            let wake = |addr: u64, n: u32| {
                if !private && mapping_is_shared(addr) {
                    shared_wake(addr, n)
                } else {
                    table_wake(addr, n, BITSET_ANY)
                }
            };
            let mut n = wake(uaddr, val);
            if wake_op_cmp(old, val3) {
                n += wake(uaddr2, ts as u32);
            }
            n
        }
        FUTEX_LOCK_PI | FUTEX_LOCK_PI2 => {
            // LOCK_PI's timeout is absolute CLOCK_REALTIME; LOCK_PI2's
            // follows FUTEX_CLOCK_REALTIME.
            let rt = op == FUTEX_LOCK_PI || realtime;
            match deadline(ts, false, rt) {
                Ok(d) => lock_pi(uaddr, d, false),
                Err(e) => e,
            }
        }
        FUTEX_TRYLOCK_PI => lock_pi(uaddr, None, true),
        FUTEX_UNLOCK_PI => unlock_pi(uaddr),
        _ => {
            eprintln!("[linux-abi] futex op {op_full:#x} not implemented");
            -(ENOSYS as i64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_op_encoding_follows_linux() {
        // FUTEX_OP(FUTEX_OP_ADD, 1, FUTEX_OP_CMP_GT, 0), as glibc encodes it.
        let enc = (1 << 28) | (4 << 24) | (1 << 12);
        let mut w = 5u32;
        let old = wake_op_apply(&mut w as *mut u32 as u64, enc).unwrap();
        assert_eq!((old, w), (5, 6));
        assert!(wake_op_cmp(old, enc));
        // OPARG_SHIFT: OR in 1 << 3.
        let enc = (8 << 28) | (2 << 28) | (3 << 12);
        wake_op_apply(&mut w as *mut u32 as u64, enc).unwrap();
        assert_eq!(w, 6 | 8);
        // Sign-extended cmparg: -1 < 0.
        assert!(wake_op_cmp(u32::MAX, 2 << 24));
    }

    #[test]
    fn a_shared_bitset_wake_reaches_the_matching_waiter() {
        // libfmq's EventFlag: FUTEX_WAIT_BITSET / FUTEX_WAKE_BITSET on a
        // word in MAP_SHARED memory, with waiters on different bits. A wake
        // of one waiter on bit 2 must reach the waiter on bit 2.
        // SAFETY: a fresh anonymous shared mapping, unmapped after the joins.
        let word_addr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                16384,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANON,
                -1,
                0,
            )
        } as u64;
        let mut deadline = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: a local timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut deadline) };
        deadline.tv_sec += 2;
        let deadline = &*Box::leak(Box::new(deadline)) as *const libc::timespec as u64;
        let wait = move |bits: u64| {
            std::thread::spawn(move || futex([word_addr, FUTEX_WAIT_BITSET, 0, deadline, 0, bits]))
        };
        let (on_1, on_2) = (wait(1), wait(2));
        std::thread::sleep(std::time::Duration::from_millis(100));
        futex([word_addr, FUTEX_WAKE_BITSET, 1, 0, 0, 2]);
        assert_eq!(on_2.join().unwrap(), 0, "the waiter on bit 2 timed out");
        // The other waiter wakes spuriously, which futex users tolerate.
        assert_eq!(on_1.join().unwrap(), 0);
        // SAFETY: our mapping.
        unsafe { libc::munmap(word_addr as *mut _, 16384) };
    }

    #[test]
    fn mapping_flavour_follows_map_shared() {
        // SAFETY: fresh anonymous mappings.
        unsafe {
            let sh = libc::mmap(
                std::ptr::null_mut(),
                16384,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANON,
                -1,
                0,
            ) as u64;
            let pr = libc::mmap(
                std::ptr::null_mut(),
                16384,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            ) as u64;
            assert!(mapping_is_shared(sh + 64));
            assert!(!mapping_is_shared(pr + 64));
            libc::munmap(sh as *mut _, 16384);
            libc::munmap(pr as *mut _, 16384);
        }
    }
}
