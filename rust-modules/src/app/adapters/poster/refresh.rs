//! Bounded stale-while-revalidate lane. Cached pixels reach the render queue immediately;
//! slow or offline servers cannot occupy either demand decoder or the main-thread store lock.
//! Visible artwork is revalidated at a bounded cadence, only while the source is idle.
//! Metadata selects the current poster/backdrop/logo; changed pixels also reach the live cache.

use plx_platform::imgcache::DiskKey;
use std::collections::{HashMap, VecDeque};
use std::sync::{Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const QUEUE_CAP: usize = 64;
const RECENT_CAP: usize = 8192;
const REFRESH_PERIOD_MS: u32 = 15_000;

pub(super) struct Job {
    pub(super) client: &'static plx_plex::plex::Client,
    pub(super) path: String,
    key: DiskKey,
    pub(super) epoch: u64,
    pub(super) token_gen: u32,
    force: bool,
}

pub(super) enum Change { Pixels(plx_ui::tex::Decoded), Removed }
pub(super) struct Landed { pub(super) job: Job, pub(super) change: Change }
static LANDED: Mutex<Option<Landed>> = Mutex::new(None);

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    seen: HashMap<DiskKey, u32>,
    resolved: HashMap<DiskKey, Option<String>>,
    order: VecDeque<DiskKey>,
    last_sweep: Option<u32>,
    epoch: u64,
    running: bool,
}
impl Queue {
    fn push(&mut self, job: Job) -> bool {
        // Worker arrivals inherit the latest main-thread sweep tick, never read SDL.
        self.push_at(job, self.last_sweep.unwrap_or(0))
    }
    fn push_at(&mut self, job: Job, now: u32) -> bool {
        if !self.running || job.epoch < self.epoch { return false; }
        if self.epoch != job.epoch {
            self.jobs.clear();
            self.seen.clear();
            self.resolved.clear();
            self.order.clear();
            self.epoch = job.epoch;
        }
        if self.jobs.len() >= QUEUE_CAP || self.seen.get(&job.key)
            .is_some_and(|last| now.wrapping_sub(*last) < REFRESH_PERIOD_MS) { return false; }
        if !self.seen.contains_key(&job.key) {
            if self.order.len() == RECENT_CAP {
                if let Some(old) = self.order.pop_front() {
                    self.seen.remove(&old);
                    self.resolved.remove(&old);
                }
            }
            self.order.push_back(job.key.clone());
        }
        self.seen.insert(job.key.clone(), now);
        self.jobs.push_back(job);
        true
    }
}

static QUEUE: Mutex<Option<Queue>> = Mutex::new(None);
static READY: Condvar = Condvar::new();
static WORKER: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

pub(super) fn enqueue(client: &'static plx_plex::plex::Client, path: String, key: DiskKey, epoch: u64, token_gen: u32) {
    enqueue_with_force(client, path, key, epoch, token_gen, true);
}

pub(super) fn enqueue_visible(client: &'static plx_plex::plex::Client, path: String, key: DiskKey, epoch: u64, token_gen: u32) {
    enqueue_with_force(client, path, key, epoch, token_gen, false);
}

fn enqueue_with_force(client: &'static plx_plex::plex::Client, path: String, key: DiskKey, epoch: u64, token_gen: u32, force: bool) {
    if epoch != plx_platform::imgcache::generation() { return; }
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if q.as_mut().is_some_and(|q| q.push(Job { client, path, key, epoch, token_gen, force })) {
        READY.notify_one();
    }
}

pub(super) fn sweep_due(now: u32) -> bool {
    let mut queue = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(queue) = queue.as_mut().filter(|queue| queue.running) else { return false; };
    if queue.last_sweep.is_some_and(|last| now.wrapping_sub(last) < REFRESH_PERIOD_MS) { return false; }
    queue.last_sweep = Some(now);
    true
}

pub(super) fn take_landed() -> Option<Landed> {
    let landed = LANDED.lock().unwrap_or_else(|e| e.into_inner()).take();
    if landed.is_some() { READY.notify_one(); }
    landed
}

