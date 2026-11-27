use crate::clipboard_bundle::ClipboardBundle;
use crate::clipboard_image::rgba_to_png;
use crate::sync_engine::{
    Counters, Decision, Direction, PayloadDecision, Snapshot, SyncEngine, inspect_payload,
    still_current,
};
use crate::sync_read::ReadOutcome;
use crate::{clipboard, file_transfer, mac_ipc, ssh};

use ssh2::Session;
use std::error::Error;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Default)]
struct WatchOptions<'a> {
    watch_status: bool,
    transport: mac_ipc::TextTransport,
    bundles: bool,
    files: bool,
    stop: Option<&'a AtomicBool>,
    file_notice: Option<&'a dyn Fn(&str)>,
}

fn snapshot(
    session: &Session,
    options: WatchOptions<'_>,
) -> Result<Option<Snapshot>, Box<dyn Error>> {
    let Some(windows_before) = clipboard::metadata_checked()? else {
        return Ok(None);
    };

    let mac = if options.watch_status {
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
        mac_has_text: mac.has_text_for(options.transport),
        windows_has_bundle: options.bundles && windows_after.has_extended_content,
        mac_has_bundle: options.bundles && mac.has_bundle(),
        windows_has_files: options.files && windows_after.has_files,
        mac_has_files: options.files && mac.has_file_urls,
    }))
}

