use ssh2::{CheckResult, KnownHostFileKind, Session};
use std::env;
use std::error::Error;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::ptr;
use std::time::Duration;

pub const HOST: &str = "127.0.0.1";
pub const PORT: u16 = 2222;
const TIMEOUT_MS: u32 = 5000;

pub struct Password(String);

impl Password {
    pub fn new(value: String) -> Self {
        Self(value)
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        // Zero bytes are valid UTF-8. Volatile writes keep the wipe intact.
        wipe(unsafe { self.0.as_mut_vec() });
    }
}

pub fn wipe<T: Default + Copy>(buffer: &mut [T]) {
    for value in buffer {
        unsafe { ptr::write_volatile(value, T::default()) };
    }
}

pub fn connect_verified() -> Result<Session, Box<dyn Error>> {
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

    Ok(session)
}

pub fn authenticate(
    session: &Session,
    username: &str,
    password: Password,
) -> Result<(), Box<dyn Error>> {
    session.userauth_password(username, &password.0)?;

    if !session.authenticated() {
        return Err("SSH authentication failed".into());
    }

    println!("[SSH] Authentication successful");
    Ok(())
}
