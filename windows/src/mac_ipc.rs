use serde::Deserialize;
use ssh2::{ExtendedData, Session};
use std::error::Error;
use std::fmt;
use std::io::{Read, Write};

use crate::clipboard_bundle::{ClipboardBundle, MAX_BUNDLE_BYTES, TOKEN as BUNDLE_TOKEN};
use crate::sync_read::ReadOutcome;
use crate::{clipboard, validation};

const PROTOCOL_VERSION: u32 = 2;
const MAX_RESPONSE_BYTES: usize = 4096;
const MAX_HEADER_BYTES: usize = 64;

const FORMAT_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipboardFormat {
    Utf8Text,
    Bundle,
}

impl ClipboardFormat {
    fn token(self) -> &'static str {
        match self {
            Self::Utf8Text => "text-utf8",
            Self::Bundle => BUNDLE_TOKEN,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextTransport {
    #[default]
    Legacy,
    NamedV1,
}

impl TextTransport {
    fn format(self) -> Option<ClipboardFormat> {
        match self {
            Self::Legacy => None,
            Self::NamedV1 => Some(ClipboardFormat::Utf8Text),
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Legacy => "legacy text",
            Self::NamedV1 => "named formats v1 (text-utf8)",
        }
    }
}

#[derive(Debug, Deserialize)]
struct FormatStatus {
    version: u32,
    #[serde(default)]
    supported: Vec<String>,
    #[serde(default)]
    available: Vec<String>,
}

const IPC_COMMAND: &str = r#"/usr/bin/nc -U -w 3 "$HOME/.reimsbridge/status.sock""#;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacStatus {
    pub protocol: u32,
    pub status: String,
    pub change_count: i64,
    pub has_plain_text: bool,
    pub is_excluded: bool,
    #[serde(default)]
    pub watch_status_supported: bool,
    #[serde(default)]
    pub file_transfer_version: u32,
    #[serde(default, rename = "hasFileURLs", alias = "hasFileUrls")]
    pub has_file_urls: bool,
    #[serde(default)]
    formats: Option<FormatStatus>,
}

impl MacStatus {
    pub fn supports_bundles(&self) -> bool {
        self.formats.as_ref().is_some_and(|formats| {
            formats.version == FORMAT_PROTOCOL_VERSION
                && formats.supported.iter().any(|token| token == BUNDLE_TOKEN)
        })
    }

    pub fn has_bundle(&self) -> bool {
        self.supports_bundles()
            && self
                .formats
                .as_ref()
                .is_some_and(|formats| formats.available.iter().any(|token| token == BUNDLE_TOKEN))
    }

    pub fn text_transport(&self) -> TextTransport {
        let supported = self.formats.as_ref().is_some_and(|formats| {
            formats.version == FORMAT_PROTOCOL_VERSION
                && formats
                    .supported
                    .iter()
                    .any(|token| token == ClipboardFormat::Utf8Text.token())
        });
        if supported {
            TextTransport::NamedV1
        } else {
            TextTransport::Legacy
        }
    }