fn counters_current(session: &Session, expected: Counters) -> Result<bool, Box<dyn Error>> {
    Ok(snapshot(session, WatchOptions::default())?
        .is_some_and(|actual| still_current(expected, actual.counters)))
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

fn transfer_bundle(
    session: &Session,
    engine: &mut SyncEngine,
    observed: Snapshot,
    direction: Direction,
) -> Result<bool, Box<dyn Error>> {
    let counters = observed.counters;
    let source = match direction {
        Direction::WindowsToMac if observed.windows_has_bundle => {
            clipboard::read_bundle(counters.windows)?
        }
        Direction::MacToWindows if observed.mac_has_bundle => {
            mac_ipc::read_bundle(session, counters.mac)?
        }
        _ => return Ok(false),
    };
    let bundle = match source {
        ReadOutcome::Data(bundle) => bundle,
        ReadOutcome::Unavailable => return Ok(false),
        ReadOutcome::Changed => return Ok(true),
    };
    let destination = match direction {
        Direction::WindowsToMac if observed.mac_has_bundle => {
            mac_ipc::read_bundle(session, counters.mac)?
        }
        Direction::MacToWindows if observed.windows_has_bundle => {
            clipboard::read_bundle(counters.windows)?
        }
        _ => ReadOutcome::Unavailable,
    };
    match destination {
        ReadOutcome::Data(current) if current == bundle => {
            skip_if_current(session, engine, counters)?;
            return Ok(true);
        }
        ReadOutcome::Changed => return Ok(true),
        _ => {}
    }
    if !counters_current(session, counters)? {
        return Ok(true);
    }

    match direction {
        Direction::WindowsToMac => {
            if let Some(counter) = mac_ipc::write_bundle_checked(session, &bundle, counters.mac)? {
                engine.record_mac_write(counters, counter);
                println!(
                    "[WATCH] Windows -> macOS clipboard bundle: {} bytes",
                    bundle.byte_len()
                );
            }
        }
        Direction::MacToWindows => {
            if let Some(sequence) = clipboard::write_bundle_checked(&bundle, counters.windows)? {
                engine.record_windows_write(counters, sequence);
                println!(
                    "[WATCH] macOS -> Windows clipboard bundle: {} bytes",
                    bundle.byte_len()
                );
            }
        }
    }

    Ok(true)
}

fn transfer(
    session: &Session,
    engine: &mut SyncEngine,
    observed: Snapshot,
    direction: Direction,
    transport: mac_ipc::TextTransport,
) -> Result<(), Box<dyn Error>> {
    if transfer_bundle(session, engine, observed, direction)? {
        return Ok(());
    }

    let source = match direction {
        Direction::WindowsToMac => clipboard::read_for_sync()?,

        Direction::MacToWindows => {
            mac_ipc::read_for_sync(session, transport, observed.counters.mac)?
        }
    };

    let source = match source {
        ReadOutcome::Data(text) => text,

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
        Direction::WindowsToMac if observed.mac_has_text => {
            mac_ipc::read_for_sync(session, transport, observed.counters.mac)?
        }

        Direction::MacToWindows if observed.windows_has_text => clipboard::read_for_sync()?,

        _ => ReadOutcome::Unavailable,
    };

    let destination = match destination {
        ReadOutcome::Data(text) => Some(text),

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
            let written =
                mac_ipc::write_for_sync(session, transport, &source, observed.counters.mac)?;

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
    options: WatchOptions<'_>,
) -> Result<Option<Decision>, Box<dyn Error>> {
    let Some(observed) = snapshot(session, options)? else {
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
            let has_files = match direction {
                Direction::WindowsToMac => observed.windows_has_files,
                Direction::MacToWindows => observed.mac_has_files,
            };
            if has_files {
                transfer_files(session, engine, observed.counters, direction, options)?;
            } else {
                transfer(session, engine, observed, direction, options.transport)?;
            }
        }
    }

    Ok(Some(decision))
}

fn transfer_files(
    session: &Session,
    engine: &mut SyncEngine,
    counters: Counters,
    direction: Direction,
    options: WatchOptions<'_>,
) -> Result<(), Box<dyn Error>> {
    use file_transfer::ResultState;

    let result = file_transfer::transfer(
        session,
        direction,
        counters.windows,
        counters.mac,
        file_transfer::TransferOptions {
            stop: options.stop,
            notice: options.file_notice,
        },
        || {
            let status = WatchOptions {
                watch_status: options.watch_status,
                ..WatchOptions::default()
            };
            Ok(snapshot(session, status)?
                .is_some_and(|observed| still_current(counters, observed.counters)))
        },
    );
    let notice = |message: &str| {
        println!("[FILES] {message}");
        if let Some(notify) = options.file_notice {
            notify(message);
        }
    };
    match result {
        Ok(ResultState::MacPublished(counter)) => engine.record_mac_write(counters, counter),
        Ok(ResultState::WindowsPublished(sequence)) => {
            engine.record_windows_write(counters, sequence)
        }
        Ok(ResultState::Changed) => notice("Clipboard changed during file transfer"),
        Ok(ResultState::Unavailable) => skip_if_current(session, engine, counters)?,
        Err(error) if error.is::<file_transfer::SelectionChanged>() => {
            notice("Clipboard changed during file transfer");
        }
        Err(error) => {
            if options
                .stop
                .is_some_and(|stop| stop.load(Ordering::Relaxed))
            {
                notice("File transfer stopped");
            } else if ssh::is_transport_error(error.as_ref())
                || mac_ipc::is_unavailable(error.as_ref())
            {
                return Err(error);
            } else {
                notice(&format!("File transfer failed: {error}"));
                skip_if_current(session, engine, counters)?;
            }
        }
    }

    Ok(())
}

fn stable_counters(session: &Session) -> Result<Counters, Box<dyn Error>> {
    for _ in 0..10 {
        if let Some(observed) = snapshot(session, WatchOptions::default())? {
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
    options: WatchOptions<'_>,
    expected: Decision,
) -> Result<(), Box<dyn Error>> {
    let actual = poll_once(session, engine, options)?;

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
    options: WatchOptions<'_>,
    expected: Counters,
    iterations: usize,
) -> Result<(), Box<dyn Error>> {
    for _ in 0..iterations {
        thread::sleep(POLL_INTERVAL);

        expect_step(session, engine, options, Decision::Idle)?;

        if stable_counters(session)? != expected {
            return Err("Clipboard counters changed while idle".into());
        }
    }

    Ok(())
}

pub fn test(session: &Session) -> Result<(), Box<dyn Error>> {
    let mut engine = SyncEngine::new();
    let status = mac_ipc::get_status(session)?;
    let options = WatchOptions {
        transport: status.text_transport(),
        bundles: status.supports_bundles(),
        files: status.file_transfer_version == 1,
        ..WatchOptions::default()
    };

    println!("[ACCEPTANCE] M2.3D automatic synchronization");
    println!(
        "[ACCEPTANCE] Clipboard framing: {}",
        options.transport.description()
    );
    println!("[ACCEPTANCE] Both clipboards will be modified");

    let windows_initial = "Independent Windows startup clipboard";

    let mac_initial = "Independent macOS startup clipboard";

    clipboard::write_text(windows_initial)?;
    mac_ipc::write_text(session, mac_initial)?;

    let initial = verify_pair(session, windows_initial, mac_initial)?;
    if options.transport == mac_ipc::TextTransport::NamedV1 {
        let stale_count = initial.mac.wrapping_add(1);
        let stale_read = mac_ipc::read_for_sync(session, options.transport, stale_count)?;
        let stale_write = mac_ipc::write_for_sync(
            session,
            options.transport,
            "Stale format write fixture",
            stale_count,
        )?;

        if !matches!(stale_read, ReadOutcome::Changed)
            || stale_write.is_some()
            || verify_pair(session, windows_initial, mac_initial)? != initial
        {
            return Err("Named format counter guards did not preserve the clipboards".into());
        }

        println!("[PASS] Named format stale read and write were rejected");
    }

    expect_step(session, &mut engine, options, Decision::Baseline)?;

    if verify_pair(session, windows_initial, mac_initial)? != initial {
        return Err("Baseline modified a clipboard".into());
    }

    expect_idle(session, &mut engine, options, initial, 3)?;

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
        options,
        Decision::Transfer(Direction::WindowsToMac),
    )?;

    let windows_synced = verify_pair(session, windows_fixture, windows_fixture)?;

    expect_idle(session, &mut engine, options, windows_synced, 3)?;

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
        options,
        Decision::Transfer(Direction::MacToWindows),
    )?;

    let mac_synced = verify_pair(session, mac_fixture, mac_fixture)?;

    expect_idle(session, &mut engine, options, mac_synced, 3)?;

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
        options,
        Decision::Transfer(Direction::WindowsToMac),
    )?;

    let identical_result = verify_pair(session, mac_fixture, mac_fixture)?;

    if identical_result != identical_source {
        return Err("Identical Windows copy modified destination".into());
    }

    expect_idle(session, &mut engine, options, identical_result, 2)?;

    mac_ipc::write_text(session, mac_fixture)?;

    let identical_mac = stable_counters(session)?;

    if identical_mac.mac == identical_result.mac {
        return Err("Repeated macOS copy did not advance counter".into());
    }

    expect_step(
        session,
        &mut engine,
        options,
        Decision::Transfer(Direction::MacToWindows),
    )?;

    let identical_reverse = verify_pair(session, mac_fixture, mac_fixture)?;

    if identical_reverse != identical_mac {
        return Err("Identical macOS copy modified destination".into());
    }

    expect_idle(session, &mut engine, options, identical_reverse, 2)?;

    println!("[PASS] Bidirectional identical payload suppression");

    let windows_conflict = "Independent Windows conflicting copy";

    let mac_conflict = "Independent macOS conflicting copy";

    clipboard::write_text(windows_conflict)?;
    mac_ipc::write_text(session, mac_conflict)?;

    let conflicting = verify_pair(session, windows_conflict, mac_conflict)?;

    expect_step(session, &mut engine, options, Decision::Conflict)?;

    if verify_pair(session, windows_conflict, mac_conflict)? != conflicting {
        return Err("Conflict handling modified a clipboard".into());
    }

    expect_idle(session, &mut engine, options, conflicting, 3)?;

    println!("[PASS] Simultaneous clipboard changes");
    println!("[PASS] Both conflicting copies preserved");

    let recovery = "ReimsBridge conflict recovery 🚀";

    clipboard::write_text(recovery)?;

    expect_step(
        session,
        &mut engine,
        options,
        Decision::Transfer(Direction::WindowsToMac),
    )?;

    let recovered = verify_pair(session, recovery, recovery)?;

    expect_idle(session, &mut engine, options, recovered, 2)?;

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
        options,
        Decision::Transfer(Direction::WindowsToMac),
    )?;

    println!(
        "[ACCEPTANCE] Synchronization elapsed: {:?}",
        started.elapsed(),
    );

    let large_windows_synced = verify_pair(session, &large_windows, &large_windows)?;

    expect_idle(session, &mut engine, options, large_windows_synced, 3)?;

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
        options,
        Decision::Transfer(Direction::MacToWindows),
    )?;

    println!(
        "[ACCEPTANCE] Synchronization elapsed: {:?}",
        started.elapsed(),
    );

    let large_mac_synced = verify_pair(session, &large_mac, &large_mac)?;

    expect_idle(session, &mut engine, options, large_mac_synced, 3)?;

    println!("[PASS] Automatic 1 MiB macOS -> Windows");

    let final_fixture = "ReimsBridge M2.3D - PASS";

    mac_ipc::write_text(session, final_fixture)?;

    expect_step(
        session,
        &mut engine,
        options,
        Decision::Transfer(Direction::MacToWindows),
    )?;

    let final_counters = verify_pair(session, final_fixture, final_fixture)?;

    expect_idle(session, &mut engine, options, final_counters, 10)?;

    println!("[PASS] Recovery after maximum payload");
    println!("[PASS] Idle counters remain stable");

    if options.bundles {
        test_bundles(session, &mut engine, options)?;
    }
    if options.files {
        test_files(session, &mut engine, options)?;
    }
    println!();
    println!("[PASS] ALL AUTOMATIC SYNCHRONIZATION TESTS PASSED");

    Ok(())
}

