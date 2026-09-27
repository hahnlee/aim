//! `binder_thread_write`: execute the BC command stream.

use crate::host::{Errno, GuestProcess, errno};
use crate::state::{
    DeathLocation, DeathState, LOOPER_ENTERED, LOOPER_EXITED, LOOPER_INVALID, LOOPER_REGISTERED,
    ProcId, State, Tid,
};
use crate::uapi::*;

impl State {
    /// Consume commands from `write` starting at `*consumed`. `*consumed` is
    /// advanced past each command once it has executed. Stops early while a
    /// return error is pending, as Linux does, so user space reads it first.
    pub(crate) fn thread_write(
        &mut self,
        proc: ProcId,
        tid: Tid,
        write: &[u8],
        consumed: &mut usize,
        guest: &mut dyn GuestProcess,
    ) -> Result<(), Errno> {
        while *consumed < write.len() {
            if self
                .thread(proc, tid)
                .is_none_or(|t| t.return_error.is_some())
            {
                break;
            }
            let rest = &write[*consumed..];
            let cmd = u32::from_le_bytes(rest.get(..4).ok_or(errno::EFAULT)?.try_into().unwrap());
            let size = command_payload_size(cmd).ok_or(errno::EINVAL)?;
            let payload = rest.get(4..4 + size).ok_or(errno::EFAULT)?;
            self.execute_command(proc, tid, cmd, payload, guest)?;
            *consumed += 4 + size;
        }
        Ok(())
    }

    fn execute_command(
        &mut self,
        proc: ProcId,
        tid: Tid,
        cmd: u32,
        payload: &[u8],
        guest: &mut dyn GuestProcess,
    ) -> Result<(), Errno> {
        match cmd {
            BC_INCREFS | BC_ACQUIRE | BC_RELEASE | BC_DECREFS => {
                let target = u32_at(payload, 0);
                let strong = cmd == BC_ACQUIRE || cmd == BC_RELEASE;
                let increment = cmd == BC_INCREFS || cmd == BC_ACQUIRE;
                let mut done = false;
                if increment && target == 0 {
                    let context = self.procs[&proc].context;
                    if let Some(mgr) = self.contexts[context].mgr_node {
                        if self.nodes[&mgr].proc == Some(proc) {
                            return Err(errno::EINVAL);
                        }
                        done = self.inc_ref_for_node(proc, mgr, strong, None).is_ok();
                    }
                }
                if !done {
                    // An invalid handle is a user error, not a failed ioctl.
                    let _ = self.update_ref_for_handle(proc, target, increment, strong);
                }
            }
            BC_INCREFS_DONE | BC_ACQUIRE_DONE => {
                let ptr = u64_at(payload, 0);
                let cookie = u64_at(payload, 8);
                let Some(&node) = self.procs[&proc].nodes.get(&ptr) else {
                    return Ok(());
                };
                let n = self.nodes.get_mut(&node).unwrap();
                if n.cookie != cookie {
                    return Ok(());
                }
                let strong = cmd == BC_ACQUIRE_DONE;
                let pending = if strong {
                    &mut n.pending_strong_ref
                } else {
                    &mut n.pending_weak_ref
                };
                if !*pending {
                    return Ok(());
                }
                *pending = false;
                self.dec_node(node, strong, false);
            }
            BC_ATTEMPT_ACQUIRE | BC_ACQUIRE_RESULT => return Err(errno::EINVAL),
            BC_FREE_BUFFER => {
                let ptr = u64_at(payload, 0);
                let offset = self
                    .procs
                    .get(&proc)
                    .and_then(|p| p.alloc.as_ref())
                    .and_then(|a| a.offset_of(ptr));
                let Some(offset) = offset else { return Ok(()) };
                if !self.buffer_mut(proc, offset).unwrap().allow_user_free {
                    return Ok(());
                }
                self.free_buffer(proc, offset, false, Some(guest));
            }
            BC_TRANSACTION | BC_REPLY => {
                let tr = TransactionData::decode(payload);
                self.transaction(proc, tid, &tr, cmd == BC_REPLY, 0, guest);
            }
            BC_TRANSACTION_SG | BC_REPLY_SG => {
                let tr = TransactionData::decode(&payload[..TRANSACTION_DATA_SIZE]);
                let buffers_size = u64_at(payload, TRANSACTION_DATA_SIZE);
                self.transaction(proc, tid, &tr, cmd == BC_REPLY_SG, buffers_size, guest);
            }
            BC_REGISTER_LOOPER => {
                let p = self.procs.get_mut(&proc).unwrap();
                let t = p.threads.get_mut(&tid).unwrap();
                if t.looper & LOOPER_ENTERED != 0 || p.requested_threads == 0 {
                    t.looper |= LOOPER_INVALID;
                } else {
                    p.requested_threads -= 1;
                    p.requested_threads_started += 1;
                }
                t.looper |= LOOPER_REGISTERED;
            }
            BC_ENTER_LOOPER => {
                let t = self.thread(proc, tid).unwrap();
                if t.looper & LOOPER_REGISTERED != 0 {
                    t.looper |= LOOPER_INVALID;
                }
                t.looper |= LOOPER_ENTERED;
            }
            BC_EXIT_LOOPER => {
                self.thread(proc, tid).unwrap().looper |= LOOPER_EXITED;
            }
            BC_REQUEST_DEATH_NOTIFICATION | BC_CLEAR_DEATH_NOTIFICATION => {
                let handle = u32_at(payload, 0);
                let cookie = u64_at(payload, 4);
                self.death_command(
                    proc,
                    tid,
                    cmd == BC_REQUEST_DEATH_NOTIFICATION,
                    handle,
                    cookie,
                );
            }
            BC_DEAD_BINDER_DONE => {
                let cookie = u64_at(payload, 0);
                let found = self.procs[&proc]
                    .delivered_death
                    .iter()
                    .copied()
                    .find(|d| self.deaths[d].cookie == cookie);
                let Some(death) = found else { return Ok(()) };
                self.dequeue_death_work(death);
                let d = self.deaths.get_mut(&death).unwrap();
                if d.state == DeathState::DeadBinderAndClear {
                    d.state = DeathState::Clear;
                    self.queue_death_for_thread_or_proc(death, proc, tid);
                }
            }
            // No freezer on this device: freeze notifications are refused.
            BC_REQUEST_FREEZE_NOTIFICATION
            | BC_CLEAR_FREEZE_NOTIFICATION
            | BC_FREEZE_NOTIFICATION_DONE => return Err(errno::EINVAL),
            _ => return Err(errno::EINVAL),
        }
        Ok(())
    }