pub(super) fn init() {
    *QUEUE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Queue { running: true, ..Default::default() });
    let worker = plx_base::task::spawn("image-refresh", run);
    if worker.is_none() {
        QUEUE.lock().unwrap_or_else(|e| e.into_inner()).as_mut().unwrap().running = false;
    }
    *WORKER.lock().unwrap_or_else(|e| e.into_inner()) = worker;
}

pub(super) fn shutdown() {
    if let Some(q) = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        q.running = false;
        q.jobs.clear();
    }
    READY.notify_all();
    if let Some(h) = WORKER.lock().unwrap_or_else(|e| e.into_inner()).take() {
        plx_base::task::join("image-refresh", h);
    }
    LANDED.lock().unwrap_or_else(|e| e.into_inner()).take();
}

fn run() {
    loop {
        let job = {
            let mut guard = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                let Some(q) = guard.as_mut() else { return; };
                if !q.running { return; }
                if !q.jobs.is_empty() && super::store_idle() && plx_ui::tex::pending_bytes() == 0
                    && LANDED.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
                    break q.jobs.pop_front().unwrap();
                }
                guard = READY.wait_timeout(guard, Duration::from_millis(250))
                    .map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
            }
        };
        if job.epoch != plx_platform::imgcache::generation() || !grant_is_current(&job) { continue; }
        refresh(job);
    }
}

fn current_request(request: &str, source: &str) -> Option<String> {
    let (path, query) = request.split_once('?')?;
    if path != "/photo/:/transcode" { return None; }
    Some(format!("{path}?{}", query.split('&').map(|pair| {
        if pair.split_once('=').is_some_and(|(key, _)| key == "url") {
            format!("url={}", plx_plex::plex::urlenc_str(source))
        } else { pair.to_string() }
    }).collect::<Vec<_>>().join("&")))
}

fn refresh(job: Job) {
    let Some(source) = plx_platform::imgcache::transcode_source(&job.path) else { return; };
    let Some(current) = job.client.current_artwork_source(&source) else { return; };
    if job.epoch != plx_platform::imgcache::generation() || !grant_is_current(&job) { return; }
    let unchanged = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        .filter(|queue| queue.epoch == job.epoch)
        .and_then(|queue| queue.resolved.get(&job.key)).is_some_and(|previous| *previous == current);
    if unchanged && !job.force { return; }
    let change = if let Some(ref source) = current {
        let Some(path) = current_request(&job.path, source) else { return; };
        let Some(bytes) = super::fetch_image(job.client, &path) else { return; };
        if job.epoch != plx_platform::imgcache::generation() || !grant_is_current(&job) { return; }
        // The disk cache may already be newer than the resident texture. Always land
        // this revision once; `resolved` prevents repeated decoding on later sweeps.
        {
            let (mut w, mut h) = (0, 0);
            let px = plx_gfx::img::img_decode_rgba(bytes.as_ptr(), bytes.len() as i32, &mut w, &mut h);
            if px.is_null() { return; }
            let pixels = if w > 0 && h > 0 && w <= u16::MAX as i32 && h <= u16::MAX as i32 {
                // SAFETY: img_decode_rgba owns exactly w*h decoded RGBA bytes.
                Some(unsafe { std::slice::from_raw_parts(px, w as usize * h as usize * 4) }.into())
            } else { None };
            plx_gfx::img::img_free(px);
            let Some(rgba) = pixels else { return; };
            plx_platform::imgcache::write_at(job.epoch, &job.key, &bytes);
            Some(Change::Pixels(plx_ui::tex::Decoded { w: w as u16, h: h as u16, rgba }))
        }
    } else {
        plx_platform::imgcache::remove_at(job.epoch, &job.key);
        Some(Change::Removed)
    };
    if job.epoch != plx_platform::imgcache::generation() || !grant_is_current(&job) { return; }
    if let Some(queue) = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
        .filter(|queue| queue.epoch == job.epoch) {
        queue.resolved.insert(job.key.clone(), current);
    }
    if let Some(change) = change {
        *LANDED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Landed { job, change });
        plx_machine::idle::invalidate();
    }
}