fn test_files(
    session: &Session,
    engine: &mut SyncEngine,
    options: WatchOptions<'_>,
) -> Result<(), Box<dyn Error>> {
    use std::fs;
    use std::io::Read;
    use std::path::Path;

    let fixture =
        std::env::temp_dir().join(format!("ReimsBridge-test-{}", file_transfer::batch_id()?));
    fs::create_dir(&fixture)?;
    let directory_name = "Reims folder é";
    let file_name = "Reims standalone.bin";
    let directory = fixture.join(directory_name);
    let standalone = fixture.join(file_name);
    fs::create_dir(&directory)?;
    fs::create_dir(directory.join("empty"))?;
    let nested = directory.join("nested.bin");
    let bytes: Vec<u8> = (0..800_001).map(|index| (index % 251) as u8).collect();
    fs::write(&nested, &bytes)?;
    fs::write(&standalone, b"ReimsBridge file selection")?;

    let before = stable_counters(session)?;
    clipboard::write_files_checked(&[directory, standalone], before.windows)?
        .ok_or("Windows clipboard changed before file fixture publication")?;
    expect_step(
        session,
        engine,
        options,
        Decision::Transfer(Direction::WindowsToMac),
    )?;
    let uploaded = stable_counters(session)?;
    let ReadOutcome::Data(paths) = mac_ipc::file_offer(session, uploaded.mac)? else {
        return Err("Mac clipboard is missing the uploaded file selection".into());
    };
    if paths.len() != 2 {
        return Err("Mac clipboard did not retain both selection roots".into());
    }
    let parent = paths[0]
        .rsplit_once('/')
        .ok_or("Invalid staged Mac path")?
        .0;
    let batch = parent.rsplit('/').next().ok_or("Invalid Mac batch name")?;
    let sftp = session.sftp()?;
    let remote_dir = format!("{parent}/{directory_name}");
    let remote_file = format!("{parent}/{file_name}");
    let mut received = Vec::new();
    sftp.open(Path::new(&format!("{remote_dir}/nested.bin")))?
        .take(bytes.len() as u64 + 1)
        .read_to_end(&mut received)?;
    let remote_marker = sftp.lstat(Path::new(&remote_file))?;
    if received != bytes
        || remote_marker.size != Some(b"ReimsBridge file selection".len() as u64)
        || !sftp
            .lstat(Path::new(&format!("{remote_dir}/empty")))?
            .is_dir()
    {
        return Err("Windows to Mac file fixture did not match".into());
    }
    expect_idle(session, engine, options, uploaded, 2)?;
    println!("[PASS] Windows to macOS files, multiple roots, and nested directory");

    mac_ipc::file_publish(session, uploaded.mac, batch)?
        .ok_or("Mac clipboard changed before file fixture publication")?;
    expect_step(
        session,
        engine,
        options,
        Decision::Transfer(Direction::MacToWindows),
    )?;
    let downloaded = stable_counters(session)?;
    let ReadOutcome::Data(paths) = clipboard::read_files(downloaded.windows)? else {
        return Err("Windows clipboard is missing the downloaded file selection".into());
    };
    let folder = paths
        .iter()
        .find(|path| path.file_name().is_some_and(|name| name == directory_name))
        .ok_or("Windows clipboard is missing the staged folder")?;
    let file = paths
        .iter()
        .find(|path| path.file_name().is_some_and(|name| name == file_name))
        .ok_or("Windows clipboard is missing the staged standalone file")?;
    if paths.len() != 2
        || !folder.join("empty").is_dir()
        || fs::read(folder.join("nested.bin"))? != bytes
        || fs::read(file)? != b"ReimsBridge file selection"
    {
        return Err("Mac to Windows file fixture did not match".into());
    }
    expect_idle(session, engine, options, downloaded, 2)?;
    // Both clipboard selections now reference application-owned staging copies.
    fs::remove_dir_all(fixture)?;
    println!("[PASS] macOS to Windows files, multiple roots, and nested directory");
    Ok(())
}

