//! Optional local Hue ownership. No bridge credentials enter this application.
//!
//! A Play intent arms the policy; the first presented video frame starts a worker.
//! The worker renews a loopback lease. Leaving the player/backgrounding, an error,
//! process exit or a missed lease restores the pre-playback Hue state.
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct Session {
    policy: Option<Policy>,
    #[cfg(not(any(test, feature = "hostsim")))]
    worker: Option<std::sync::mpsc::Sender<()>>,
}

struct Policy {
    deadline: Instant,
    entered: bool,
    started: bool,
}

#[derive(Debug, PartialEq)]
enum Action { Wait, Start, Stop }

impl Policy {
    fn step(&mut self, now: Instant, player: bool, foreground: bool, picture: bool, failed: bool) -> Action {
        if !foreground || failed || (self.entered && !player) || (!self.started && now >= self.deadline) {
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
    pub(crate) fn arm(&mut self, enabled: bool) {
        self.stop();
        if enabled {
            self.policy = Some(Policy { deadline: Instant::now() + Duration::from_secs(60), entered: false, started: false });
        }
    }

    fn stop(&mut self) {
        self.policy = None;
        #[cfg(not(any(test, feature = "hostsim")))]
        if let Some(worker) = self.worker.take() {
            // Unbounded single-use channel: this never waits on network IO or a worker.
            let _ = worker.send(());
        }
    }

    pub(crate) fn tick(&mut self, player: bool, foreground: bool, picture: bool, failed: bool) {
        let Some(policy) = self.policy.as_mut() else { return };
        match policy.step(Instant::now(), player, foreground, picture, failed) {
            Action::Wait => {}
            Action::Stop => self.stop(),
            Action::Start => {
                #[cfg(not(any(test, feature = "hostsim")))]
                { self.worker = transport::start(); }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) { self.stop(); }
}

#[cfg(not(any(test, feature = "hostsim")))]
mod transport {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn request(command: &str, token: &str) -> std::io::Result<()> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 8976));
        let mut socket = TcpStream::connect_timeout(&address, Duration::from_millis(500))?;
        socket.set_read_timeout(Some(Duration::from_secs(8)))?;
        socket.set_write_timeout(Some(Duration::from_secs(1)))?;
        write!(socket, "POST /api/playback/{command} HTTP/1.0\r\nHost: 127.0.0.1:8976\r\nX-LG-Hue-Test: 1\r\nX-LG-Hue-Session: {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        let mut response = Vec::new();
        socket.take(65536).read_to_end(&mut response)?;
        let first = response.split(|byte| *byte == b'\n').next().unwrap_or_default();
        if !first.starts_with(b"HTTP/1.0 200 ") && !first.starts_with(b"HTTP/1.1 200 ") {
            return Err(std::io::Error::other("local Hue lease refused"));
        }
        Ok(())
    }

    pub(super) fn start() -> Option<Sender<()>> {
        let (sender, receiver) = mpsc::channel();
        let token = format!("{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());
        match std::thread::Builder::new().name("hue-lease".into()).spawn(move || {
            let result = request("start", &token).and_then(|()| {
                loop {
                    match receiver.recv_timeout(Duration::from_secs(2)) {
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                        Err(RecvTimeoutError::Timeout) => request("heartbeat", &token)?,
                    }
                }
            });
            // Always release, even if the start timed out after the server accepted it.
            let _ = request("stop", &token);
            if result.is_err() {
                plx_base::eventlog::log("Hue: local synchronization unavailable; playback continues");
                let _ = plx_platform::tv::toast::toast(plx_platform::i18n::msg::widgets_hue_unavailable());
            }
        }) {
            Ok(_) => Some(sender),
            Err(_) => {
                plx_base::eventlog::log("Hue: worker could not start; playback continues");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy(now: Instant) -> Policy {
        Policy { deadline: now + Duration::from_secs(60), entered: false, started: false }
    }
    #[test]
    fn waits_for_video_in_the_player_and_starts_once() {
        let now = Instant::now();
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
        let now = Instant::now();
        assert_eq!(policy(now).step(now, false, false, false, false), Action::Stop);
        assert_eq!(policy(now).step(now, true, true, false, true), Action::Stop);
        assert_eq!(policy(now).step(now + Duration::from_secs(60), false, true, false, false), Action::Stop);
    }
    #[test]
    fn ordinary_play_disarms_a_previous_hue_request() {
        let mut session = Session::default();
        session.arm(true);
        assert!(session.policy.is_some());
        session.arm(false);
        assert!(session.policy.is_none());
    }
}
