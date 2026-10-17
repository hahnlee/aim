//! Audio focus (`MediaFocusControl`, `FocusRequester` at
//! `android-16.0.0_r1`): the focus stack, its top the owner. A request
//! takes the top, and each entry below loses focus as the request's gain
//! makes it (a definitive loss takes it off the stack); a locked owner
//! (`AUDIOFOCUS_FLAG_LOCK`, or the call's client) keeps it, and a request
//! that may wait goes below it, delayed. Abandoning the top, or its
//! holder dying, gives the next entry its focus back. A loss is
//! dispatched to its owner's `IAudioFocusDispatcher` unless the framework
//! handles it on the loser's players ([`Playback`]): a duck from another
//! app, which an app of an SDK above N MR1 that does not pause on ducks
//! does not hear, and a loss for good, after the loser's players are
//! faded out. The call's client mutes media and game players while it
//! holds focus.
//!
//! No external focus policy or focus follower registers: the app host has
//! no audio policies (`registerAudioPolicy`), so the framework decides
//! focus and notifies owners of ducks. Multiple audio focus
//! (`MULTI_AUDIO_FOCUS_ENABLED`) is not kept, and a test's focus freeze
//! never starts (`enterAudioFocusFreezeForTest`); these and the focus
//! test APIs are #1528.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{BTreeMap, BTreeSet};

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_media_iaudiofocusdispatcher as dispatcher;

use super::playback::{self, Attributes, Playback};

/// `AudioManager.AUDIOFOCUS_*`.
pub const AUDIOFOCUS_NONE: i32 = 0;
pub const AUDIOFOCUS_GAIN: i32 = 1;
pub const AUDIOFOCUS_GAIN_TRANSIENT: i32 = 2;
pub const AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK: i32 = 3;
pub const AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE: i32 = 4;
pub const AUDIOFOCUS_LOSS: i32 = -1;
pub const AUDIOFOCUS_LOSS_TRANSIENT: i32 = -2;
pub const AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK: i32 = -3;
pub const AUDIOFOCUS_REQUEST_FAILED: i32 = 0;
pub const AUDIOFOCUS_REQUEST_GRANTED: i32 = 1;
pub const AUDIOFOCUS_REQUEST_DELAYED: i32 = 2;
pub const AUDIOFOCUS_FLAG_DELAY_OK: i32 = 1 << 0;
pub const AUDIOFOCUS_FLAG_LOCK: i32 = 1 << 2;
pub const AUDIOFOCUS_FLAG_TEST: i32 = 1 << 3;
/// `AudioSystem.IN_VOICE_COMM_FOCUS_ID`: the call's client.
pub const IN_VOICE_COMM_FOCUS_ID: &str = "AudioFocus_For_Phone_Ring_And_Calls";
/// `MediaFocusControl.MAX_STACK_SIZE`.
const MAX_STACK_SIZE: usize = 100;
/// `DUCKING_IN_APP_SDK_LEVEL`: `Build.VERSION_CODES.N_MR1`.
const DUCKING_IN_APP_SDK_LEVEL: i32 = 25;
/// `RING_CALL_MUTING_ENFORCEMENT_DELAY_MS`.
const RING_CALL_MUTING_ENFORCEMENT_DELAY_MS: u64 = 100;
/// `AudioAttributes.USAGE_ASSISTANT`.
const USAGE_ASSISTANT: i32 = 16;

/// A focus owner's `IAudioFocusDispatcher`.
pub trait Dispatcher {
    /// `dispatchAudioFocusChange` (one-way): whether it was sent.
    fn dispatch(&self, change: i32, client_id: &str) -> bool;
}

impl Dispatcher for Strong {
    fn dispatch(&self, change: i32, client_id: &str) -> bool {
        let mut p = Parcel::new();
        dispatcher::DispatchAudioFocusChange { focus_change: change, client_id: Some(client_id.to_string()) }.write(&mut p);
        self.transact(dispatcher::DISPATCH_AUDIO_FOCUS_CHANGE, &p, true).is_ok()
    }
}

/// `getFocusRampTimeMs`: of the request's public usage.
pub fn ramp_time_ms(attributes: &Attributes) -> i32 {
    match attributes.public_usage() {
        1 | 14 => 1000,
        4 | 6 | 16 | 11 | 12 => 700,
        2 | 3 | 5 | 7 | 8 | 9 | 10 | 13 => 500,
        _ => 0,
    }
}

