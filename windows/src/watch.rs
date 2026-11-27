use crate::sync_engine::{
    Counters, Decision, Direction, PayloadDecision, Snapshot, SyncEngine, inspect_payload,
    still_current,
};
use crate::sync_read::ReadOutcome;
use crate::{clipboard, mac_ipc};

use ssh2::Session;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(300);

fn snapshot(session: &Session, watch_status: bool) -> Result<Option<Snapshot>, Box<dyn Error>> {
    let Some(windows_before) = clipboard::metadata_checked()? else {
        return Ok(None);
    };

    let mac = if watch_status {
        mac_ipc::get_watch_status(session)?
    } else {
        mac_ipc::get_status(session)?
    };

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
    Ok(snapshot(session, false)?.is_some_and(|actual| still_current(expected, actual.counters)))
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

fn poll_once(
    session: &Session,
    engine: &mut SyncEngine,
    watch_status: bool,
) -> Result<Option<Decision>, Box<dyn Error>> {
    let Some(observed) = snapshot(session, watch_status)? else {
        return Ok(None);
    };

    let decision = engine.observe(observed);

    match decision {
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
            transfer(session, engine, observed, direction)?;
        }
    }

    Ok(Some(decision))
}

fn stable_counters(session: &Session) -> Result<Counters, Box<dyn Error>> {
    for _ in 0..10 {
        if let Some(observed) = snapshot(session, false)? {
            return Ok(observed.counters);
        }

        thread::sleep(Duration::from_millis(50));
    }

    Err("Could not obtain stable clipboard counters".into())
}

fn verify_pair(
    session: &Session,
    expected_windows: &str,
    expected_mac: &str,
) -> Result<Counters, Box<dyn Error>> {
    let before = stable_counters(session)?;

    let windows_text = clipboard::read_text()?;
    let mac_text = mac_ipc::read_text(session)?;

    if windows_text.as_bytes() != expected_windows.as_bytes() {
        return Err("Windows clipboard payload mismatch".into());
    }

    if mac_text.as_bytes() != expected_mac.as_bytes() {
        return Err("macOS clipboard payload mismatch".into());
    }

    let after = stable_counters(session)?;

    if before != after {
        return Err("Clipboard changed during acceptance verification".into());
    }

    Ok(after)
}

fn expect_step(
    session: &Session,
    engine: &mut SyncEngine,
    expected: Decision,
) -> Result<(), Box<dyn Error>> {
    let actual = poll_once(session, engine, false)?;

    if actual != Some(expected) {
        return Err(format!(
            "Unexpected synchronization decision: expected {expected:?}, got {actual:?}"
        )
        .into());
    }

    Ok(())
}

fn expect_idle(
    session: &Session,
    engine: &mut SyncEngine,
    expected: Counters,
    iterations: usize,
) -> Result<(), Box<dyn Error>> {
    for _ in 0..iterations {
        thread::sleep(POLL_INTERVAL);

        expect_step(session, engine, Decision::Idle)?;

        if stable_counters(session)? != expected {
            return Err("Clipboard counters changed while idle".into());
        }
    }

    Ok(())
}

