mod clipboard;
mod mac_ipc;
mod remote_clipboard;
mod sync;
mod validation;

use ssh2::{CheckResult, KnownHostFileKind, Session};
use std::env;
use std::error::Error;
use std::io::Read;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

const HOST: &str = "127.0.0.1";
const PORT: u16 = 2222;
const TIMEOUT_MS: u32 = 5000;

fn connect(username: &str) -> Result<Session, Box<dyn Error>> {
    let address: SocketAddr = format!("{HOST}:{PORT}").parse()?;

    println!("[SSH] Connecting to {address}");

    let tcp = TcpStream::connect_timeout(&address, Duration::from_millis(TIMEOUT_MS.into()))?;

    tcp.set_nodelay(true)?;

    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.set_timeout(TIMEOUT_MS);
    session.handshake()?;

    println!("[SSH] Handshake successful");

    let home = env::var_os("USERPROFILE").ok_or("USERPROFILE is unavailable")?;

    let known_hosts_path = PathBuf::from(home).join(".ssh").join("known_hosts");

    let mut known_hosts = session.known_hosts()?;

    known_hosts.read_file(&known_hosts_path, KnownHostFileKind::OpenSSH)?;

    let (host_key, key_type) = session
        .host_key()
        .ok_or("Server did not provide a host key")?;

    match known_hosts.check_port(HOST, PORT, host_key) {
        CheckResult::Match => {
            println!("[SSH] Host key verified: {key_type:?}");
        }
        CheckResult::Mismatch => {
            return Err("SSH host key mismatch".into());
        }
        CheckResult::NotFound => {
            return Err("SSH host key is not trusted".into());
        }
        CheckResult::Failure => {
            return Err("SSH host key verification failed".into());
        }
    }

    drop(known_hosts);

    let password = rpassword::prompt_password(format!("{username}@{HOST}'s password: "))?;

    session.userauth_password(username, &password)?;

    drop(password);

    if !session.authenticated() {
        return Err("SSH authentication failed".into());
    }

    println!("[SSH] Authentication successful");

    Ok(session)
}

fn transport_test(session: &Session) -> Result<(), Box<dyn Error>> {
    let mut channel = session.channel_session()?;

    channel.exec("/usr/bin/whoami")?;

    let mut output = String::new();

    channel.read_to_string(&mut output)?;
    channel.wait_close()?;

    if channel.exit_status()? != 0 {
        return Err("Remote command failed".into());
    }

    println!("[SSH] Remote user: {}", output.trim());
    println!("[SSH] Transport test passed");

    Ok(())
}

fn print_help() {
    println!("Usage:");
    println!("  reimsbridge-windows clipboard-check");
    println!("  reimsbridge-windows clipboard-test");
    println!("  reimsbridge-windows ssh-test <username>");
    println!("  reimsbridge-windows mac-status <username>");
    println!("  reimsbridge-windows send <username>");
    println!("  reimsbridge-windows receive <username>");
    println!("  reimsbridge-windows roundtrip-test <username>");
}

fn run() -> Result<(), Box<dyn Error>> {
    println!("ReimsBridge v{}", env!("CARGO_PKG_VERSION"));
    println!("M2.2B - Native macOS IPC integration");
    println!();

    let mut args = env::args().skip(1);

    let Some(command) = args.next() else {
        print_help();
        return Ok(());
    };

    match command.as_str() {
        "help" | "--help" | "-h" => {
            print_help();
            return Ok(());
        }
        "clipboard-check" => return clipboard::check(),
        "clipboard-test" => return clipboard::self_test(),
        "ssh-test" | "send" | "receive" | "roundtrip-test" | "mac-status" => {}
        _ => {
            print_help();
            return Err("Unknown command".into());
        }
    }

    let username = args.next().ok_or("Missing SSH username")?;

    if args.next().is_some() {
        return Err("Unexpected command-line argument".into());
    }

    let session = connect(&username)?;

    match command.as_str() {
        "ssh-test" => transport_test(&session)?,
        "mac-status" => mac_ipc::check(&session)?,
        "send" => sync::send(&session)?,
        "receive" => sync::receive(&session)?,
        "roundtrip-test" => sync::roundtrip_test(&session)?,
        _ => unreachable!(),
    }

    session.disconnect(None, "ReimsBridge session complete", None)?;

    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("[ERROR] {error}");
        std::process::exit(1);
    }
}