    pub fn has_text_for(&self, transport: TextTransport) -> bool {
        match transport.format() {
            Some(format) => self.formats.as_ref().is_some_and(|formats| {
                formats.version == FORMAT_PROTOCOL_VERSION
                    && formats
                        .available
                        .iter()
                        .any(|token| token == format.token())
            }),
            None => self.has_plain_text,
        }
    }
}

fn parse_status(response: &[u8]) -> Result<MacStatus, Box<dyn Error>> {
    if response.len() > MAX_RESPONSE_BYTES {
        return Err("IPC response exceeds size limit".into());
    }

    let status: MacStatus =
        serde_json::from_slice(response).map_err(|_| "Invalid macOS IPC response")?;

    if status.protocol != PROTOCOL_VERSION {
        return Err("Unsupported macOS IPC protocol version".into());
    }

    if status.status != "ok" {
        return Err("macOS IPC returned an error".into());
    }

    Ok(status)
}

#[derive(Debug)]
struct Unavailable(String);

impl fmt::Display for Unavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Unavailable {}

pub fn is_unavailable(error: &(dyn Error + 'static)) -> bool {
    error.is::<Unavailable>()
}

fn query(
    session: &Session,
    request: &[u8],
    max_response_bytes: usize,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut channel = session.channel_session()?;

    channel.handle_extended_data(ExtendedData::Ignore)?;
    channel.exec(IPC_COMMAND)?;

    channel
        .write_all(request)
        .and_then(|()| channel.flush())
        .map_err(|error| Unavailable(error.to_string()))?;
    channel.send_eof()?;

    let mut response = Vec::new();

    (&mut channel)
        .take((max_response_bytes + 1) as u64)
        .read_to_end(&mut response)
        .map_err(|error| Unavailable(error.to_string()))?;

    if response.len() > max_response_bytes {
        return Err("IPC response exceeds size limit".into());
    }

    channel.wait_close()?;

    if channel.exit_status()? != 0 {
        return Err(Unavailable(
            "Cannot reach macOS IPC service. Is ReimsBridge running with IPC enabled?".into(),
        )
        .into());
    }

    if channel.exit_signal()?.exit_signal.is_some() {
        return Err(Unavailable("macOS IPC command was terminated".into()).into());
    }

    if response.is_empty() {
        return Err(Unavailable("macOS IPC returned no response".into()).into());
    }

    Ok(response)
}

pub fn get_status(session: &Session) -> Result<MacStatus, Box<dyn Error>> {
    let response = query(session, b"status\n", MAX_RESPONSE_BYTES)?;

    parse_status(&response)
}

pub fn get_watch_status(session: &Session) -> Result<MacStatus, Box<dyn Error>> {
    let response = query(session, b"watch-status\n", MAX_RESPONSE_BYTES)?;

    parse_status(&response)
}

fn format_value(value: &[u8], format: Option<ClipboardFormat>) -> Result<&[u8], Box<dyn Error>> {
    match format {
        None => Ok(value),
        Some(format) => value
            .strip_prefix(format.token().as_bytes())
            .and_then(|value| value.strip_prefix(b" "))
            .ok_or_else(|| "IPC response format mismatch".into()),
    }
}

fn parse_read(response: Vec<u8>) -> Result<String, Box<dyn Error>> {
    parse_format_read(response, None)
}

fn parse_format_read(
    response: Vec<u8>,
    format: Option<ClipboardFormat>,
) -> Result<String, Box<dyn Error>> {
    validation::decode_text(parse_payload_read(
        response,
        format,
        validation::MAX_CLIPBOARD_BYTES,
    )?)
}

fn parse_payload_read(
    mut response: Vec<u8>,
    format: Option<ClipboardFormat>,
    max_payload: usize,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let header_end = response
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or("Missing IPC response header")?;

    if header_end >= MAX_HEADER_BYTES {
        return Err("IPC response header exceeds size limit".into());
    }

    let header =
        std::str::from_utf8(&response[..header_end]).map_err(|_| "Invalid IPC response header")?;

    if header.starts_with("ERR ") {
        let message = match header {
            "ERR NO_TEXT" => "macOS clipboard contains no plain text",

            "ERR EXCLUDED" => "macOS clipboard is marked as excluded",

            "ERR OVERSIZED" => "macOS clipboard exceeds size limit",

            "ERR INVALID_TEXT" => "macOS clipboard contains invalid text",

            "ERR CLIPBOARD_CHANGED" => "macOS clipboard changed during read",

            _ => "Native macOS clipboard read failed",
        };

        return Err(message.into());
    }

    let value = header
        .strip_prefix("OK ")
        .ok_or("Invalid IPC response status")?;
    let declared_size = std::str::from_utf8(format_value(value.as_bytes(), format)?)?
        .parse::<usize>()
        .map_err(|_| "Invalid IPC payload length")?;

    if declared_size > max_payload {
        return Err("IPC payload exceeds size limit".into());
    }

    let actual_size = response.len() - header_end - 1;

    if actual_size != declared_size {
        return Err("IPC payload length mismatch".into());
    }

    Ok(response.split_off(header_end + 1))
}

pub fn read_text(session: &Session) -> Result<String, Box<dyn Error>> {
    let response = query(
        session,
        b"read\n",
        validation::MAX_CLIPBOARD_BYTES + MAX_HEADER_BYTES,
    )?;

    parse_read(response)
}

fn parse_sync_read(
    response: Vec<u8>,
    format: Option<ClipboardFormat>,
) -> Result<ReadOutcome, Box<dyn Error>> {
    match response.as_slice() {
        b"ERR NO_TEXT\n" | b"ERR OVERSIZED\n" | b"ERR INVALID_TEXT\n" => {
            Ok(ReadOutcome::Unavailable)
        }

        b"ERR CLIPBOARD_CHANGED\n" => Ok(ReadOutcome::Changed),

        _ => parse_format_read(response, format).map(ReadOutcome::Data),
    }
}

pub fn read_for_sync(
    session: &Session,
    transport: TextTransport,
    expected_change_count: i64,
) -> Result<ReadOutcome, Box<dyn Error>> {
    let format = transport.format();
    let request = match format {
        Some(format) => format!("read-format {} {expected_change_count}\n", format.token()),
        None => "read\n".to_owned(),
    };
    let response = query(
        session,
        request.as_bytes(),
        validation::MAX_CLIPBOARD_BYTES + MAX_HEADER_BYTES,
    )?;

    parse_sync_read(response, format)
}

pub fn receive(session: &Session) -> Result<(), Box<dyn Error>> {
    println!("[IPC] Native macOS -> Windows");

    let text = read_text(session)?;

    println!("[IPC] Received {} bytes", text.len());

    clipboard::write_text(&text)?;

    println!("[IPC] Native clipboard transfer: PASS");

    Ok(())
}

fn build_write_request(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    validation::validate_text(text)?;

    let mut request = format!("write {}\n", text.len()).into_bytes();

    request.extend_from_slice(text.as_bytes());
    request.push(0);

    Ok(request)
}

fn parse_write_response(response: &[u8]) -> Result<(), Box<dyn Error>> {
    match response {
        b"OK\n" => Ok(()),

        b"ERR NO_TEXT\n" => Err("macOS rejected empty clipboard text".into()),

        b"ERR OVERSIZED\n" => Err("macOS rejected oversized clipboard text".into()),

        b"ERR INVALID_TEXT\n" => Err("macOS rejected invalid clipboard text".into()),

        b"ERR BAD_REQUEST\n" => Err("macOS rejected IPC write request".into()),

        b"ERR WRITE_FAILED\n" => Err("Native macOS clipboard write failed".into()),

        _ => Err("Invalid IPC write acknowledgement".into()),
    }
}

pub fn write_text(session: &Session, text: &str) -> Result<(), Box<dyn Error>> {
    let request = build_write_request(text)?;

    let response = query(session, &request, MAX_RESPONSE_BYTES)?;

    parse_write_response(&response)
}

fn build_checked_write_request(
    text: &str,
    expected_change_count: i64,
) -> Result<Vec<u8>, Box<dyn Error>> {
    build_format_write(text, expected_change_count, None)
}

fn build_format_write(
    text: &str,
    expected_change_count: i64,
    format: Option<ClipboardFormat>,
) -> Result<Vec<u8>, Box<dyn Error>> {
    validation::validate_text(text)?;

    let header = match format {
        Some(format) => format!(
            "write-format-if {} {expected_change_count} {}\n",
            format.token(),
            text.len()
        ),
        None => format!("write-if {expected_change_count} {}\n", text.len()),
    };
    let mut request = header.into_bytes();

    request.extend_from_slice(text.as_bytes());
    request.push(0);

    Ok(request)
}

fn parse_checked_write_response(response: &[u8]) -> Result<Option<i64>, Box<dyn Error>> {
    parse_format_ack(response, None)
}

fn parse_format_ack(
    response: &[u8],
    format: Option<ClipboardFormat>,
) -> Result<Option<i64>, Box<dyn Error>> {
    match response {
        b"ERR STALE\n" => return Ok(None),

        b"ERR NO_TEXT\n" => {
            return Err("macOS rejected empty clipboard text".into());
        }

        b"ERR OVERSIZED\n" => {
            return Err("macOS rejected oversized clipboard text".into());
        }

        b"ERR INVALID_TEXT\n" => {
            return Err("macOS rejected invalid clipboard text".into());
        }

        b"ERR BAD_REQUEST\n" => {
            return Err("macOS rejected conditional IPC write request".into());
        }

        b"ERR WRITE_FAILED\n" => {
            return Err("Native macOS clipboard write failed or was not verified".into());
        }

        b"ERR INVALID_PAYLOAD\n" => return Err("macOS rejected the clipboard payload".into()),

        _ => {}
    }

    if response.len() > MAX_HEADER_BYTES {
        return Err("IPC acknowledgement exceeds size limit".into());
    }

    let counter_bytes = response
        .strip_prefix(b"OK ")
        .and_then(|value| value.strip_suffix(b"\n"))
        .ok_or("Invalid conditional IPC write acknowledgement")?;

    let counter_bytes = format_value(counter_bytes, format)?;
    let digits = counter_bytes.strip_prefix(b"-").unwrap_or(counter_bytes);

    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err("Invalid IPC acknowledgement counter".into());
    }

    let counter = std::str::from_utf8(counter_bytes)?
        .parse::<i64>()
        .map_err(|_| "Invalid IPC acknowledgement counter")?;

    Ok(Some(counter))
}

pub fn write_text_checked(
    session: &Session,
    text: &str,
    expected_change_count: i64,
) -> Result<Option<i64>, Box<dyn Error>> {
    let request = build_checked_write_request(text, expected_change_count)?;

    let response = query(session, &request, MAX_RESPONSE_BYTES)?;

    parse_checked_write_response(&response)
}

pub fn write_for_sync(
    session: &Session,
    transport: TextTransport,
    text: &str,
    expected_change_count: i64,
) -> Result<Option<i64>, Box<dyn Error>> {
    let Some(format) = transport.format() else {
        return write_text_checked(session, text, expected_change_count);
    };

    let request = build_format_write(text, expected_change_count, Some(format))?;
    let response = query(session, &request, MAX_RESPONSE_BYTES)?;
    parse_format_ack(&response, Some(format))
}

pub fn read_bundle(
    session: &Session,
    expected_change_count: i64,
) -> Result<ReadOutcome<ClipboardBundle>, Box<dyn Error>> {
    let request = format!("read-format {BUNDLE_TOKEN} {expected_change_count}\n");
    let response = query(
        session,
        request.as_bytes(),
        MAX_BUNDLE_BYTES + MAX_HEADER_BYTES,
    )?;
    match response.as_slice() {
        b"ERR NO_TEXT\n" | b"ERR OVERSIZED\n" | b"ERR INVALID_PAYLOAD\n" => {
            Ok(ReadOutcome::Unavailable)
        }
        b"ERR CLIPBOARD_CHANGED\n" => Ok(ReadOutcome::Changed),
        _ => {
            let payload =
                parse_payload_read(response, Some(ClipboardFormat::Bundle), MAX_BUNDLE_BYTES)?;
            ClipboardBundle::decode(&payload).map(ReadOutcome::Data)
        }
    }
}

pub fn write_bundle_checked(
    session: &Session,
    bundle: &ClipboardBundle,
    expected_change_count: i64,
) -> Result<Option<i64>, Box<dyn Error>> {
    let payload = bundle.encode()?;
    let mut request = format!(
        "write-format-if {BUNDLE_TOKEN} {expected_change_count} {}\n",
        payload.len()
    )
    .into_bytes();
    request.extend_from_slice(&payload);
    request.push(0);
    let response = query(session, &request, MAX_RESPONSE_BYTES)?;
    parse_format_ack(&response, Some(ClipboardFormat::Bundle))
}

const MAX_FILE_CONTROL_BYTES: usize = 1_048_576;

pub fn file_stage(session: &Session) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let response = query(session, b"file-stage\n", MAX_FILE_CONTROL_BYTES)?;
    if response.starts_with(b"ERR ") {
        return Err("Could not open the Mac file staging directory".into());
    }
    let paths: [String; 1] = serde_json::from_slice(&response)?;
    let [path] = paths;
    Ok(path.into())
}

