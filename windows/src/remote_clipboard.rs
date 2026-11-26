use crate::validation::{MAX_CLIPBOARD_BYTES, decode_text, validate_text};
use ssh2::{Channel, ExtendedData, Session};
use std::error::Error;
use std::io::{Read, Write};

const COPY_COMMAND: &str = r#"
export LC_ALL=en_US.UTF-8
/bin/bash -c 'IFS= read -r -d "" payload &&
    printf "%s" "$payload" | /usr/bin/pbcopy'
"#;

const PASTE_COMMAND: &str = "export LC_ALL=en_US.UTF-8; /usr/bin/pbpaste -Prefer txt";

fn open_channel(session: &Session, command: &str) -> Result<Channel, Box<dyn Error>> {
    let mut channel = session.channel_session()?;

    channel.handle_extended_data(ExtendedData::Ignore)?;
    channel.exec(command)?;

    Ok(channel)
}

fn read_reply(mut channel: Channel) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut output = Vec::new();

    (&mut channel)
        .take((MAX_CLIPBOARD_BYTES + 1) as u64)
        .read_to_end(&mut output)?;

    if output.len() > MAX_CLIPBOARD_BYTES {
        return Err("Remote clipboard exceeds size limit".into());
    }

    channel.wait_close()?;

    if channel.exit_status()? != 0 {
        return Err("Remote clipboard command failed".into());
    }

    if channel.exit_signal()?.exit_signal.is_some() {
        return Err("Remote clipboard command was terminated".into());
    }

    Ok(output)
}

pub fn write_text(session: &Session, text: &str) -> Result<(), Box<dyn Error>> {
    validate_text(text)?;

    let mut channel = open_channel(session, COPY_COMMAND)?;

    channel.write_all(text.as_bytes())?;
    channel.write_all(&[0])?;
    channel.flush()?;
    channel.send_eof()?;

    let output = read_reply(channel)?;

    if !output.is_empty() {
        return Err("Unexpected response from remote clipboard".into());
    }

    Ok(())
}

pub fn read_text(session: &Session) -> Result<String, Box<dyn Error>> {
    let mut channel = open_channel(session, PASTE_COMMAND)?;

    channel.send_eof()?;

    let output = read_reply(channel)?;

    decode_text(output)
}