fn test_bundles(
    session: &Session,
    engine: &mut SyncEngine,
    options: WatchOptions<'_>,
) -> Result<(), Box<dyn Error>> {
    for (direction, include_rich_text) in [
        (Direction::WindowsToMac, true),
        (Direction::MacToWindows, true),
        (Direction::WindowsToMac, false),
        (Direction::MacToWindows, false),
    ] {
        let color = match direction {
            Direction::WindowsToMac => [255, 0, 64, 128],
            Direction::MacToWindows => [0, 255, 127, 64],
        };
        let image = image::RgbaImage::from_pixel(2, 2, image::Rgba(color));
        let mut bundle = ClipboardBundle {
            png: rgba_to_png(&image)?,
            ..ClipboardBundle::default()
        };
        if include_rich_text {
            bundle.text = format!("Clipboard bundle fixture {direction:?}");
            bundle.rtf = format!("{{\\rtf1\\ansi \\b Bundle {direction:?}\\b0}}").into_bytes();
            bundle.html = format!("<strong>Bundle {direction:?}</strong>");
        }
        let before = stable_counters(session)?;
        let written = match direction {
            Direction::WindowsToMac => {
                clipboard::write_bundle_checked(&bundle, before.windows)?.is_some()
            }
            Direction::MacToWindows => {
                mac_ipc::write_bundle_checked(session, &bundle, before.mac)?.is_some()
            }
        };
        if !written {
            return Err("Clipboard changed while preparing bundle fixture".into());
        }

        expect_step(session, engine, options, Decision::Transfer(direction))?;
        let after = stable_counters(session)?;
        if !matches!(clipboard::read_bundle(after.windows)?, ReadOutcome::Data(value) if value == bundle)
            || !matches!(mac_ipc::read_bundle(session, after.mac)?, ReadOutcome::Data(value) if value == bundle)
        {
            return Err("Clipboard bundle changed across the native transfer".into());
        }
        expect_idle(session, engine, options, after, 2)?;
        let formats = if include_rich_text {
            "RTF, HTML, plain text, and transparent PNG bundle"
        } else {
            "Transparent PNG without text"
        };
        println!("[PASS] {formats}: {direction:?}");
    }
    Ok(())
}