/// A focus request (`requestAudioFocus`'s arguments).
pub struct Request<D> {
    pub attributes: Attributes,
    pub gain: i32,
    /// The client's token (`cb`), by handle.
    pub source: u32,
    pub dispatcher: Option<D>,
    pub client_id: String,
    pub package: String,
    pub flags: i32,
    pub sdk: i32,
    pub force_duck: bool,
    pub uid: i32,
}

/// `FocusRequester`: a request, with the link to its token's death until
/// released.
struct Requester<D, L> {
    request: Request<D>,
    link: Option<L>,
    /// `mFocusLossReceived`, `mFocusLossWasNotified`.
    loss: i32,
    loss_notified: bool,
    /// `mFocusLossFadeLimbo`: the fade's token, while its players fade
    /// out before its loss is dispatched.
    limbo: Option<u64>,
}

impl<D: Dispatcher, L> Requester<D, L> {
    fn same_client(&self, client_id: &str) -> bool {
        self.request.client_id == client_id
    }

    /// `MediaFocusControl.isLockedFocusOwner`.
    fn locked(&self) -> bool {
        self.same_client(IN_VOICE_COMM_FOCUS_ID) || self.request.flags & AUDIOFOCUS_FLAG_LOCK != 0
    }

    /// `focusLossForGainRequest`: the loss a gain causes, given the loss
    /// already received.
    fn loss_for_gain(&self, gain: i32) -> i32 {
        let loss = self.loss;
        match gain {
            AUDIOFOCUS_GAIN => AUDIOFOCUS_LOSS,
            AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE | AUDIOFOCUS_GAIN_TRANSIENT if loss == AUDIOFOCUS_LOSS => AUDIOFOCUS_LOSS,
            AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE | AUDIOFOCUS_GAIN_TRANSIENT => AUDIOFOCUS_LOSS_TRANSIENT,
            AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK => match loss {
                AUDIOFOCUS_LOSS_TRANSIENT | AUDIOFOCUS_LOSS => loss,
                _ => AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK,
            },
            _ => AUDIOFOCUS_NONE,
        }
    }

    /// A focus change to its dispatcher, if it still has one: false if
    /// sending failed.
    fn dispatch(&self, change: i32, reason: &str) -> bool {
        let id = &self.request.client_id;
        match &self.request.dispatcher {
            Some(d) if !d.dispatch(change, id) => {
                eprintln!("app host: focus {change} to {id} ({reason}) failed");
                false
            }
            Some(_) => true,
            None => {
                eprintln!("app host: focus {change} to {id} ({reason}): no listener");
                true
            }
        }
    }

    /// `AudioFocusInfo` of it.
    fn info(&self) -> FocusInfo {
        let r = &self.request;
        FocusInfo {
            attributes: r.attributes.clone(),
            uid: r.uid,
            client_id: r.client_id.clone(),
            package: r.package.clone(),
            gain: r.gain,
            loss: self.loss,
            flags: r.flags,
            sdk: r.sdk,
        }
    }
}

/// `android.media.AudioFocusInfo`; its generation count is that of an
/// external policy's request, so 0.
pub struct FocusInfo {
    attributes: Attributes,
    uid: i32,
    client_id: String,
    package: String,
    gain: i32,
    loss: i32,
    flags: i32,
    sdk: i32,
}

impl WriteParcelable for FocusInfo {
    fn write_to(&self, p: &mut Parcel) {
        self.attributes.write_to(p);
        p.write_i32(self.uid);
        p.write_string16(Some(&self.client_id));
        p.write_string16(Some(&self.package));
        p.write_i32(self.gain);
        p.write_i32(self.loss);
        p.write_i32(self.flags);
        p.write_i32(self.sdk);
        p.write_i64(0);
    }
}

/// Work focus asks to run later (`mFocusHandler`'s messages, the ring or
/// call checker's thread), after its delay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Timer {
    /// `MSG_L_FOCUS_LOSS_AFTER_FADE` of the loser in limbo by its token.
    LossAfterFade { token: u64, delay_ms: u64 },
    /// `MSL_L_FORGET_UID`.
    ForgetUid { uid: i32, token: u64, delay_ms: u64 },
    /// `runAudioCheckerForRingOrCallAsync`.
    RingOrCall { delay_ms: u64 },
}

