use crate::validation::MAX_CLIPBOARD_BYTES;
use crate::{clipboard, remote_clipboard};
use ssh2::Session;
use std::error::Error;

pub fn send(session: &Session) -> Result<(), Box<dyn Error>> {
    let text = clipboard::read_text()?;

    println!("[SYNC] Windows -> macOS");
    println!("[SYNC] Transferring {} bytes", text.len());

    remote_clipboard::write_text(session, &text)?;

    println!("[SYNC] Transfer successful");

    Ok(())
}

pub fn receive(session: &Session) -> Result<(), Box<dyn Error>> {
    println!("[SYNC] macOS -> Windows");

    let text = remote_clipboard::read_text(session)?;

    println!("[SYNC] Received {} bytes", text.len());

    clipboard::write_text(&text)?;

    println!("[SYNC] Transfer successful");

    Ok(())
}

pub fn roundtrip_test(session: &Session) -> Result<(), Box<dyn Error>> {
    const FIXTURE: &str = concat!(
        "ReimsBridge M1.3\r\n",
        "Árvíztűrő tükörfúrógép\r\n",
        "🚀 🦀 🍎\r\n",
        "\tTabbed code\r\n",
        "    Indented code\r\n",
        "'Single quotes' \"Double quotes\"\r\n",
        "$HOME $(whoami) `whoami`\r\n",
        "Trailing spaces  \r\n",
        "\r\n"
    );

    roundtrip_case(session, "Unicode and formatting", FIXTURE)?;

    roundtrip_case(session, "LF endings", "Line 1\nLine 2\n\n")?;

    roundtrip_case(
        session,
        "Mixed line endings",
        "Line 1\r\nLine 2\nLine 3\rLine 4\r\n",
    )?;

    roundtrip_case(session, "Whitespace only", " \t\r\n    ")?;

    roundtrip_case(session, "64 KiB", &"x".repeat(64 * 1024))?;

    roundtrip_case(session, "256 KiB", &"x".repeat(256 * 1024))?;

    roundtrip_case(
        session,
        "1 MiB maximum payload",
        &"x".repeat(MAX_CLIPBOARD_BYTES),
    )?;

    let oversized = "x".repeat(MAX_CLIPBOARD_BYTES + 1);

    if remote_clipboard::write_text(session, &oversized).is_ok() {
        return Err("Oversized upload was accepted".into());
    }

    if remote_clipboard::write_text(session, "").is_ok() {
        return Err("Empty upload was accepted".into());
    }

    if remote_clipboard::write_text(session, "a\0b").is_ok() {
        return Err("Embedded NUL upload was accepted".into());
    }

    let expected = "x".repeat(MAX_CLIPBOARD_BYTES);

    if remote_clipboard::read_text(session)? != expected {
        return Err("Rejected upload modified remote clipboard".into());
    }

    println!("[TEST] Invalid upload rejection: PASS");
    println!("[TEST] Remote clipboard preservation: PASS");
    println!("[TEST] All integration tests passed");

    Ok(())
}

fn roundtrip_case(session: &Session, name: &str, fixture: &str) -> Result<(), Box<dyn Error>> {
    println!();
    println!("[TEST] Case: {name}");
    println!("[TEST] Writing Windows clipboard");

    clipboard::write_text(fixture)?;

    let original = clipboard::read_text()?;

    if original.as_bytes() != fixture.as_bytes() {
        return Err("Initial Windows clipboard mismatch".into());
    }

    println!("[TEST] Windows clipboard: PASS");

    send(session)?;

    let remote = remote_clipboard::read_text(session)?;

    if remote.as_bytes() != original.as_bytes() {
        return Err("macOS clipboard content mismatch".into());
    }

    println!("[TEST] macOS clipboard: PASS");

    receive(session)?;

    let received = clipboard::read_text()?;

    if received.as_bytes() != original.as_bytes() {
        return Err("Final Windows clipboard mismatch".into());
    }

    println!("[TEST] Sent bytes: {}", original.len());
    println!("[TEST] Received bytes: {}", received.len());
    println!("[TEST] Exact bidirectional round-trip: PASS");

    Ok(())
}
