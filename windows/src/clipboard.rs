use crate::clipboard_bundle::{
    ClipboardBundle, MAX_IMAGE_PIXELS, MAX_PNG_BYTES, canonical_png, valid_rtf,
};
use crate::clipboard_image::{dib_to_png, png_to_dib};
use crate::sync_read::ReadOutcome;
use crate::validation::{MAX_CLIPBOARD_BYTES, validate_text};
use clipboard_win::{
    Clipboard, Getter, Setter, formats, get_clipboard, is_format_avail, options::NoClear, raw,
    register_format, seq_num,
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
    pub has_extended_content: bool,
    pub has_files: bool,
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
    let native_formats = ExtendedFormats::get()?;
    let has_files = is_format_avail(formats::CF_HDROP);
    let has_extended_content = !has_files
        && [
            native_formats.rtf,
            native_formats.html,
            native_formats.png,
            formats::CF_DIBV5,
            formats::CF_DIB,
        ]
        .into_iter()
        .any(is_format_avail);
    let is_excluded = has_excluded_format()?;

    let after = sequence_number()?;

    if before != after {
        return Ok(None);
    }

    Ok(Some(WindowsClipboardMetadata {
        sequence_number: after,
        has_plain_text,
        has_extended_content,
        has_files,
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

    Ok(ReadOutcome::Data(text))
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

struct ExtendedFormats {
    rtf: u32,
    html: u32,
    png: u32,
}

impl ExtendedFormats {
    fn get() -> Result<Self, Box<dyn Error>> {
        let id = |name| {
            register_format(name)
                .map(|format| format.get())
                .ok_or("Could not register extended Windows clipboard formats")
        };

        Ok(Self {
            rtf: id("Rich Text Format")?,
            html: id("HTML Format")?,
            png: id("PNG")?,
        })
    }
}

fn bounded_raw(format: u32, limit: usize) -> Vec<u8> {
    let mut data = Vec::new();
    if raw::size(format).is_some_and(|length| length.get() <= limit) {
        let _ = raw::get_vec(format, &mut data);
    }
    data
}

pub fn read_bundle(expected_sequence: u32) -> Result<ReadOutcome<ClipboardBundle>, Box<dyn Error>> {
    let native_formats = ExtendedFormats::get()?;
    let guard = Clipboard::new_attempts(10)?;
    if sequence_number()? != expected_sequence {
        return Ok(ReadOutcome::Changed);
    }
    if is_format_avail(formats::CF_HDROP) {
        return Ok(ReadOutcome::Unavailable);
    }

    let mut text = String::new();
    let text_size_limit = 2 * MAX_CLIPBOARD_BYTES + 2;
    if raw::size(formats::CF_UNICODETEXT).is_some_and(|size| size.get() <= text_size_limit)
        && (formats::Unicode.read_clipboard(&mut text).is_err() || validate_text(&text).is_err())
    {
        text.clear();
    }

    // Strip the Windows terminator and any allocation padding.
    let mut rtf = bounded_raw(native_formats.rtf, MAX_CLIPBOARD_BYTES + 64);
    rtf.truncate(
        rtf.iter()
            .rposition(|&byte| byte != 0)
            .map_or(0, |last| last + 1),
    );
    if !valid_rtf(&rtf) {
        rtf.clear();
    }

    let mut html_bytes = Vec::new();
    if raw::size(native_formats.html).is_some_and(|size| size.get() <= MAX_CLIPBOARD_BYTES + 4096)
        && raw::get_html(native_formats.html, &mut html_bytes).is_err()
    {
        html_bytes.clear();
    }
    let html = String::from_utf8(html_bytes)
        .ok()
        .filter(|text| validate_text(text).is_ok())
        .unwrap_or_default();

    let raw_png = bounded_raw(native_formats.png, MAX_PNG_BYTES);
    let png = canonical_png(&raw_png).ok().map(<[u8]>::to_vec);
    let dib = if png.is_none() {
        let limit = (4 * MAX_IMAGE_PIXELS) as usize + 65_536;
        let mut bitmap = bounded_raw(formats::CF_DIBV5, limit);
        if bitmap.is_empty() {
            bitmap = bounded_raw(formats::CF_DIB, limit);
        }
        bitmap
    } else {
        Vec::new()
    };
    if sequence_number()? != expected_sequence {
        return Ok(ReadOutcome::Changed);
    }
    drop(guard);

    let png = png.unwrap_or_else(|| {
        if dib.is_empty() {
            Vec::new()
        } else {
            dib_to_png(&dib).unwrap_or_default()
        }
    });
    let bundle = ClipboardBundle {
        text,
        rtf,
        html,
        png,
    };
    if bundle.has_extended_content() {
        Ok(ReadOutcome::Data(bundle))
    } else {
        Ok(ReadOutcome::Unavailable)
    }
}

pub fn write_bundle_checked(
    bundle: &ClipboardBundle,
    expected_sequence: u32,
) -> Result<Option<u32>, Box<dyn Error>> {
    let native_formats = ExtendedFormats::get()?;
    let dib = if bundle.png.is_empty() {
        None
    } else {
        Some(png_to_dib(&bundle.png)?)
    };
    let mut rtf = bundle.rtf.clone();
    if !rtf.is_empty() {
        rtf.push(0);
    }

    let guard = Clipboard::new_attempts(10)?;
    if sequence_number()? != expected_sequence {
        return Ok(None);
    }

    raw::empty()?;
    if !rtf.is_empty() {
        raw::set_without_clear(native_formats.rtf, &rtf)?;
    }
    if !bundle.html.is_empty() {
        raw::set_html_with(native_formats.html, &bundle.html, NoClear)?;
    }
    if let Some(dib) = dib {
        raw::set_without_clear(native_formats.png, &bundle.png)?;
        raw::set_without_clear(formats::CF_DIBV5, &dib)?;
    }
    if !bundle.text.is_empty() {
        raw::set_string_with(&bundle.text, NoClear)?;
    }
    drop(guard);

    let written_sequence = sequence_number()?;
    if !matches!(read_bundle(written_sequence)?, ReadOutcome::Data(received) if received == *bundle)
    {
        return Err("Windows clipboard bundle write could not be verified".into());
    }
    Ok(Some(written_sequence))
}

pub fn read_files(
    expected_sequence: u32,
) -> Result<ReadOutcome<Vec<std::path::PathBuf>>, Box<dyn Error>> {
    let _guard = Clipboard::new_attempts(10)?;
    if sequence_number()? != expected_sequence {
        return Ok(ReadOutcome::Changed);
    }
    if !is_format_avail(formats::CF_HDROP) {
        return Ok(ReadOutcome::Unavailable);
    }

    let mut paths = Vec::<std::path::PathBuf>::new();
    formats::FileList.read_clipboard(&mut paths)?;
    if sequence_number()? != expected_sequence {
        return Ok(ReadOutcome::Changed);
    }
    if paths.is_empty() {
        return Ok(ReadOutcome::Unavailable);
    }
    Ok(ReadOutcome::Data(paths))
}

pub fn write_files_checked(
    paths: &[std::path::PathBuf],
    expected_sequence: u32,
) -> Result<Option<u32>, Box<dyn Error>> {
    let encoded = paths
        .iter()
        .map(|path| path.to_str().ok_or("Clipboard file path is not Unicode"))
        .collect::<Result<Vec<_>, _>>()?;
    let preferred_effect = register_format("Preferred DropEffect")
        .ok_or("Cannot register the clipboard copy action")?
        .get();

    let guard = Clipboard::new_attempts(10)?;
    if sequence_number()? != expected_sequence {
        return Ok(None);
    }
    raw::empty()?;
    raw::set_file_list_with(&encoded, NoClear)?;
    raw::set_without_clear(preferred_effect, &1u32.to_le_bytes())?;
    drop(guard);

    let actual = sequence_number()?;
    if !matches!(read_files(actual)?, ReadOutcome::Data(paths_read) if paths_read == paths) {
        return Err("Windows file clipboard update could not be verified".into());
    }
    Ok(Some(actual))
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
