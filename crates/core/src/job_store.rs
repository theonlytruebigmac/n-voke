//! The live list of jobs: loads saved history, watches unfinished tasks by
//! polling the API, and saves history again. Shared with the desktop app through
//! the same encrypted `jobs` store, and following the same rules as `jobs.ts`.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::Utc;

use crate::{
    Core,
    jobs::{Job, MAX_JOBS, STORE_NAME, cap_output, parse_jobs, retain, rewatch, sort_newest_first},
    poll::{POLL_INTERVAL_MS, apply_batch, expire, watched_batches},
    task_state::{JobTask, StopReason, is_done},
};

/// How long to wait after a change before saving, so a burst becomes one write.
const SAVE_DELAY: Duration = Duration::from_secs(1);

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

struct State {
    jobs: Vec<Job>,
    history_days: u32,
    /// True once saved history has been read; until then nothing is written (so it can't be overwritten).
    loaded: bool,
    /// Whether the encrypted store works at all (no keychain: history stays in memory).
    persist: bool,
    dirty: bool,
    /// Bumped on every change, so a UI can tell when to refresh what it shows.
    version: u64,
}

pub struct JobStore {
    core: Arc<Core>,
    state: Mutex<State>,
    polling: AtomicBool,
    /// One write at a time, so a slow write can't land after (and undo) a newer one.
    saving: tokio::sync::Mutex<()>,
}

impl JobStore {
    pub fn new(core: Arc<Core>, history_days: u32) -> Arc<Self> {
        Arc::new(Self {
            core,
            state: Mutex::new(State { jobs: vec![], history_days, loaded: false, persist: true, dirty: false, version: 0 }),
            polling: AtomicBool::new(false),
            saving: tokio::sync::Mutex::new(()),
        })
    }

    /// Loads saved history. Jobs started before loading finished win over saved copies.
    pub async fn load(&self) {
        let stored = self.core.store_get(STORE_NAME).await;
        let mut state = self.state.lock().unwrap();
        let saved = match &stored {
            Ok(raw) => parse_jobs(raw.as_deref()),
            Err(_) => {
                state.persist = false;
                vec![]
            }
        };
        let mut by_id: Vec<Job> = rewatch(saved, now_ms());
        for j in std::mem::take(&mut state.jobs) {
            match by_id.iter().position(|s| s.id == j.id) {
                Some(p) => by_id[p] = j,
                None => by_id.push(j),
            }
        }
        sort_newest_first(&mut by_id);
        state.jobs = retain(by_id, u32::MAX, now_ms(), MAX_JOBS);
        state.loaded = true;
        state.version += 1;
    }

    /// The jobs, newest first.
    pub fn jobs(&self) -> Vec<Job> {
        self.state.lock().unwrap().jobs.clone()
    }

    /// Changes whenever the jobs do.
    pub fn version(&self) -> u64 {
        self.state.lock().unwrap().version
    }

    pub fn is_loaded(&self) -> bool {
        self.state.lock().unwrap().loaded
    }

    pub fn set_history_days(&self, days: u32) {
        let mut s = self.state.lock().unwrap();
        s.history_days = days;
        Self::changed(&mut s);
    }

    fn changed(s: &mut State) {
        s.version += 1;
        s.dirty = true;
    }

    /// Adds a job (replacing one with the same id) and starts watching it.
    pub async fn add(&self, job: Job) {
        {
            let mut s = self.state.lock().unwrap();
            s.jobs.retain(|j| j.id != job.id);
            s.jobs.insert(0, job);
            // Past the limit, older finished jobs make way; ones still being watched stay.
            let jobs = std::mem::take(&mut s.jobs);
            s.jobs = jobs.into_iter().enumerate().filter(|(i, j)| *i < MAX_JOBS || j.is_active()).map(|(_, j)| j).collect();
            Self::changed(&mut s);
        }
        self.poll_once().await;
    }

    /// Resumes watching every unfinished task in a job (or just one).
    pub async fn resume(&self, job_id: &str, task_id: Option<&str>) {
        {
            let mut s = self.state.lock().unwrap();
            let now = now_ms();
            for t in s.jobs.iter_mut().filter(|j| j.id == job_id).flat_map(|j| j.tasks.iter_mut()) {
                if task_id.is_none_or(|id| id == t.task_id) && !is_done(t) {
                    *t = JobTask { watching: true, watch_started_at: now, poll_failures: 0, last_error: None, stop_reason: None, ..t.clone() };
                }
            }
            Self::changed(&mut s);
        }
        self.poll_once().await;
    }

    pub fn stop(&self, job_id: &str) {
        let mut s = self.state.lock().unwrap();
        for t in s.jobs.iter_mut().filter(|j| j.id == job_id).flat_map(|j| j.tasks.iter_mut()).filter(|t| t.watching) {
            t.watching = false;
            t.stop_reason = Some(StopReason::Manual);
        }
        Self::changed(&mut s);
    }

    pub fn remove(&self, job_id: &str) {
        let mut s = self.state.lock().unwrap();
        s.jobs.retain(|j| j.id != job_id);
        Self::changed(&mut s);
    }

    /// Forgets all finished jobs (after the user clears local data). Jobs still running keep being watched.
    pub fn clear_finished(&self) {
        let mut s = self.state.lock().unwrap();
        s.jobs.retain(Job::is_active);
        Self::changed(&mut s);
    }

    /// One round of polling: expire what has run out of time, then ask about the rest.
    pub async fn poll_once(&self) {
        if self.polling.swap(true, Ordering::SeqCst) {
            return;
        }
        let batches = {
            let mut s = self.state.lock().unwrap();
            if expire(&mut s.jobs, now_ms()) {
                Self::changed(&mut s);
            }
            watched_batches(&s.jobs)
        };
        for ids in batches {
            let result = self.core.get_tasks(&ids).await;
            let mut s = self.state.lock().unwrap();
            apply_batch(&mut s.jobs, &ids, result);
            Self::changed(&mut s);
        }
        self.polling.store(false, Ordering::SeqCst);
    }

    /// Writes the retained history, encrypted. Nothing is written when history isn't saved.
    pub async fn save(&self) {
        let _one_at_a_time = self.saving.lock().await;
        let (days, jobs) = {
            let mut s = self.state.lock().unwrap();
            if !s.loaded || !s.persist {
                return;
            }
            s.dirty = false;
            (s.history_days, s.jobs.clone())
        };
        if days == 0 {
            let _ = self.core.store_remove(STORE_NAME).await;
            return;
        }
        let keep: Vec<Job> = retain(jobs, days, now_ms(), MAX_JOBS).iter().map(cap_output).collect();
        if let Ok(json) = serde_json::to_string(&keep) {
            if let Err(e) = self.core.store_set(STORE_NAME, json).await {
                eprintln!("Couldn't save the run history: {e}");
            }
        }
    }

    /// Runs forever: polls every few seconds and saves changes shortly after they happen.
    pub async fn run(self: Arc<Self>) {
        let poller = {
            let store = self.clone();
            async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
                    store.poll_once().await;
                }
            }
        };
        let saver = {
            let store = self.clone();
            async move {
                loop {
                    tokio::time::sleep(SAVE_DELAY).await;
                    if store.state.lock().unwrap().dirty {
                        store.save().await;
                    }
                }
            }
        };
        tokio::join!(poller, saver);
    }
}