impl Timer {
    pub fn delay_ms(&self) -> u64 {
        match *self {
            Timer::LossAfterFade { delay_ms, .. } | Timer::ForgetUid { delay_ms, .. } | Timer::RingOrCall { delay_ms } => delay_ms,
        }
    }
}

/// What a loss asks of its owner beyond the stack: the timers and their
/// tokens.
#[derive(Default)]
struct Effects {
    timers: Vec<Timer>,
    next_token: u64,
}

impl Effects {
    fn token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }
}

/// `MediaFocusControl`, over dispatchers `D` and death links `L`.
pub struct Focus<D, L> {
    /// `mFocusStack`, its top last.
    stack: Vec<Requester<D, L>>,
    /// `mRingOrCallActive`.
    ring_or_call: bool,
    /// The losers in limbo, off the stack, by their fade's token.
    limbo: BTreeMap<u64, Requester<D, L>>,
    /// The uids to forget later, with their timers' tokens
    /// (`removeEqualMessages` drops them).
    forgets: BTreeSet<(i32, u64)>,
    /// The death links of released requesters, for the owner to clear.
    released: Vec<L>,
    fx: Effects,
}

impl<D, L> Default for Focus<D, L> {
    fn default() -> Self {
        Focus { stack: Vec::new(), ring_or_call: false, limbo: BTreeMap::new(), forgets: BTreeSet::new(), released: Vec::new(), fx: Effects::default() }
    }
}

/// `FocusRequester.handleFocusLoss`: `r` loses `loss` to `winner`, which
/// the framework may handle on the players instead of dispatching it.
fn handle_loss<D: Dispatcher, L>(r: &mut Requester<D, L>, loss: i32, winner: Option<&Request<D>>, force_duck: bool, pb: &mut Playback, fx: &mut Effects) {
    if loss == r.loss {
        return;
    }
    r.loss = loss;
    r.loss_notified = false;
    if winner.is_some_and(|w| framework_handles_loss(r, loss, w, force_duck, pb, fx)) {
        return;
    }
    if r.request.dispatcher.is_some() {
        r.loss_notified = true;
    }
    r.dispatch(loss, "handleLoss");
}

/// `frameworkHandleFocusLoss`: another app's duck ducks the loser's
/// players (unless it pauses on ducks or targets N MR1 or below, and not
/// when forced), its gain for good fades them out first and dispatches
/// the loss after the fade.
fn framework_handles_loss<D, L>(r: &mut Requester<D, L>, loss: i32, winner: &Request<D>, force_duck: bool, pb: &mut Playback, fx: &mut Effects) -> bool {
    let loser = &r.request;
    if winner.uid == loser.uid {
        return false;
    }
    if loss == AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK {
        if !force_duck && (loser.flags & playback::AUDIOFOCUS_FLAG_PAUSES_ON_DUCKABLE_LOSS != 0 || loser.sdk <= DUCKING_IN_APP_SDK_LEVEL) {
            return false;
        }
        // `reqCausesStrongDuck`.
        let strong = winner.gain == AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK && winner.attributes.public_usage() == USAGE_ASSISTANT;
        return pb.duck_players(winner.uid, loser.uid, strong, force_duck);
    }
    if loss == AUDIOFOCUS_LOSS && pb.fade_out_players(winner.uid, winner.attributes.content_type(), loser.uid, loser.flags) {
        let token = fx.token();
        r.limbo = Some(token);
        fx.timers.push(Timer::LossAfterFade { token, delay_ms: playback::fade_out_duration_ms(&loser.attributes) });
        return true;
    }
    false
}

impl<D: Dispatcher, L> Focus<D, L> {
    /// `FocusRequester.release`: its dispatcher and death link go.
    fn release(&mut self, mut r: Requester<D, L>) {
        r.request.dispatcher = None;
        if let Some(link) = r.link.take() {
            self.released.push(link);
        }
    }

    /// `maybeRelease`: one in limbo stays until its loss is dispatched.
    fn maybe_release(&mut self, r: Requester<D, L>) {
        match r.limbo {
            Some(token) => {
                self.limbo.insert(token, r);
            }
            None => self.release(r),
        }
    }