    fn death_command(&mut self, proc: ProcId, tid: Tid, request: bool, handle: u32, cookie: u64) {
        let Some(r) = self.procs[&proc].refs_by_desc.get(&handle) else {
            return;
        };
        let (node, existing) = (r.node, r.death);
        if request {
            if existing.is_some() {
                return;
            }
            let id = self.next_id();
            let node_dead = self.nodes.get(&node).is_none_or(|n| n.proc.is_none());
            self.deaths.insert(
                id,
                crate::state::Death {
                    proc,
                    cookie,
                    state: DeathState::Armed,
                    location: None,
                },
            );
            self.procs
                .get_mut(&proc)
                .unwrap()
                .refs_by_desc
                .get_mut(&handle)
                .unwrap()
                .death = Some(id);
            if node_dead {
                let d = self.deaths.get_mut(&id).unwrap();
                d.state = DeathState::DeadBinder;
                d.location = Some(DeathLocation::Queued(crate::state::WorkList::Proc(proc)));
                self.enqueue_proc_work(proc, crate::state::Work::Death(id));
                self.wakeup_proc(proc);
            }
        } else {
            let Some(id) = existing else { return };
            if self.deaths[&id].cookie != cookie {
                return;
            }
            self.procs
                .get_mut(&proc)
                .unwrap()
                .refs_by_desc
                .get_mut(&handle)
                .unwrap()
                .death = None;
            let d = self.deaths.get_mut(&id).unwrap();
            if d.location.is_none() {
                d.state = DeathState::Clear;
                self.queue_death_for_thread_or_proc(id, proc, tid);
            } else {
                // Still queued or delivered as DEAD_BINDER: finish with a
                // CLEAR_DEATH_NOTIFICATION_DONE after BC_DEAD_BINDER_DONE.
                d.state = DeathState::DeadBinderAndClear;
            }
        }
    }
}