pub fn file_offer(
    session: &Session,
    expected_change_count: i64,
) -> Result<ReadOutcome<Vec<String>>, Box<dyn Error>> {
    let request = format!("file-offer {expected_change_count}\n");
    let response = query(session, request.as_bytes(), MAX_FILE_CONTROL_BYTES)?;
    match response.as_slice() {
        b"ERR CLIPBOARD_CHANGED\n" => Ok(ReadOutcome::Changed),
        b"ERR NO_FILES\n" => Ok(ReadOutcome::Unavailable),
        b"ERR OVERSIZED\n" => Err("Mac file selection is too large to describe".into()),
        _ => {
            let paths: Vec<String> = serde_json::from_slice(&response)?;
            if paths.is_empty() {
                return Ok(ReadOutcome::Unavailable);
            }
            Ok(ReadOutcome::Data(paths))
        }
    }
}

pub fn file_publish(
    session: &Session,
    expected_change_count: i64,
    batch: &str,
) -> Result<Option<i64>, Box<dyn Error>> {
    let request = format!("file-publish {expected_change_count} {batch}\n");
    let response = query(session, request.as_bytes(), MAX_RESPONSE_BYTES)?;
    if response == b"ERR FILE_STAGE\n" {
        return Err("Mac rejected the staged file selection".into());
    }
    parse_checked_write_response(&response)
}