    /// `restoreVShapedPlayers` of a winner, whose forgetting is then due no
    /// more.
    fn restore(&mut self, uid: i32, pb: &mut Playback) {
        pb.restore(uid);
        self.forgets.retain(|(u, _)| *u != uid);
    }

    /// `handleFocusGain` of the top: its players are restored unless
    /// telling it failed.
    fn gain_top(&mut self, gain: i32, pb: &mut Playback) {
        let Some(top) = self.stack.last_mut() else { return };
        top.loss = AUDIOFOCUS_NONE;
        top.limbo = None;
        if top.loss_notified && !top.dispatch(gain, "handleGain") {
            return;
        }
        let uid = top.request.uid;
        self.restore(uid, pb);
    }

    /// `canReassignAudioFocus`.
    fn can_reassign(&self) -> bool {
        !self.stack.last().is_some_and(Requester::locked)
    }

    /// `notifyTopOfAudioFocusStack`.
    fn notify_top(&mut self, pb: &mut Playback) {
        if self.can_reassign() {
            self.gain_top(AUDIOFOCUS_GAIN, pb);
        }
    }

    /// `removeFocusStackEntry`: the top if it is the client's (then the
    /// next gains focus if `signal`), else each entry of the client.
    fn remove_entry(&mut self, client_id: &str, signal: bool, pb: &mut Playback) {
        if self.stack.last().is_some_and(|t| t.same_client(client_id)) {
            let r = self.stack.pop().unwrap();
            self.maybe_release(r);
            if signal {
                self.notify_top(pb);
            }
            return;
        }
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.stack).into_iter().partition(|r| r.same_client(client_id));
        self.stack = kept;
        for r in gone {
            self.maybe_release(r);
        }
    }

    /// `propagateFocusLossFromGain_syncAf`: each entry loses focus to
    /// `winner`, and the definitive losers leave the stack.
    fn propagate_loss(&mut self, winner: &Request<D>, below: usize, force_duck: bool, pb: &mut Playback) {
        let mut gone = Vec::new();
        for r in &mut self.stack[..below] {
            let loss = r.loss_for_gain(winner.gain);
            handle_loss(r, loss, Some(winner), force_duck, pb, &mut self.fx);
            if loss == AUDIOFOCUS_LOSS {
                gone.push(r.request.client_id.clone());
            }
        }
        for client_id in gone {
            self.remove_entry(&client_id, false, pb);
        }
    }

    /// `requestAudioFocus` of a live client, once AudioService's checks
    /// passed; `link` links to its token's death.
    pub fn request(&mut self, request: Request<D>, link: impl FnOnce() -> L, pb: &mut Playback) -> i32 {
        if self.stack.len() > MAX_STACK_SIZE {
            return AUDIOFOCUS_REQUEST_FAILED;
        }
        let entering_ring_or_call = !self.ring_or_call && request.client_id == IN_VOICE_COMM_FOCUS_ID;
        if entering_ring_or_call {
            self.ring_or_call = true;
        }
        let mut delayed = false;
        if !self.can_reassign() {
            if request.flags & AUDIOFOCUS_FLAG_DELAY_OK == 0 {
                return AUDIOFOCUS_REQUEST_FAILED;
            }
            delayed = true;
        }
        let link = link();
        if let Some(top) = self.stack.last()
            && top.same_client(&request.client_id)
        {
            // The same request again is granted as it is.
            if top.request.gain == request.gain && top.request.flags == request.flags {
                self.released.push(link);
                return AUDIOFOCUS_REQUEST_GRANTED;
            }
            if !delayed {
                let r = self.stack.pop().unwrap();
                self.release(r);
            }
        }
        self.remove_entry(&request.client_id.clone(), false, pb);
        let uid = request.uid;
        let force_duck = request.force_duck;
        let new = Requester { request, link: Some(link), loss: AUDIOFOCUS_NONE, loss_notified: true, limbo: None };
        if delayed {
            return self.push_below_locked(new, pb);
        }
        let len = self.stack.len();
        self.propagate_loss(&new.request, len, force_duck, pb);
        self.stack.push(new);
        // `handleFocusGainFromRequest`.
        self.restore(uid, pb);
        if entering_ring_or_call {
            self.fx.timers.push(Timer::RingOrCall { delay_ms: RING_CALL_MUTING_ENFORCEMENT_DELAY_MS });
        }
        AUDIOFOCUS_REQUEST_GRANTED
    }

    /// `pushBelowLockedFocusOwnersAndPropagate`: below the lowest locked
    /// owner, the entries under it losing focus to it.
    fn push_below_locked(&mut self, new: Requester<D, L>, pb: &mut Playback) -> i32 {
        let Some(at) = self.stack.iter().position(Requester::locked) else {
            let len = self.stack.len();
            self.propagate_loss(&new.request, len, false, pb);
            self.stack.push(new);
            return AUDIOFOCUS_REQUEST_GRANTED;
        };
        self.stack.insert(at, new);
        let (below, rest) = self.stack.split_at_mut(at);
        let winner = &rest[0].request;
        let mut gone = Vec::new();
        for r in below.iter_mut().rev() {
            let loss = r.loss_for_gain(winner.gain);
            handle_loss(r, loss, Some(winner), false, pb, &mut self.fx);
            if loss == AUDIOFOCUS_LOSS {
                gone.push(r.request.client_id.clone());
            }
        }
        for client_id in gone {
            self.remove_entry(&client_id, false, pb);
        }
        AUDIOFOCUS_REQUEST_DELAYED
    }

    /// `abandonAudioFocus`: the client leaves the stack, and the next
    /// entry gains focus if it held it.
    pub fn abandon(&mut self, client_id: &str, pb: &mut Playback) -> i32 {
        let exiting_ring_or_call = self.ring_or_call && client_id == IN_VOICE_COMM_FOCUS_ID;
        if exiting_ring_or_call {
            self.ring_or_call = false;
        }
        self.remove_entry(client_id, true, pb);
        if exiting_ring_or_call {
            self.fx.timers.push(Timer::RingOrCall { delay_ms: 0 });
        }
        AUDIOFOCUS_REQUEST_GRANTED
    }

    /// `unregisterAudioFocusClient`.
    pub fn unregister(&mut self, client_id: &str, pb: &mut Playback) {
        self.remove_entry(client_id, false, pb);
    }

    /// `removeFocusStackEntryOnDeath`: the entries of a dead token.
    pub fn died(&mut self, source: u32, pb: &mut Playback) {
        let top = self.stack.last().is_some_and(|t| t.request.source == source);
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.stack).into_iter().partition(|r| r.request.source == source);
        self.stack = kept;
        for r in gone {
            self.release(r);
        }
        if top {
            self.notify_top(pb);
        }
    }

    /// Runs a timer that came due.
    pub fn fire(&mut self, timer: Timer, pb: &mut Playback) {
        match timer {
            Timer::LossAfterFade { token, .. } => {
                let Some(loser) = self.limbo.remove(&token) else { return };
                loser.dispatch(AUDIOFOCUS_LOSS, "loss after fade");
                // `postForgetUidLater`.
                let uid = loser.request.uid;
                let forget = self.fx.token();
                self.forgets.insert((uid, forget));
                self.fx.timers.push(Timer::ForgetUid { uid, token: forget, delay_ms: playback::FADE_IN_DELAY_FOR_OFFENDERS_MS });
                self.release(loser);
            }
            Timer::ForgetUid { uid, token, .. } => {
                if self.forgets.remove(&(uid, token)) {
                    pb.forget_uid(uid);
                }
            }
            Timer::RingOrCall { .. } => {
                if self.ring_or_call {
                    pb.mute_for_call();
                } else {
                    pb.unmute_for_call();
                }
            }
        }
    }

    /// `hasAudioFocusUsers`, negated.
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// `getCurrentAudioFocus`: the top's gain.
    pub fn current(&self) -> i32 {
        self.stack.last().map_or(AUDIOFOCUS_NONE, |t| t.request.gain)
    }

    /// `getFocusStack`, its top last.
    pub fn infos(&self) -> Vec<Option<FocusInfo>> {
        self.stack.iter().map(|r| Some(r.info())).collect()
    }

    /// The death links released since, to clear.
    pub fn take_released(&mut self) -> Vec<L> {
        std::mem::take(&mut self.released)
    }

    /// The timers asked for since, to schedule.
    pub fn take_timers(&mut self) -> Vec<Timer> {
        std::mem::take(&mut self.fx.timers)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use aim_binder_host::parcel::Reader;
    use aim_service_aidl::ReadParcelable;

    use super::*;
    use crate::app_host::playback::{Operation, PlayerCall, PlayerIdCard};

    /// The focus changes dispatched, to whom.
    type Log = Rc<RefCell<Vec<(String, i32)>>>;

    struct Recorder(Log);

    impl Dispatcher for Recorder {
        fn dispatch(&self, change: i32, client_id: &str) -> bool {
            self.0.borrow_mut().push((client_id.to_string(), change));
            true
        }
    }

    struct Stack {
        focus: Focus<Recorder, u32>,
        pb: Playback,
        log: Log,
    }

    impl Stack {
        fn new() -> Self {
            Stack { focus: Focus::default(), pb: Playback::default(), log: Log::default() }
        }

        /// A media request of client `c<client>` (its token `client`) by
        /// `uid`.
        fn request(&mut self, client: u32, uid: i32, gain: i32, flags: i32, sdk: i32) -> i32 {
            let request = Request {
                attributes: Attributes::built(playback::USAGE_MEDIA, 2, 0),
                gain,
                source: client,
                dispatcher: Some(Recorder(self.log.clone())),
                client_id: format!("c{client}"),
                package: format!("p{uid}"),
                flags,
                sdk,
                force_duck: false,
                uid,
            };
            self.focus.request(request, || client, &mut self.pb)
        }

        fn log(&self) -> Vec<(String, i32)> {
            std::mem::take(&mut *self.log.borrow_mut())
        }

        /// A started media player of `uid`.
        fn play(&mut self, piid: i32, uid: i32) {
            let card = PlayerIdCard { player_type: 2, attributes: Attributes::built(playback::USAGE_MEDIA, 2, 0), player: Some(piid as u32), session: 9 };
            self.pb.track(piid, card, uid, 77, None);
            self.pb.event(piid, 2, &[], uid);
        }

        fn fire_all(&mut self) {
            loop {
                let timers = self.focus.take_timers();
                if timers.is_empty() {
                    return;
                }
                for timer in timers {
                    self.focus.fire(timer, &mut self.pb);
                }
            }
        }
    }

    const APP: i32 = 10_154;
    const OTHER: i32 = 10_155;

    fn to(client: &str, change: i32) -> (String, i32) {
        (client.to_string(), change)
    }

    #[test]
    fn a_gain_takes_focus_for_good() {
        let mut s = Stack::new();
        assert_eq!(s.request(1, APP, AUDIOFOCUS_GAIN, 0, 36), AUDIOFOCUS_REQUEST_GRANTED);
        assert_eq!(s.request(2, OTHER, AUDIOFOCUS_GAIN, 0, 36), AUDIOFOCUS_REQUEST_GRANTED);
        assert_eq!(s.log(), [to("c1", AUDIOFOCUS_LOSS)]);
        assert_eq!(s.focus.stack.len(), 1);
        assert_eq!(s.focus.take_released(), [1]);
        // The same request again is granted as it stands.
        assert_eq!(s.request(2, OTHER, AUDIOFOCUS_GAIN, 0, 36), AUDIOFOCUS_REQUEST_GRANTED);
        assert_eq!(s.focus.take_released(), [2]);
        assert_eq!(s.focus.current(), AUDIOFOCUS_GAIN);
        s.focus.abandon("c2", &mut s.pb);
        assert!(s.focus.is_empty());
        assert!(s.log().is_empty());
    }

    #[test]
    fn a_transient_loss_gets_focus_back() {
        let mut s = Stack::new();
        s.request(1, APP, AUDIOFOCUS_GAIN, 0, 36);
        s.request(2, OTHER, AUDIOFOCUS_GAIN_TRANSIENT, 0, 36);
        assert_eq!(s.log(), [to("c1", AUDIOFOCUS_LOSS_TRANSIENT)]);
        assert_eq!(s.focus.current(), AUDIOFOCUS_GAIN_TRANSIENT);
        // A duck after a transient loss changes nothing; the same app's
        // duck is dispatched.
        s.request(3, OTHER, AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK, 0, 36);
        assert_eq!(s.log(), [to("c2", AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK)]);
        s.focus.abandon("c3", &mut s.pb);
        assert_eq!(s.log(), [to("c2", AUDIOFOCUS_GAIN)]);
        s.focus.abandon("c2", &mut s.pb);
        assert_eq!(s.log(), [to("c1", AUDIOFOCUS_GAIN)]);
    }

    #[test]
    fn another_apps_duck_ducks_the_players_instead() {
        let mut s = Stack::new();
        s.play(5, APP);
        s.request(1, APP, AUDIOFOCUS_GAIN, 0, 36);
        s.request(2, OTHER, AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK, 0, 36);
        assert!(s.log().is_empty());
        assert_eq!(s.pb.ducked_uids(), [APP]);
        assert!(matches!(s.pb.take_calls()[..], [(5, PlayerCall::Shape(_, Operation::Play { skip_ramp: false }))]));
        // The loser was not told, so is not told of its gain either.
        s.focus.abandon("c2", &mut s.pb);
        assert!(s.log().is_empty());
        assert!(matches!(s.pb.take_calls()[..], [(5, PlayerCall::Shape(_, Operation::Reverse))]));
        assert!(s.pb.ducked_uids().is_empty());
    }

    #[test]
    fn a_duck_is_dispatched_to_old_apps_those_that_pause_and_the_same_app() {
        for (winner, flags, sdk) in [(OTHER, 0, DUCKING_IN_APP_SDK_LEVEL), (OTHER, playback::AUDIOFOCUS_FLAG_PAUSES_ON_DUCKABLE_LOSS, 36), (APP, 0, 36)] {
            let mut s = Stack::new();
            s.play(5, APP);
            s.request(1, APP, AUDIOFOCUS_GAIN, flags, sdk);
            s.request(2, winner, AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK, 0, 36);
            assert_eq!(s.log(), [to("c1", AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK)]);
            assert!(s.pb.take_calls().is_empty());
            s.focus.abandon("c2", &mut s.pb);
            assert_eq!(s.log(), [to("c1", AUDIOFOCUS_GAIN)]);
        }
    }

    #[test]
    fn a_loss_for_good_fades_out_before_it_is_dispatched() {
        let mut s = Stack::new();
        s.play(5, APP);
        s.request(1, APP, AUDIOFOCUS_GAIN, 0, 36);
        s.request(2, OTHER, AUDIOFOCUS_GAIN, 0, 36);
        assert!(s.log().is_empty());
        assert!(matches!(s.pb.take_calls()[..], [(5, PlayerCall::Shape(_, Operation::Play { skip_ramp: false }))]));
        // In limbo: off the stack, its link kept until its loss is told.
        assert_eq!(s.focus.stack.len(), 1);
        assert!(s.focus.take_released().is_empty());
        let timers = s.focus.take_timers();
        assert!(matches!(timers[..], [Timer::LossAfterFade { delay_ms: 2000, .. }]));
        s.focus.fire(timers[0], &mut s.pb);
        assert_eq!(s.log(), [to("c1", AUDIOFOCUS_LOSS)]);
        assert_eq!(s.focus.take_released(), [1]);
        // An offender is faded back in after a delay.
        let timers = s.focus.take_timers();
        assert!(matches!(timers[..], [Timer::ForgetUid { uid: APP, delay_ms: 2000, .. }]));
        s.focus.fire(timers[0], &mut s.pb);
        assert!(matches!(s.pb.take_calls()[..], [(5, PlayerCall::Shape(_, Operation::Join(2)))]));
    }

    #[test]
    fn a_gain_fades_in_at_once_and_cancels_the_delayed_fade_in() {
        let mut s = Stack::new();
        s.play(5, APP);
        s.request(1, APP, AUDIOFOCUS_GAIN, 0, 36);
        s.request(2, OTHER, AUDIOFOCUS_GAIN, 0, 36);
        s.pb.take_calls();
        let timers = s.focus.take_timers();
        s.focus.fire(timers[0], &mut s.pb);
        let forget = s.focus.take_timers()[0];
        s.request(3, APP, AUDIOFOCUS_GAIN, 0, 36);
        assert!(matches!(s.pb.take_calls()[..], [(5, PlayerCall::Shape(_, Operation::Join(2)))]));
        s.focus.fire(forget, &mut s.pb);
        assert!(s.pb.take_calls().is_empty());
    }

    #[test]
    fn a_call_locks_focus_and_mutes_media() {
        let mut s = Stack::new();
        s.play(5, APP);
        s.request(1, APP, AUDIOFOCUS_GAIN_TRANSIENT, playback::AUDIOFOCUS_FLAG_PAUSES_ON_DUCKABLE_LOSS, 36);
        let call = Request {
            attributes: Attributes::built(2, 1, 0),
            gain: AUDIOFOCUS_GAIN_TRANSIENT,
            source: 9,
            dispatcher: Some(Recorder(s.log.clone())),
            client_id: IN_VOICE_COMM_FOCUS_ID.to_string(),
            package: "telecom".to_string(),
            flags: AUDIOFOCUS_FLAG_LOCK,
            sdk: 36,
            force_duck: false,
            uid: 1000,
        };
        assert_eq!(s.focus.request(call, || 9, &mut s.pb), AUDIOFOCUS_REQUEST_GRANTED);
        assert_eq!(s.log(), [to("c1", AUDIOFOCUS_LOSS_TRANSIENT)]);
        s.fire_all();
        assert_eq!(s.pb.take_calls(), [(5, PlayerCall::Volume(0.0))]);
        // Locked: a request fails, or waits below the call.
        assert_eq!(s.request(2, OTHER, AUDIOFOCUS_GAIN, 0, 36), AUDIOFOCUS_REQUEST_FAILED);
        assert_eq!(s.request(2, OTHER, AUDIOFOCUS_GAIN, AUDIOFOCUS_FLAG_DELAY_OK, 36), AUDIOFOCUS_REQUEST_DELAYED);
        assert_eq!(s.log(), [to("c1", AUDIOFOCUS_LOSS)]);
        assert_eq!(s.focus.current(), AUDIOFOCUS_GAIN_TRANSIENT);
        s.focus.abandon(IN_VOICE_COMM_FOCUS_ID, &mut s.pb);
        assert_eq!(s.log(), [to("c2", AUDIOFOCUS_GAIN)]);
        assert_eq!(s.focus.current(), AUDIOFOCUS_GAIN);
        s.fire_all();
        assert_eq!(s.pb.take_calls(), [(5, PlayerCall::Volume(1.0))]);
    }

    #[test]
    fn the_holders_death_gives_focus_back() {
        let mut s = Stack::new();
        s.request(1, APP, AUDIOFOCUS_GAIN, 0, 36);
        s.request(2, OTHER, AUDIOFOCUS_GAIN_TRANSIENT, 0, 36);
        s.log();
        s.focus.died(2, &mut s.pb);
        assert_eq!(s.log(), [to("c1", AUDIOFOCUS_GAIN)]);
        assert_eq!(s.focus.take_released(), [2]);
        // Not the holder's: nobody gains.
        s.request(3, OTHER, AUDIOFOCUS_GAIN_TRANSIENT, 0, 36);
        s.log();
        s.focus.died(1, &mut s.pb);
        assert!(s.log().is_empty());
        assert_eq!(s.focus.stack.len(), 1);
    }

    #[test]
    fn the_stack_is_written_as_focus_infos() {
        let mut s = Stack::new();
        s.request(1, APP, AUDIOFOCUS_GAIN, 0, 36);
        s.request(2, OTHER, AUDIOFOCUS_GAIN_TRANSIENT, AUDIOFOCUS_FLAG_DELAY_OK, 34);
        let mut p = Parcel::new();
        aim_service_aidl::write_typed_list(&mut p, Some(&s.focus.infos()));
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (2, 1));
        Attributes::read_from(&mut r).unwrap();
        assert_eq!(r.read_i32().unwrap(), APP);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("c1"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("p10154"));
        let fields: Vec<i32> = (0..4).map(|_| r.read_i32().unwrap()).collect();
        assert_eq!(fields, [AUDIOFOCUS_GAIN, AUDIOFOCUS_LOSS_TRANSIENT, 0, 36]);
        assert_eq!(r.read_i64().unwrap(), 0);
    }

    #[test]
    fn ramp_times_follow_the_public_usage() {
        assert_eq!(ramp_time_ms(&Attributes::built(playback::USAGE_MEDIA, 2, 0)), 1000);
        assert_eq!(ramp_time_ms(&Attributes::built(4, 4, 0)), 700);
        assert_eq!(ramp_time_ms(&Attributes::built(5, 4, 0)), 500);
        assert_eq!(ramp_time_ms(&Attributes::built(1003, 0, 0)), 0);
    }
}
