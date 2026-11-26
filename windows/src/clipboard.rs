use crate::sync_read::ReadOutcome;
use crate::validation::validate_text;
use clipboard_win::{
    Clipboard, Getter, Setter, formats, get_clipboard, is_format_avail, register_format, seq_num,
};
use std::error::Error;

const EXCLUDED_FORMATS: &[&str] = &[
    "ExcludeClipboardContentFromMonitorProcessing",
    "CanIncludeInClipboardHistory",
    "CanUploadToCloudClipboard",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsClipboardMetadata {
    pub sequence_number: u32,
    pub has_plain_text: bool,
    pub is_excluded: bool,
}

fn sequence_number() -> Result<u32, Box<dyn Error>> {
    seq_num()
        .map(|number| number.get())
        .ok_or_else(|| "Clipboard sequence number unavailable".into())
}

fn has_excluded_format() -> Result<bool, Box<dyn Error>> {
    for name in EXCLUDED_FORMATS {
        let format = register_format(name).ok_or("Failed to register clipboard privacy format")?;

        if is_format_avail(format.get()) {
            return Ok(true);
        }
    }

    Ok(false)
}

pub fn metadata_checked() -> Result<Option<WindowsClipboardMetadata>, Box<dyn Error>> {
    let before = sequence_number()?;

    let has_plain_text = is_format_avail(formats::CF_UNICODETEXT);
    let is_excluded = has_excluded_format()?;

    let after = sequence_number()?;

    if before != after {
        return Ok(None);
    }

    Ok(Some(WindowsClipboardMetadata {
        sequence_number: after,
        has_plain_text,
        is_excluded,
    }))
}

pub fn metadata() -> Result<WindowsClipboardMetadata, Box<dyn Error>> {
    metadata_checked()?.ok_or_else(|| "Clipboard changed during metadata inspection".into())
}

pub fn check_metadata() -> Result<(), Box<dyn Error>> {
    let metadata = metadata()?;

    println!("[CLIPBOARD] Native Windows metadata");
    println!("[CLIPBOARD] Sequence number: {}", metadata.sequence_number);
    println!("[CLIPBOARD] Has plain text: {}", metadata.has_plain_text);
    println!("[CLIPBOARD] Is excluded: {}", metadata.is_excluded);

    Ok(())
}

pub fn read_text() -> Result<String, Box<dyn Error>> {
    let before = metadata()?;

    if !before.has_plain_text {
        return Err("Clipboard does not contain plain text".into());
    }

    let text: String = get_clipboard(formats::Unicode)?;

    let after = metadata()?;

    if before.sequence_number != after.sequence_number {
        return Err("Clipboard changed during text retrieval".into());
    }

    validate_text(&text)?;

    Ok(text)
}

pub fn read_for_sync() -> Result<ReadOutcome, Box<dyn Error>> {
    let Some(before) = metadata_checked()? else {
        return Ok(ReadOutcome::Changed);
    };

    if !before.has_plain_text {
        return Ok(ReadOutcome::Unavailable);
    }

    let result = get_clipboard(formats::Unicode);

    let Some(after) = metadata_checked()? else {
        return Ok(ReadOutcome::Changed);
    };

    if before.sequence_number != after.sequence_number {
        return Ok(ReadOutcome::Changed);
    }

    let text: String = result?;

    if validate_text(&text).is_err() {
        return Ok(ReadOutcome::Unavailable);
    }

    Ok(ReadOutcome::Text(text))
}

fn verify_written_text(expected: &str) -> Result<u32, Box<dyn Error>> {
    let _guard = Clipboard::new_attempts(10)?;

    let before = sequence_number()?;

    let mut received = String::new();

    formats::Unicode.read_clipboard(&mut received)?;

    let after = sequence_number()?;

    if before != after {
        return Err("Clipboard changed during post-write verification".into());
    }

    if received.as_bytes() != expected.as_bytes() {
        return Err("Clipboard replaced before write verification".into());
    }

    Ok(after)
}

pub fn write_text_checked(
    text: &str,
    expected_sequence: Option<u32>,
) -> Result<Option<u32>, Box<dyn Error>> {
    validate_text(text)?;

    let guard = Clipboard::new_attempts(10)?;

    let before = sequence_number()?;

    if expected_sequence.is_some_and(|expected| expected != before) {
        return Ok(None);
    }

    formats::Unicode.write_clipboard(&text)?;

    drop(guard);

    let written_sequence = verify_written_text(text)?;

    Ok(Some(written_sequence))
}

pub fn write_text(text: &str) -> Result<(), Box<dyn Error>> {
    write_text_checked(text, None).map(|_| ())
}

pub fn check() -> Result<(), Box<dyn Error>> {
    let text = read_text()?;

    println!("[CLIPBOARD] Read successful");
    println!("[CLIPBOARD] UTF-8 bytes: {}", text.len());
    println!("[CLIPBOARD] Characters: {}", text.chars().count());

    Ok(())
}

pub fn self_test() -> Result<(), Box<dyn Error>> {
    const FIXTURE: &str = concat!(
        "ReimsBridge M1.2\r\n",
        "Árvíztűrő tükörfúrógép\r\n",
        "🚀\r\n",
        "    Indented code\r\n",
        "Trailing spaces  \r\n"
    );

    println!("[CLIPBOARD] Writing test fixture");

    write_text(FIXTURE)?;

    let received = read_text()?;

    if received.as_bytes() != FIXTURE.as_bytes() {
        return Err("Clipboard round-trip mismatch".into());
    }

    println!("[CLIPBOARD] Write successful");
    println!("[CLIPBOARD] Read successful");
    println!("[CLIPBOARD] Written bytes: {}", FIXTURE.len());
    println!("[CLIPBOARD] Received bytes: {}", received.len());
    println!("[CLIPBOARD] Exact Unicode round-trip: PASS");

    println!();
    println!("[CLIPBOARD] Testing conditional writes");

    const SECOND_FIXTURE: &str = "ReimsBridge conditional write test 🚀\r\n";

    let expected_sequence = sequence_number()?;

    let written_sequence = write_text_checked(SECOND_FIXTURE, Some(expected_sequence))?
        .ok_or("Current-sequence write was unexpectedly rejected")?;

    let observed_sequence = sequence_number()?;

    println!("[CLIPBOARD] Initial sequence: {expected_sequence}");
    println!("[CLIPBOARD] Written sequence: {written_sequence}");
    println!("[CLIPBOARD] Observed sequence: {observed_sequence}");

    if written_sequence == expected_sequence {
        return Err("Clipboard write did not advance sequence".into());
    }

    if written_sequence != observed_sequence {
        return Err("Clipboard sequence changed after lock release".into());
    }

    println!("[CLIPBOARD] Current-sequence write: PASS");
    println!("[CLIPBOARD] Write sequence attribution: PASS");

    let before_rejection = sequence_number()?;

    let rejected = write_text_checked(
        "This must never reach the clipboard",
        Some(expected_sequence),
    )?;

    if rejected.is_some() {
        return Err("Stale conditional write was accepted".into());
    }

    if sequence_number()? != before_rejection {
        return Err("Rejected write modified clipboard sequence".into());
    }

    println!("[CLIPBOARD] Stale conditional write rejected: PASS");

    let received = read_text()?;

    if received.as_bytes() != SECOND_FIXTURE.as_bytes() {
        return Err("Rejected write modified clipboard contents".into());
    }

    println!("[CLIPBOARD] Destination preservation: PASS");

    let before_invalid = sequence_number()?;

    for invalid in ["", "a\0b"] {
        if write_text_checked(invalid, None).is_ok() {
            return Err("Invalid conditional write was accepted".into());
        }
    }

    if sequence_number()? != before_invalid {
        return Err("Invalid write modified clipboard sequence".into());
    }

    let received = read_text()?;

    if received.as_bytes() != SECOND_FIXTURE.as_bytes() {
        return Err("Invalid write modified clipboard contents".into());
    }

    println!("[CLIPBOARD] Invalid write preservation: PASS");

    const REPLACEMENT_FIXTURE: &str = "Simulated external clipboard replacement";

    write_text(REPLACEMENT_FIXTURE)?;

    let replacement_sequence = sequence_number()?;

    if verify_written_text(SECOND_FIXTURE).is_ok() {
        return Err("Replaced clipboard incorrectly attributed to previous write".into());
    }

    println!("[CLIPBOARD] External replacement detected: PASS");

    if sequence_number()? != replacement_sequence {
        return Err("Post-write verification modified clipboard sequence".into());
    }

    let received = read_text()?;

    if received.as_bytes() != REPLACEMENT_FIXTURE.as_bytes() {
        return Err("Post-write verification modified external content".into());
    }

    println!("[CLIPBOARD] External replacement preserved: PASS");
    println!("[CLIPBOARD] All counter-aware write tests: PASS");

    Ok(())
}