pub fn test(session: &Session) -> Result<(), Box<dyn Error>> {
    let mut engine = SyncEngine::new();

    println!("[ACCEPTANCE] M2.3D automatic synchronization");
    println!("[ACCEPTANCE] Both clipboards will be modified");

    let windows_initial = "Independent Windows startup clipboard";

    let mac_initial = "Independent macOS startup clipboard";

    clipboard::write_text(windows_initial)?;
    mac_ipc::write_text(session, mac_initial)?;

    let initial = verify_pair(session, windows_initial, mac_initial)?;

    expect_step(session, &mut engine, Decision::Baseline)?;

    if verify_pair(session, windows_initial, mac_initial)? != initial {
        return Err("Baseline modified a clipboard".into());
    }

    expect_idle(session, &mut engine, initial, 3)?;

    println!("[PASS] Startup baseline preservation");

    let windows_fixture = concat!(
        "Windows automatic synchronization\r\n",
        "Árvíztűrő tükörfúrógép 🚀 🦀\n",
        "\tTabbed text\n",
        "    Indented code\n",
        "Trailing spaces  \n",
    );

    clipboard::write_text(windows_fixture)?;

    expect_step(
        session,
        &mut engine,
        Decision::Transfer(Direction::WindowsToMac),
    )?;

    let windows_synced = verify_pair(session, windows_fixture, windows_fixture)?;

    expect_idle(session, &mut engine, windows_synced, 3)?;

    println!("[PASS] Automatic Windows -> macOS");
    println!("[PASS] Exact Unicode and formatting");
    println!("[PASS] Windows -> macOS feedback suppression");

    let mac_fixture = concat!(
        "macOS automatic synchronization\n",
        "Őrült programozás 🍎 🚀\r\n",
        "\tMore tabs\n",
        "    More indentation\n",
        "More trailing spaces  \n",
    );

    mac_ipc::write_text(session, mac_fixture)?;

    expect_step(
        session,
        &mut engine,
        Decision::Transfer(Direction::MacToWindows),
    )?;

    let mac_synced = verify_pair(session, mac_fixture, mac_fixture)?;

    expect_idle(session, &mut engine, mac_synced, 3)?;

    println!("[PASS] Automatic macOS -> Windows");
    println!("[PASS] macOS -> Windows feedback suppression");

    clipboard::write_text(mac_fixture)?;

    let identical_source = stable_counters(session)?;

    if identical_source.windows == mac_synced.windows {
        return Err("Repeated Windows copy did not advance sequence".into());
    }

    expect_step(
        session,
        &mut engine,
        Decision::Transfer(Direction::WindowsToMac),
    )?;

    let identical_result = verify_pair(session, mac_fixture, mac_fixture)?;

    if identical_result != identical_source {
        return Err("Identical Windows copy modified destination".into());
    }

    expect_idle(session, &mut engine, identical_result, 2)?;

    mac_ipc::write_text(session, mac_fixture)?;

    let identical_mac = stable_counters(session)?;

    if identical_mac.mac == identical_result.mac {
        return Err("Repeated macOS copy did not advance counter".into());
    }

    expect_step(
        session,
        &mut engine,
        Decision::Transfer(Direction::MacToWindows),
    )?;

    let identical_reverse = verify_pair(session, mac_fixture, mac_fixture)?;

    if identical_reverse != identical_mac {
        return Err("Identical macOS copy modified destination".into());
    }

    expect_idle(session, &mut engine, identical_reverse, 2)?;

    println!("[PASS] Bidirectional identical payload suppression");

    let windows_conflict = "Independent Windows conflicting copy";

    let mac_conflict = "Independent macOS conflicting copy";

    clipboard::write_text(windows_conflict)?;
    mac_ipc::write_text(session, mac_conflict)?;

    let conflicting = verify_pair(session, windows_conflict, mac_conflict)?;

    expect_step(session, &mut engine, Decision::Conflict)?;

    if verify_pair(session, windows_conflict, mac_conflict)? != conflicting {
        return Err("Conflict handling modified a clipboard".into());
    }

    expect_idle(session, &mut engine, conflicting, 3)?;

    println!("[PASS] Simultaneous clipboard changes");
    println!("[PASS] Both conflicting copies preserved");

    let recovery = "ReimsBridge conflict recovery 🚀";

    clipboard::write_text(recovery)?;

    expect_step(
        session,
        &mut engine,
        Decision::Transfer(Direction::WindowsToMac),
    )?;

    let recovered = verify_pair(session, recovery, recovery)?;

    expect_idle(session, &mut engine, recovered, 2)?;

    println!("[PASS] Automatic synchronization after conflict");

    let large_windows = "0123456789ABCDEF".repeat(crate::validation::MAX_CLIPBOARD_BYTES / 16);

    println!(
        "[ACCEPTANCE] Windows -> macOS maximum payload: {} bytes",
        large_windows.len(),
    );

    clipboard::write_text(&large_windows)?;

    let started = std::time::Instant::now();

    expect_step(
        session,
        &mut engine,
        Decision::Transfer(Direction::WindowsToMac),
    )?;

    println!(
        "[ACCEPTANCE] Synchronization elapsed: {:?}",
        started.elapsed(),
    );

    let large_windows_synced = verify_pair(session, &large_windows, &large_windows)?;

    expect_idle(session, &mut engine, large_windows_synced, 3)?;

    println!("[PASS] Automatic 1 MiB Windows -> macOS");

    let large_mac = "FEDCBA9876543210".repeat(crate::validation::MAX_CLIPBOARD_BYTES / 16);

    println!(
        "[ACCEPTANCE] macOS -> Windows maximum payload: {} bytes",
        large_mac.len(),
    );

    mac_ipc::write_text(session, &large_mac)?;

    let started = std::time::Instant::now();

    expect_step(
        session,
        &mut engine,
        Decision::Transfer(Direction::MacToWindows),
    )?;

    println!(
        "[ACCEPTANCE] Synchronization elapsed: {:?}",
        started.elapsed(),
    );

    let large_mac_synced = verify_pair(session, &large_mac, &large_mac)?;

    expect_idle(session, &mut engine, large_mac_synced, 3)?;

    println!("[PASS] Automatic 1 MiB macOS -> Windows");

    let final_fixture = "ReimsBridge M2.3D - PASS";

    mac_ipc::write_text(session, final_fixture)?;

    expect_step(
        session,
        &mut engine,
        Decision::Transfer(Direction::MacToWindows),
    )?;

    let final_counters = verify_pair(session, final_fixture, final_fixture)?;

    expect_idle(session, &mut engine, final_counters, 10)?;

    println!("[PASS] Recovery after maximum payload");
    println!("[PASS] Idle counters remain stable");
    println!();
    println!("[PASS] ALL AUTOMATIC SYNCHRONIZATION TESTS PASSED");

    Ok(())
}

pub fn run(session: &Session) -> Result<(), Box<dyn Error>> {
    println!("[WATCH] Press Ctrl+C to stop");
    run_until_stopped(session, &AtomicBool::new(false), || {})
}

pub fn run_until_stopped(
    session: &Session,
    stop: &AtomicBool,
    on_active: impl Fn(),
) -> Result<(), Box<dyn Error>> {
    let mut engine = SyncEngine::new();

    println!("[WATCH] Starting native clipboard synchronization");

    println!("[WATCH] Poll interval: {} ms", POLL_INTERVAL.as_millis());

    println!("[WATCH] Privacy-marked text is allowed");

    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }

    let watch_status = mac_ipc::get_status(session)?.watch_status_supported;
    if watch_status {
        println!("[WATCH] macOS watcher activity reporting enabled");
    }

    while !stop.load(Ordering::Relaxed) {
        if poll_once(session, &mut engine, watch_status)? == Some(Decision::Baseline) {
            on_active();
        }

        thread::sleep(POLL_INTERVAL);
    }

    Ok(())
}
