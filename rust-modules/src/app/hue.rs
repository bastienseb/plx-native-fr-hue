//! Optional local Hue ownership. No bridge credentials enter this application.
//!
//! A Play intent arms the policy; the first presented video frame starts a worker.
//! The worker renews a loopback lease. Leaving the player/backgrounding, an error,
//! process exit or a missed lease restores the pre-playback Hue state.

#[derive(Default)]
pub(crate) struct Session {
    policy: Option<Policy>,
    #[cfg(not(any(test, feature = "hostsim")))]
    worker: Option<std::sync::mpsc::Sender<()>>,
}

struct Policy {
    armed_at: u32,
    entered: bool,
    started: bool,
}

#[derive(Debug, PartialEq)]
enum Action { Wait, Start, Stop }

impl Policy {
    fn step(&mut self, now: u32, player: bool, foreground: bool, picture: bool, failed: bool) -> Action {
        if !foreground || failed || (self.entered && !player) || (!self.started && now.wrapping_sub(self.armed_at) >= 60_000) {
            return Action::Stop;
        }
        self.entered |= player;
        if player && picture && !self.started {
            self.started = true;
            return Action::Start;
        }
        Action::Wait
    }
}

impl Session {
    #[cfg(all(test, feature = "hostsim"))]
    pub(super) fn started_for_test(&self) -> bool {
        self.policy.as_ref().is_some_and(|policy| policy.started)
    }

    pub(crate) fn arm(&mut self, enabled: bool, now: u32) {
        self.stop();
        if enabled {
            #[cfg(not(test))]
            plx_base::eventlog::log("Hue: waiting for the first player picture");
            self.policy = Some(Policy { armed_at: now, entered: false, started: false });
        }
    }

    fn stop(&mut self) {
        #[cfg(not(test))]
        if self.policy.is_some() {
            plx_base::eventlog::log("Hue: releasing playback session");
        }
        self.policy = None;
        #[cfg(not(any(test, feature = "hostsim")))]
        if let Some(worker) = self.worker.take() {
            // Unbounded single-use channel: this never waits on network IO or a worker.
            let _ = worker.send(());
        }
    }

    pub(crate) fn tick(&mut self, now: u32, player: bool, foreground: bool, picture: bool, failed: bool) {
        let Some(policy) = self.policy.as_mut() else { return };
        match policy.step(now, player, foreground, picture, failed) {
            Action::Wait => {}
            Action::Stop => self.stop(),
            Action::Start => {
                #[cfg(not(test))]
                plx_base::eventlog::log("Hue: first player picture; requesting synchronization");
                #[cfg(not(any(test, feature = "hostsim")))]
                { self.worker = plx_net::hue::start(|| {
                    plx_base::eventlog::log("Hue: local synchronization unavailable; playback continues");
                    let _ = plx_platform::tv::toast::toast(plx_platform::i18n::msg::widgets_hue_unavailable());
                }); }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) { self.stop(); }
}


#[cfg(test)]
mod tests {
    use super::*;
    fn policy(now: u32) -> Policy {
        Policy { armed_at: now, entered: false, started: false }
    }
    #[test]
    fn waits_for_video_in_the_player_and_starts_once() {
        let now = 1_000;
        let mut p = policy(now);
        // A stale trailer frame on the detail page must not start the lights.
        assert_eq!(p.step(now, false, true, true, false), Action::Wait);
        assert_eq!(p.step(now, true, true, false, false), Action::Wait);
        assert_eq!(p.step(now, true, true, true, false), Action::Start);
        assert_eq!(p.step(now, true, true, true, false), Action::Wait);
        // A seek/reload can temporarily clear seen_frame without ending the session.
        assert_eq!(p.step(now, true, true, false, false), Action::Wait);
        assert_eq!(p.step(now, false, true, false, false), Action::Stop);
    }
    #[test]
    fn background_failure_and_start_timeout_cancel() {
        let now = 1_000;
        assert_eq!(policy(now).step(now, false, false, false, false), Action::Stop);
        assert_eq!(policy(now).step(now, true, true, false, true), Action::Stop);
        assert_eq!(policy(now).step(now + 60_000, false, true, false, false), Action::Stop);
    }
    #[test]
    fn ordinary_play_disarms_a_previous_hue_request() {
        let mut session = Session::default();
        session.arm(true, 0);
        assert!(session.policy.is_some());
        session.arm(false, 0);
        assert!(session.policy.is_none());
    }
    #[test]
    fn startup_timeout_survives_the_frame_clock_wrapping() {
        let now = u32::MAX - 30_000;
        let mut p = policy(now);
        assert_eq!(p.step(now.wrapping_add(59_999), false, true, false, false), Action::Wait);
        assert_eq!(p.step(now.wrapping_add(60_000), false, true, false, false), Action::Stop);
    }
}
