use crate::credentials::{self, Credentials};
use crate::instance::InstanceGuard;
use crate::{mac_ipc, ssh, watch};
use std::error::Error;
use std::io;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub enum Update {
    Active,
    Reconnecting { delay: Duration, reason: String },
    CredentialWarning(String),
}

pub struct Worker {
    stop: Arc<AtomicBool>,
    pub updates: Receiver<Update>,
    pub thread: JoinHandle<Result<(), String>>,
}

impl Worker {
    pub fn start(credentials: Credentials) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (updates_tx, updates) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("reimsbridge-watch".into())
            .spawn(move || {
                run(credentials, &worker_stop, &updates_tx).map_err(|error| error.to_string())
            })?;

        Ok(Self {
            stop,
            updates,
            thread,
        })
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.thread().unpark();
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

struct Backoff {
    seconds: u64,
}

impl Default for Backoff {
    fn default() -> Self {
        Self { seconds: 1 }
    }
}

impl Backoff {
    fn next_delay(&mut self) -> Duration {
        let delay = Duration::from_secs(self.seconds);
        self.seconds = (self.seconds * 2).min(15);
        delay
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

fn wait_to_retry(stop: &AtomicBool, delay: Duration) {
    let deadline = Instant::now() + delay;
    while !stop.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        thread::park_timeout(remaining);
    }
}

fn run(
    mut credentials: Credentials,
    stop: &AtomicBool,
    updates: &Sender<Update>,
) -> Result<(), Box<dyn Error>> {
    let _watcher = InstanceGuard::watcher()?;
    let mut backoff = Backoff::default();

    while !stop.load(Ordering::Relaxed) {
        let error = match attempt(&mut credentials, stop, updates, &mut backoff) {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };

        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if !ssh::is_transport_error(error.as_ref()) && !mac_ipc::is_unavailable(error.as_ref()) {
            return Err(error);
        }

        let delay = backoff.next_delay();
        let _ = updates.send(Update::Reconnecting {
            delay,
            reason: error.to_string(),
        });
        wait_to_retry(stop, delay);
    }

    Ok(())
}

fn attempt(
    credentials: &mut Credentials,
    stop: &AtomicBool,
    updates: &Sender<Update>,
    backoff: &mut Backoff,
) -> Result<(), Box<dyn Error>> {
    let session = ssh::connect_verified()?;
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }

    ssh::authenticate(&session, &credentials.username, &credentials.password)?;
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }

    if mem::take(&mut credentials.remember)
        && let Err(error) = credentials::save(credentials)
    {
        let _ = updates.send(Update::CredentialWarning(format!(
            "Could not save SSH credentials in Windows Credential Manager: {error}"
        )));
    }

    // Each call owns a new sync engine and establishes a fresh baseline.
    let result = watch::run_until_stopped(&session, stop, || {
        backoff.reset();
        let _ = updates.send(Update::Active);
    });

    if result.is_ok() {
        let _ = session.disconnect(None, "ReimsBridge session complete", None);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_delay_backs_off_to_a_cap_and_resets() {
        let mut backoff = Backoff::default();
        let delay_seconds: Vec<_> = (0..7).map(|_| backoff.next_delay().as_secs()).collect();
        assert_eq!(delay_seconds, [1, 2, 4, 8, 15, 15, 15]);

        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn stop_wakes_a_worker_waiting_to_retry() {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (started_tx, started) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            wait_to_retry(&worker_stop, Duration::from_secs(30));
            done_tx.send(()).unwrap();
        });
        started.recv().unwrap();

        stop.store(true, Ordering::Relaxed);
        worker.thread().unpark();
        done.recv_timeout(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
    }
}
