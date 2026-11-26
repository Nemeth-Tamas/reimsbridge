use crate::validation::validate_text;
use clipboard_win::{
    formats, get_clipboard, is_format_avail, register_format, seq_num, set_clipboard,
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

pub fn metadata() -> Result<WindowsClipboardMetadata, Box<dyn Error>> {
    let before = sequence_number()?;

    let has_plain_text = is_format_avail(formats::CF_UNICODETEXT);
    let is_excluded = has_excluded_format()?;

    let after = sequence_number()?;

    if before != after {
        return Err("Clipboard changed during metadata inspection".into());
    }

    Ok(WindowsClipboardMetadata {
        sequence_number: after,
        has_plain_text,
        is_excluded,
    })
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

    if before.is_excluded {
        return Err("Clipboard content is excluded".into());
    }

    if !before.has_plain_text {
        return Err("Clipboard does not contain plain text".into());
    }

    let text: String = get_clipboard(formats::Unicode)?;

    let after = metadata()?;

    if before.sequence_number != after.sequence_number {
        return Err("Clipboard changed during text retrieval".into());
    }

    if after.is_excluded {
        return Err("Clipboard content is excluded".into());
    }

    validate_text(&text)?;

    Ok(text)
}

pub fn write_text(text: &str) -> Result<(), Box<dyn Error>> {
    validate_text(text)?;

    set_clipboard(formats::Unicode, text)?;

    Ok(())
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

    Ok(())
}
