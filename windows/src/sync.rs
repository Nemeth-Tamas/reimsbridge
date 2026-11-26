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

    println!("[TEST] Writing Windows clipboard");

    clipboard::write_text(FIXTURE)?;

    let original = clipboard::read_text()?;

    if original.as_bytes() != FIXTURE.as_bytes() {
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
