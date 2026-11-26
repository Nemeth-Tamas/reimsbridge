use crate::sync_engine::{
    Counters, Decision, Direction, PayloadDecision, Snapshot, SyncEngine, inspect_payload,
    still_current,
};
use crate::sync_read::ReadOutcome;
use crate::{clipboard, mac_ipc};

use ssh2::Session;
use std::error::Error;
use std::thread;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(300);

fn snapshot(session: &Session) -> Result<Option<Snapshot>, Box<dyn Error>> {
    let Some(windows_before) = clipboard::metadata_checked()? else {
        return Ok(None);
    };

    let mac = mac_ipc::get_status(session)?;

    let Some(windows_after) = clipboard::metadata_checked()? else {
        return Ok(None);
    };

    if windows_before.sequence_number != windows_after.sequence_number {
        return Ok(None);
    }

    Ok(Some(Snapshot {
        counters: Counters {
            windows: windows_after.sequence_number,
            mac: mac.change_count,
        },
        windows_has_text: windows_after.has_plain_text,
        mac_has_text: mac.has_plain_text,
    }))
}

fn counters_current(session: &Session, expected: Counters) -> Result<bool, Box<dyn Error>> {
    Ok(snapshot(session)?.is_some_and(|actual| still_current(expected, actual.counters)))
}

fn skip_if_current(
    session: &Session,
    engine: &mut SyncEngine,
    observed: Counters,
) -> Result<(), Box<dyn Error>> {
    if counters_current(session, observed)? {
        engine.record_skipped(observed);
    }

    Ok(())
}

fn transfer(
    session: &Session,
    engine: &mut SyncEngine,
    observed: Snapshot,
    direction: Direction,
) -> Result<(), Box<dyn Error>> {
    let source = match direction {
        Direction::WindowsToMac => clipboard::read_for_sync()?,

        Direction::MacToWindows => mac_ipc::read_for_sync(session)?,
    };

    let source = match source {
        ReadOutcome::Text(text) => text,

        ReadOutcome::Unavailable => {
            println!("[WATCH] Unsupported source text; skipping");

            return skip_if_current(session, engine, observed.counters);
        }

        ReadOutcome::Changed => {
            println!("[WATCH] Source changed during read; reconsidering");

            return Ok(());
        }
    };

    let destination = match direction {
        Direction::WindowsToMac if observed.mac_has_text => mac_ipc::read_for_sync(session)?,

        Direction::MacToWindows if observed.windows_has_text => clipboard::read_for_sync()?,

        _ => ReadOutcome::Unavailable,
    };

    let destination = match destination {
        ReadOutcome::Text(text) => Some(text),

        ReadOutcome::Unavailable => None,

        ReadOutcome::Changed => {
            println!("[WATCH] Destination changed during read; reconsidering");

            return Ok(());
        }
    };

    let decision = inspect_payload(&source, destination.as_deref());

    if !counters_current(session, observed.counters)? {
        println!("[WATCH] Clipboard changed before transfer; reconsidering");

        return Ok(());
    }

    match decision {
        PayloadDecision::Invalid => {
            engine.record_skipped(observed.counters);

            println!("[WATCH] Invalid payload; skipping");

            return Ok(());
        }

        PayloadDecision::Identical => {
            engine.record_skipped(observed.counters);

            println!("[WATCH] Identical text; write suppressed");

            return Ok(());
        }

        PayloadDecision::Write => {}
    }

    match direction {
        Direction::WindowsToMac => {
            let written = mac_ipc::write_text_checked(session, &source, observed.counters.mac)?;

            match written {
                Some(counter) => {
                    engine.record_mac_write(observed.counters, counter);

                    println!("[WATCH] Windows -> macOS: {} bytes", source.len());
                }

                None => {
                    println!("[WATCH] macOS destination became stale; write rejected");
                }
            }
        }

        Direction::MacToWindows => {
            let written = clipboard::write_text_checked(&source, Some(observed.counters.windows))?;

            match written {
                Some(sequence) => {
                    engine.record_windows_write(observed.counters, sequence);

                    println!("[WATCH] macOS -> Windows: {} bytes", source.len());
                }

                None => {
                    println!("[WATCH] Windows destination became stale; write rejected");
                }
            }
        }
    }

    Ok(())
}

pub fn run(session: &Session) -> Result<(), Box<dyn Error>> {
    let mut engine = SyncEngine::new();

    println!("[WATCH] Starting native clipboard synchronization");

    println!("[WATCH] Poll interval: {} ms", POLL_INTERVAL.as_millis());

    println!("[WATCH] Privacy-marked text is allowed");
    println!("[WATCH] Press Ctrl+C to stop");

    loop {
        if let Some(observed) = snapshot(session)? {
            match engine.observe(observed) {
                Decision::Baseline => {
                    println!(
                        "[WATCH] Baseline established: Windows={}, macOS={}",
                        observed.counters.windows, observed.counters.mac,
                    );

                    println!("[WATCH] Automatic synchronization active");
                }

                Decision::Idle => {}

                Decision::Unsupported => {
                    println!("[WATCH] Unsupported clipboard change; skipping");
                }

                Decision::Conflict => {
                    println!("[WATCH] Simultaneous clipboard changes; preserving both");
                }

                Decision::Transfer(direction) => {
                    transfer(session, &mut engine, observed, direction)?;
                }
            }
        }

        thread::sleep(POLL_INTERVAL);
    }
}