pub fn run(session: &Session) -> Result<(), Box<dyn Error>> {
    println!("[WATCH] Press Ctrl+C to stop");
    run_until_stopped(session, &AtomicBool::new(false), || {}, |_| {})
}

pub fn run_until_stopped(
    session: &Session,
    stop: &AtomicBool,
    mut on_active: impl FnMut(),
    file_notice: impl Fn(&str),
) -> Result<(), Box<dyn Error>> {
    let mut engine = SyncEngine::new();

    println!("[WATCH] Starting native clipboard synchronization");

    println!("[WATCH] Poll interval: {} ms", POLL_INTERVAL.as_millis());

    println!("[WATCH] Privacy-marked text is allowed");

    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }

    let status = mac_ipc::get_status(session)?;
    let options = WatchOptions {
        watch_status: status.watch_status_supported,
        transport: status.text_transport(),
        bundles: status.supports_bundles(),
        files: status.file_transfer_version == 1,
        stop: Some(stop),
        file_notice: Some(&file_notice),
    };
    println!(
        "[WATCH] Clipboard framing: {}",
        options.transport.description()
    );

    if options.bundles {
        println!("[WATCH] Native rich text and image bundles enabled");
    }

    if options.files {
        println!("[WATCH] Streaming file and folder copies enabled");
    }

    if options.watch_status {
        println!("[WATCH] macOS watcher activity reporting enabled");
    }

    while !stop.load(Ordering::Relaxed) {
        if poll_once(session, &mut engine, options)? == Some(Decision::Baseline) {
            on_active();
        }

        thread::sleep(POLL_INTERVAL);
    }

    Ok(())
}
