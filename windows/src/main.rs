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

fn run() -> Result<(), Box<dyn Error>> {
    println!("ReimsBridge v{}", env!("CARGO_PKG_VERSION"));
    println!("M1.1 - SSH transport verification");
    println!();

    let username = env::args()
        .nth(1)
        .ok_or("Usage: reimsbridge-windows <username>")?;

    let session = connect(&username)?;

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

    session.disconnect(None, "ReimsBridge test complete", None)?;

    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("[ERROR] {error}");
        std::process::exit(1);
    }
}
