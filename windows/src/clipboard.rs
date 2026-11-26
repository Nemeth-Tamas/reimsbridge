use crate::validation::validate_text;
use clipboard_win::{formats, get_clipboard, set_clipboard};
use std::error::Error;

pub fn read_text() -> Result<String, Box<dyn Error>> {
    let text: String = get_clipboard(formats::Unicode)?;

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