pub fn test_checked_write(session: &Session) -> Result<(), Box<dyn Error>> {
    println!("[IPC] Testing conditional native clipboard writes");

    let fixture = concat!(
        "ReimsBridge M2.3C.2B\r\n",
        "Árvíztűrő tükörfúrógép 🚀 🦀 🍎\n",
        "\tTabbed text\n",
        "    Indented code\n",
        "Trailing spaces  \n",
    );

    clipboard::write_text(fixture)?;

    let source = clipboard::read_text()?;

    if source.as_bytes() != fixture.as_bytes() {
        return Err("Windows fixture verification failed".into());
    }

    println!("[IPC] Windows source fixture: PASS");

    let initial = get_status(session)?.change_count;

    println!("[IPC] Initial macOS change count: {initial}");

    let written =
        write_text_checked(session, &source, initial)?.ok_or("Unexpected stale destination")?;

    println!("[IPC] Written macOS change count: {written}");

    if written == initial || get_status(session)?.change_count != written {
        return Err("Conditional write counter verification failed".into());
    }

    if read_text(session)?.as_bytes() != source.as_bytes() {
        return Err("Conditional write payload verification failed".into());
    }

    println!("[IPC] Current-count conditional write: PASS");
    println!("[IPC] Verified acknowledgement counter: PASS");
    println!("[IPC] Exact Unicode transfer: PASS");

    if write_text_checked(session, "Stale replacement", initial)?.is_some() {
        return Err("Stale conditional write was accepted".into());
    }

    if get_status(session)?.change_count != written
        || read_text(session)?.as_bytes() != source.as_bytes()
    {
        return Err("Stale write modified destination".into());
    }

    println!("[IPC] Stale conditional write rejected: PASS");
    println!("[IPC] Destination preservation: PASS");

    for invalid in ["", "Embedded\0NUL"] {
        if write_text_checked(session, invalid, written).is_ok() {
            return Err("Invalid conditional write was accepted".into());
        }
    }

    let oversized = "x".repeat(validation::MAX_CLIPBOARD_BYTES + 1);

    if write_text_checked(session, &oversized, written).is_ok() {
        return Err("Oversized conditional write was accepted".into());
    }

    if get_status(session)?.change_count != written
        || read_text(session)?.as_bytes() != source.as_bytes()
    {
        return Err("Invalid write modified destination".into());
    }

    println!("[IPC] Invalid payload rejection: PASS");
    println!("[IPC] Invalid write preservation: PASS");

    let large = "0123456789ABCDEF".repeat(validation::MAX_CLIPBOARD_BYTES / 16);

    clipboard::write_text(&large)?;

    let large_source = clipboard::read_text()?;

    if large_source.as_bytes() != large.as_bytes() {
        return Err("Maximum Windows fixture verification failed".into());
    }

    println!("[IPC] Transferring {} bytes", large_source.len());

    let started = std::time::Instant::now();

    let large_counter = write_text_checked(session, &large_source, written)?
        .ok_or("Unexpected stale destination during maximum transfer")?;

    println!("[IPC] Upload elapsed: {:?}", started.elapsed());

    if large_counter == written || get_status(session)?.change_count != large_counter {
        return Err("Maximum write counter verification failed".into());
    }

    let started = std::time::Instant::now();

    let received = read_text(session)?;

    println!("[IPC] Download elapsed: {:?}", started.elapsed());

    if received.as_bytes() != large_source.as_bytes() {
        return Err("Maximum conditional payload mismatch".into());
    }

    println!("[IPC] Maximum conditional transfer: PASS");
    println!("[IPC] Maximum payload exact readback: PASS");

    if write_text_checked(session, "Old counter", written)?.is_some() {
        return Err("Previous counter was incorrectly accepted".into());
    }

    if get_status(session)?.change_count != large_counter
        || read_text(session)?.as_bytes() != large_source.as_bytes()
    {
        return Err("Stale write modified maximum payload".into());
    }

    println!("[IPC] Stale rejection after maximum write: PASS");

    let final_fixture = "ReimsBridge M2.3C.2B - PASS";

    write_text(session, final_fixture)?;

    if read_text(session)? != final_fixture {
        return Err("Legacy write compatibility failed".into());
    }

    clipboard::write_text(final_fixture)?;

    println!("[IPC] Legacy unconditional write: PASS");
    println!("[IPC] All conditional IPC tests: PASS");

    Ok(())
}

