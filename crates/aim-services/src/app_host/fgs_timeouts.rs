//! Foreground-service deadlines. One cancellable worker, with generations checked
//! again by the service owner when its handler receives a due event.
use super::AppHost;
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception};
use std::{
    sync::{Mutex, Weak, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Event {
    Required,
    Short,
    Demote,
    ShortAnr,
    Limited,
    LimitedCrash,
    StartAnr { seq: i64, reason: String },
}

pub(super) struct Deadline {
    pub token: Binder,
    pub generation: u64,
    pub event: Event,
    pub at: Instant,
}

enum Command {
    Schedule(Deadline),
    Cancel(Binder),
}

pub(super) struct Timers {
    sender: Option<mpsc::Sender<Command>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Timers {
    pub fn new(host: Weak<AppHost>) -> Self {
        Self::with_dispatch(move |deadline| {
            let Some(host) = host.upgrade() else {
                return false;
            };
            host.post_job(move |host| host.foreground_service_event(deadline));
            true
        })
    }

    fn with_dispatch(mut dispatch: impl FnMut(Deadline) -> bool + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut pending: Vec<Deadline> = Vec::new();
            loop {
                let next = pending
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, d)| d.at)
                    .map(|(index, d)| (index, d.at));
                let command = match next {
                    Some((index, at)) if at <= Instant::now() => {
                        let deadline = pending.remove(index);
                        if !dispatch(deadline) {
                            break;
                        }
                        continue;
                    }
                    Some((_, at)) => {
                        match receiver.recv_timeout(at.saturating_duration_since(Instant::now())) {
                            Ok(command) => command,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                    None => match receiver.recv() {
                        Ok(command) => command,
                        Err(_) => break,
                    },
                };
                match command {
                    // An accepted start-foreground ANR belongs to the process,
                    // even if its service stops or is promoted in the meantime.
                    Command::Cancel(token) => pending
                        .retain(|d| d.token != token || matches!(d.event, Event::StartAnr { .. })),
                    Command::Schedule(deadline) => {
                        pending.retain(|d| {
                            d.token != deadline.token
                                || std::mem::discriminant(&d.event)
                                    != std::mem::discriminant(&deadline.event)
                        });
                        pending.push(deadline);
                    }
                }
            }
        });
        Self {
            sender: Some(sender),
            worker: Mutex::new(Some(worker)),
        }
    }

    pub fn schedule(&self, deadline: Deadline) {
        self.sender
            .as_ref()
            .unwrap()
            .send(Command::Schedule(deadline))
            .expect("FGS timer worker");
    }

    pub fn cancel(&self, token: Binder) {
        self.sender
            .as_ref()
            .unwrap()
            .send(Command::Cancel(token))
            .expect("FGS timer worker");
    }
}

impl Drop for Timers {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.lock().unwrap().take()
            && worker.thread().id() != thread::current().id()
        {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Policy {
    pub short: Duration,
    pub demote: Duration,
    pub anr: Duration,
    pub limited_type: i32,
    pub limit_ms: i64,
    pub crash: Duration,
    pub crash_enabled: bool,
    pub start_anr: Duration,
    pub required: Duration,
}

impl Policy {
    pub fn read(values: Option<Vec<i64>>) -> Result<Self, Exception> {
        let bad = || Exception::new(EX_ILLEGAL_STATE, "invalid Java FGS timeout policy");
        let values = values.ok_or_else(bad)?;
        if values.len() != 9 || values.iter().any(|v| *v < 0) {
            return Err(bad());
        }
        if !matches!(
            values[3] as i32,
            0 | super::fgs_types::TYPE_DATA_SYNC | super::fgs_types::TYPE_MEDIA_PROCESSING
        ) || !matches!(values[6], 0 | 1)
        {
            return Err(bad());
        }
        let duration = |index| Duration::from_millis(values[index] as u64);
        Ok(Self {
            short: duration(0),
            demote: duration(1),
            anr: duration(2),
            limited_type: i32::try_from(values[3]).map_err(|_| bad())?,
            limit_ms: values[4],
            crash: duration(5),
            crash_enabled: values[6] != 0,
            start_anr: duration(7),
            required: duration(8),
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct Short {
    pub generation: u64,
    pub start_id: i32,
    pub timeout: Instant,
    pub demote: Instant,
    pub anr: Instant,
    pub demoted: bool,
    pub notified: bool,
}

impl Short {
    pub fn new(
        generation: u64,
        start_id: i32,
        policy: &Policy,
        now: Instant,
    ) -> Result<Self, Exception> {
        let overflow = || Exception::new(EX_ILLEGAL_STATE, "FGS timeout exceeds clock range");
        let timeout = now.checked_add(policy.short).ok_or_else(overflow)?;
        Ok(Self {
            generation,
            start_id,
            timeout,
            demote: timeout.checked_add(policy.demote).ok_or_else(overflow)?,
            anr: timeout.checked_add(policy.anr).ok_or_else(overflow)?,
            demoted: false,
            notified: false,
        })
    }

    pub fn due(&self, generation: u64, event: &Event, now: Instant) -> bool {
        let at = match event {
            Event::Short if !self.notified => self.timeout,
            Event::Demote if self.notified && !self.demoted => self.demote,
            Event::ShortAnr if self.notified => self.anr,
            _ => return false,
        };
        self.generation == generation && now >= at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_accepted_start_foreground_anr_survives_service_cancellation() {
        let (send, receive) = mpsc::channel();
        let timers = Timers::with_dispatch(move |deadline| send.send(deadline).is_ok());
        let token = Binder::Local(2);
        timers.schedule(Deadline {
            token,
            generation: 1,
            event: Event::StartAnr {
                seq: 37,
                reason: "did not call startForeground".into(),
            },
            at: Instant::now() + Duration::from_millis(10),
        });
        timers.cancel(token);
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(2)).unwrap().event,
            Event::StartAnr { seq: 37, .. }
        ));
    }

    #[test]
    fn canceled_and_replaced_deadlines_do_not_fire_and_drop_joins_the_worker() {
        let (send, receive) = mpsc::channel();
        let timers = Timers::with_dispatch(move |deadline| send.send(deadline).is_ok());
        let later = Instant::now() + Duration::from_secs(3600);
        let token = Binder::Local(1);
        timers.schedule(Deadline {
            token,
            generation: 1,
            event: Event::Short,
            at: later,
        });
        timers.cancel(token);
        timers.schedule(Deadline {
            token,
            generation: 2,
            event: Event::Short,
            at: later,
        });
        timers.schedule(Deadline {
            token,
            generation: 3,
            event: Event::Short,
            at: Instant::now(),
        });
        let fired = receive.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(fired.generation, 3);
        // This would wait an hour if shutdown kept a sleeping timer thread alive.
        drop(timers);
        assert!(matches!(
            receive.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }
}