/// Profile changes retain disk images but retire their old request credentials. A client can
/// also be replaced at a new origin without changing the account's disk-cache epoch.
fn grant_is_current(job: &Job) -> bool {
    plx_plex::plex::client_for(job.client.id()).is_some_and(|live|
        std::ptr::eq(live, job.client) && live.token_gen() == job.token_gen)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(client: &'static plx_plex::plex::Client, i: usize, epoch: u64) -> Job {
        let path = format!("/photo/:/transcode?url=%2Fthumb%2F{i}&width=250&height=375");
        Job { client, key: plx_platform::imgcache::classify("fixture", &path).unwrap(), path, epoch, token_gen: client.token_gen(), force: true }
    }
    #[test]
    fn refreshes_are_bounded_deduplicated_and_retired_with_the_account() {
        let _guard = plx_base::testlock::serial();
        plx_plex::plex::reset_servers_for_test();
        let sid = plx_plex::plex::register_for_test("refresh-fixture", "127.0.0.1", 1, "fixture", "fixture");
        let client = plx_plex::plex::client_for(sid).unwrap();
        let old_grant = job(client, 0, 1);
        assert!(grant_is_current(&old_grant));
        let job = |i, epoch| job(client, i, epoch);
        let mut q = Queue { running: true, ..Default::default() };
        assert!(q.push(job(0, 1)));
        assert!(!q.push(job(0, 1)));
        q.jobs.pop_front();
        assert!(!q.push(job(0, 1)), "failed attempts must not spin on every revisit");
        for i in 1..=QUEUE_CAP { assert!(q.push(job(i, 1))); }
        assert!(!q.push(job(QUEUE_CAP + 1, 1)));
        assert!(q.push(job(0, 2)));
        assert_eq!(q.jobs.len(), 1);
        assert_eq!(q.seen.len(), 1);
        assert!(!q.push(job(1, 1)), "delayed old-account work cannot displace current refreshes");
        assert_eq!(q.jobs.len(), 1);
        assert_eq!(q.epoch, 2);
        q.running = false;
        assert!(!q.push(job(1, 2)));
        plx_plex::plex::register_for_test("refresh-fixture", "127.0.0.1", 1, "changed-grant", "fixture");
        assert!(!grant_is_current(&old_grant), "queued old-profile credentials must never refresh");
        plx_plex::plex::reset_servers_for_test();
    }

    #[test]
    fn visible_art_can_be_rechecked_after_the_refresh_period() {
        let _guard = plx_base::testlock::serial();
        plx_plex::plex::reset_servers_for_test();
        let sid = plx_plex::plex::register_for_test("periodic-art", "127.0.0.1", 1, "fixture", "fixture");
        let client = plx_plex::plex::client_for(sid).unwrap();
        let now: u32 = u32::MAX - 5_000;
        let mut queue = Queue { running: true, ..Default::default() };
        assert!(queue.push_at(job(client, 0, 1), now));
        queue.jobs.pop_front();
        assert!(!queue.push_at(job(client, 0, 1), now.wrapping_add(REFRESH_PERIOD_MS / 2)));
        assert!(queue.push_at(job(client, 0, 1), now.wrapping_add(REFRESH_PERIOD_MS)));
        assert_eq!(queue.seen.len(), 1);
        plx_plex::plex::reset_servers_for_test();
    }

    #[test]
    fn current_image_replaces_only_the_source_and_keeps_transforms() {
        let request = "/photo/:/transcode?width=720&url=%2Fold&height=405&format=png&X-Plex-Token=fixture";
        let source = "/library/metadata/42/clearLogo?timestamp=2";
        let current = current_request(request, source).unwrap();
        assert_eq!(plx_platform::imgcache::transcode_source(&current).as_deref(), Some(source));
        assert_eq!(current, format!("/photo/:/transcode?width=720&url={}&height=405&format=png&X-Plex-Token=fixture", plx_plex::plex::urlenc_str(source)));
        assert!(current_request("/other?url=old", source).is_none());
    }
}