pub fn send(session: &Session) -> Result<(), Box<dyn Error>> {
    println!("[IPC] Native Windows -> macOS");

    let text = clipboard::read_text()?;

    println!("[IPC] Transferring {} bytes", text.len());

    write_text(session, &text)?;

    println!("[IPC] Native clipboard transfer: PASS");

    Ok(())
}

pub fn check(session: &Session) -> Result<(), Box<dyn Error>> {
    println!("[IPC] Querying native macOS application");

    let status = get_status(session)?;

    println!("[IPC] Protocol version: {}", status.protocol);
    println!("[IPC] Application status: {}", status.status);
    println!("[IPC] Clipboard change count: {}", status.change_count);
    println!("[IPC] Plain text available: {}", status.has_plain_text);
    println!(
        "[IPC] Rich text or image available: {}",
        status.has_bundle()
    );
    println!("[IPC] Content excluded: {}", status.is_excluded);

    let eligible = status.has_plain_text || status.has_bundle();

    println!("[IPC] Transfer candidate: {eligible}");
    println!(
        "[IPC] Clipboard framing: {}",
        status.text_transport().description()
    );
    println!("[IPC] Native macOS communication: PASS");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_RESPONSE: &[u8] = br#"{
        "protocol": 2,
        "status": "ok",
        "changeCount": 14,
        "hasPlainText": true,
        "isExcluded": false
    }"#;

    #[test]
    fn mac_status_recognizes_file_url_capability_and_availability() {
        for field in ["hasFileURLs", "hasFileUrls"] {
            for available in [false, true] {
                let response = format!(
                    r#"{{"protocol":2,"status":"ok","changeCount":42,"hasPlainText":false,"isExcluded":false,"fileTransferVersion":1,"{field}":{available}}}"#
                );
                let status = parse_status(response.as_bytes()).unwrap();
                assert_eq!(status.file_transfer_version, 1);
                assert_eq!(status.has_file_urls, available);
            }
        }

        assert!(!parse_status(VALID_RESPONSE).unwrap().has_file_urls);
    }

    #[test]
    fn valid_response() {
        let status = parse_status(VALID_RESPONSE).unwrap();

        assert_eq!(status.protocol, 2);
        assert_eq!(status.change_count, 14);
        assert!(status.has_plain_text);
        assert!(!status.is_excluded);
        assert!(!status.watch_status_supported);
    }

    #[test]
    fn watch_status_capability_detected() {
        let response = br#"{
            "protocol": 2,
            "status": "ok",
            "changeCount": 14,
            "hasPlainText": true,
            "isExcluded": false,
            "watchStatusSupported": true
        }"#;

        assert!(parse_status(response).unwrap().watch_status_supported);
    }

    #[test]
    fn malformed_watch_status_capability_rejected() {
        let response = br#"{
            "protocol": 2,
            "status": "ok",
            "changeCount": 14,
            "hasPlainText": true,
            "isExcluded": false,
            "watchStatusSupported": "true"
        }"#;

        assert!(parse_status(response).is_err());
    }

    #[test]
    fn malformed_json_rejected() {
        assert!(parse_status(b"not json").is_err());
    }

    #[test]
    fn missing_fields_rejected() {
        assert!(parse_status(br#"{"protocol":1}"#).is_err());
    }

    #[test]
    fn unsupported_protocol_rejected() {
        let response = br#"{
            "protocol": 99,
            "status": "ok",
            "changeCount": 14,
            "hasPlainText": true,
            "isExcluded": false
        }"#;

        assert!(parse_status(response).is_err());
    }

    #[test]
    fn remote_error_rejected() {
        let response = br#"{
            "protocol": 2,
            "status": "error",
            "changeCount": 14,
            "hasPlainText": true,
            "isExcluded": false
        }"#;

        let error = parse_status(response).unwrap_err();

        assert_eq!(error.to_string(), "macOS IPC returned an error");
    }

    #[test]
    fn oversized_response_rejected() {
        let response = vec![b'x'; MAX_RESPONSE_BYTES + 1];

        assert!(parse_status(&response).is_err());
    }

    #[test]
    fn sync_read_valid_text() {
        let response = b"OK 5\nHello".to_vec();

        let result = parse_sync_read(response, None).unwrap();

        assert!(matches!(
            result,
            ReadOutcome::Data(text) if text == "Hello"
        ));
    }

    #[test]
    fn sync_read_unavailable() {
        for response in [
            b"ERR NO_TEXT\n".as_slice(),
            b"ERR OVERSIZED\n",
            b"ERR INVALID_TEXT\n",
        ] {
            assert!(matches!(
                parse_sync_read(response.to_vec(), None).unwrap(),
                ReadOutcome::Unavailable
            ));
        }
    }

    #[test]
    fn sync_read_changed() {
        assert!(matches!(
            parse_sync_read(b"ERR CLIPBOARD_CHANGED\n".to_vec(), None).unwrap(),
            ReadOutcome::Changed
        ));
    }

    #[test]
    fn sync_read_unexpected_failures() {
        for response in [
            b"ERR READ_FAILED\n".as_slice(),
            b"ERR EXCLUDED\n",
            b"ERR UNKNOWN\n",
            b"ERR NO_TEXT\nextra",
            b"OK 10\nHello",
            b"",
        ] {
            assert!(parse_sync_read(response.to_vec(), None).is_err());
        }
    }

    #[test]
    fn valid_text_response() {
        let text = "Árvíztűrő 🚀\r\n    Code  \n";

        let mut response = format!("OK {}\n", text.len()).into_bytes();

        response.extend_from_slice(text.as_bytes());

        assert_eq!(parse_read(response).unwrap(), text);
    }

    #[test]
    fn truncated_payload_rejected() {
        assert!(parse_read(b"OK 10\nHello".to_vec()).is_err());
    }

    #[test]
    fn unexpected_extra_bytes_rejected() {
        assert!(parse_read(b"OK 3\nHello".to_vec()).is_err());
    }

    #[test]
    fn invalid_utf8_payload_rejected() {
        let mut response = b"OK 2\n".to_vec();

        response.extend_from_slice(&[0xff, 0xfe]);

        assert!(parse_read(response).is_err());
    }

    #[test]
    fn excluded_clipboard_rejected() {
        assert!(parse_read(b"ERR EXCLUDED\n".to_vec()).is_err());
    }

    #[test]
    fn oversized_payload_rejected() {
        let response = format!("OK {}\n", validation::MAX_CLIPBOARD_BYTES + 1);

        assert!(parse_read(response.into_bytes()).is_err());
    }

    #[test]
    fn valid_checked_write_request() {
        let request = build_checked_write_request("Hello", 42).unwrap();

        assert_eq!(request, b"write-if 42 5\nHello\0");
    }

    #[test]
    fn unicode_checked_write_request() {
        let request = build_checked_write_request("🚀", 42).unwrap();

        assert_eq!(request, b"write-if 42 4\n\xf0\x9f\x9a\x80\0");
    }

    #[test]
    fn signed_checked_write_counter() {
        let request = build_checked_write_request("x", i64::MIN).unwrap();

        assert_eq!(request, b"write-if -9223372036854775808 1\nx\0");
    }

    #[test]
    fn maximum_checked_write_request() {
        let text = "x".repeat(validation::MAX_CLIPBOARD_BYTES);

        let request = build_checked_write_request(&text, i64::MAX).unwrap();

        let header = b"write-if 9223372036854775807 1048576\n";

        assert!(request.starts_with(header));
        assert_eq!(request.last(), Some(&0));

        assert_eq!(
            request.len(),
            header.len() + validation::MAX_CLIPBOARD_BYTES + 1
        );
    }

    #[test]
    fn empty_checked_write_rejected() {
        assert!(build_checked_write_request("", 42).is_err());
    }

    #[test]
    fn nul_checked_write_rejected() {
        assert!(build_checked_write_request("a\0b", 42).is_err());
    }

    #[test]
    fn oversized_checked_write_rejected() {
        let text = "x".repeat(validation::MAX_CLIPBOARD_BYTES + 1);

        assert!(build_checked_write_request(&text, 42).is_err());
    }

    #[test]
    fn valid_checked_write_acknowledgement() {
        assert_eq!(parse_checked_write_response(b"OK 43\n").unwrap(), Some(43));
    }

    #[test]
    fn zero_checked_write_acknowledgement() {
        assert_eq!(parse_checked_write_response(b"OK 0\n").unwrap(), Some(0));
    }

    #[test]
    fn signed_checked_write_acknowledgement() {
        assert_eq!(
            parse_checked_write_response(b"OK -9223372036854775808\n").unwrap(),
            Some(i64::MIN)
        );

        assert_eq!(
            parse_checked_write_response(b"OK 9223372036854775807\n").unwrap(),
            Some(i64::MAX)
        );
    }

    #[test]
    fn stale_checked_write_acknowledgement() {
        assert_eq!(parse_checked_write_response(b"ERR STALE\n").unwrap(), None);
    }

    #[test]
    fn missing_checked_write_counter_rejected() {
        for response in [b"".as_slice(), b"OK", b"OK\n", b"OK \n", b"OK 43"] {
            assert!(parse_checked_write_response(response).is_err());
        }
    }

    #[test]
    fn malformed_checked_write_counter_rejected() {
        for response in [
            b"OK abc\n".as_slice(),
            b"OK +43\n",
            b"OK --43\n",
            b"OK -\n",
            b"OK  43\n",
            b"OK 43 \n",
            b"OK 4.3\n",
            b"OK \xff\n",
        ] {
            assert!(parse_checked_write_response(response).is_err());
        }
    }

    #[test]
    fn overflow_checked_write_counter_rejected() {
        for response in [
            b"OK 9223372036854775808\n".as_slice(),
            b"OK -9223372036854775809\n",
        ] {
            assert!(parse_checked_write_response(response).is_err());
        }
    }

    #[test]
    fn trailing_checked_write_bytes_rejected() {
        for response in [
            b"OK 43\nextra".as_slice(),
            b"OK 43\n\n",
            b"ERR STALE\nextra",
            b"ERR STALE",
        ] {
            assert!(parse_checked_write_response(response).is_err());
        }
    }

    #[test]
    fn failed_checked_write_acknowledgement() {
        for response in [
            b"ERR NO_TEXT\n".as_slice(),
            b"ERR OVERSIZED\n",
            b"ERR INVALID_TEXT\n",
            b"ERR BAD_REQUEST\n",
            b"ERR WRITE_FAILED\n",
            b"ERR UNKNOWN\n",
        ] {
            assert!(parse_checked_write_response(response).is_err());
        }
    }

    #[test]
    fn oversized_checked_acknowledgement_rejected() {
        let response = format!("OK {}\n", "0".repeat(MAX_HEADER_BYTES));

        assert!(parse_checked_write_response(response.as_bytes()).is_err());
    }

    #[test]
    fn valid_write_request() {
        let request = build_write_request("Hello").unwrap();

        assert_eq!(request, b"write 5\nHello\0");
    }

    #[test]
    fn unicode_write_request() {
        let request = build_write_request("🚀").unwrap();

        assert_eq!(request, b"write 4\n\xf0\x9f\x9a\x80\0");
    }

    #[test]
    fn empty_write_request_rejected() {
        assert!(build_write_request("").is_err());
    }

    #[test]
    fn embedded_nul_write_request_rejected() {
        assert!(build_write_request("a\0b").is_err());
    }

    #[test]
    fn maximum_write_request_accepted() {
        let text = "x".repeat(validation::MAX_CLIPBOARD_BYTES);

        let request = build_write_request(&text).unwrap();

        assert!(request.starts_with(b"write 1048576\n"));
        assert_eq!(request.last(), Some(&0));
        assert_eq!(
            request.len(),
            validation::MAX_CLIPBOARD_BYTES + b"write 1048576\n".len() + 1
        );
    }

    #[test]
    fn oversized_write_request_rejected() {
        let text = "x".repeat(validation::MAX_CLIPBOARD_BYTES + 1);

        assert!(build_write_request(&text).is_err());
    }

    #[test]
    fn valid_write_acknowledgement() {
        assert!(parse_write_response(b"OK\n").is_ok());
    }

    #[test]
    fn failed_write_acknowledgement() {
        assert!(parse_write_response(b"ERR WRITE_FAILED\n").is_err());
    }

    #[test]
    fn malformed_write_acknowledgement() {
        for response in [b"".as_slice(), b"OK", b"OK\nextra", b"UNKNOWN\n"] {
            assert!(parse_write_response(response).is_err());
        }
    }
    #[test]
    fn negotiates_named_text_and_falls_back_for_legacy_or_unknown_versions() {
        for (formats, transport, available) in [
            ("", TextTransport::Legacy, true),
            (
                r#","formats":{"version":2,"supported":["text-utf8"],"available":[]}"#,
                TextTransport::Legacy,
                true,
            ),
            (
                r#","formats":{"version":1,"supported":["image-png"],"available":[]}"#,
                TextTransport::Legacy,
                true,
            ),
            (
                r#","formats":{"version":1,"supported":["text-utf8"],"available":[]}"#,
                TextTransport::NamedV1,
                false,
            ),
            (
                r#","formats":{"version":1,"supported":["text-utf8","image-png"],"available":["text-utf8"]}"#,
                TextTransport::NamedV1,
                true,
            ),
        ] {
            let response = format!(
                r#"{{"protocol":2,"status":"ok","changeCount":-42,"hasPlainText":true,"isExcluded":false{formats}}}"#
            );
            let status = parse_status(response.as_bytes()).unwrap();
            assert_eq!(status.text_transport(), transport);
            assert_eq!(status.has_text_for(transport), available);
        }
    }

    #[test]
    fn named_text_frames_require_matching_format_and_utf8_byte_length() {
        let format = Some(ClipboardFormat::Utf8Text);
        let frame = build_format_write("á🦀", i64::MIN, format).unwrap();
        assert_eq!(
            frame,
            "write-format-if text-utf8 -9223372036854775808 6\ná🦀\0".as_bytes()
        );
        assert!(frame.iter().position(|&byte| byte == b'\n').unwrap() < MAX_HEADER_BYTES);

        let read = "OK text-utf8 6\ná🦀".as_bytes().to_vec();
        assert!(
            matches!(parse_sync_read(read, format).unwrap(), ReadOutcome::Data(text) if text == "á🦀")
        );
        assert!(parse_sync_read(b"OK image-png 5\nHello".to_vec(), format).is_err());
        assert!(parse_sync_read(b"OK text-utf8 6\nHello".to_vec(), format).is_err());
        assert!(matches!(
            parse_sync_read(b"ERR CLIPBOARD_CHANGED\n".to_vec(), format).unwrap(),
            ReadOutcome::Changed
        ));
    }

    #[test]
    fn named_write_acknowledgements_bind_to_the_requested_format() {
        let format = Some(ClipboardFormat::Utf8Text);
        assert_eq!(
            parse_format_ack(b"OK text-utf8 -42\n", format).unwrap(),
            Some(-42)
        );
        assert_eq!(parse_format_ack(b"ERR STALE\n", format).unwrap(), None);
        assert!(parse_format_ack(b"OK 42\n", format).is_err());
        assert!(parse_format_ack(b"OK image-png 42\n", format).is_err());
    }
}
