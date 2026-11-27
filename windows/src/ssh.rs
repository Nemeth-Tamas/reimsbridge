use libssh2_sys::{
    LIBSSH2_ERROR_BANNER_RECV, LIBSSH2_ERROR_BANNER_SEND, LIBSSH2_ERROR_CHANNEL_CLOSED,
    LIBSSH2_ERROR_CHANNEL_FAILURE, LIBSSH2_ERROR_EAGAIN, LIBSSH2_ERROR_SOCKET_DISCONNECT,
    LIBSSH2_ERROR_SOCKET_RECV, LIBSSH2_ERROR_SOCKET_SEND, LIBSSH2_ERROR_SOCKET_TIMEOUT,
    LIBSSH2_ERROR_TIMEOUT,
};
use ssh2::{CheckResult, ErrorCode, KnownHostFileKind, Session};
use std::error::Error;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::ptr;
use std::time::Duration;
use std::{env, fmt, io};

pub const HOST: &str = "127.0.0.1";
pub const PORT: u16 = 2222;
const TIMEOUT_MS: u32 = 5000;

pub struct Password(String);

impl Password {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
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
    verify_host_key(&session).map_err(HostVerificationError)?;
    Ok(session)
}

#[derive(Debug)]
struct HostVerificationError(Box<dyn Error>);

impl fmt::Display for HostVerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Error for HostVerificationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

fn verify_host_key(session: &Session) -> Result<(), Box<dyn Error>> {
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

    Ok(())
}

pub fn authenticate(
    session: &Session,
    username: &str,
    password: &Password,
) -> Result<(), Box<dyn Error>> {
    session.userauth_password(username, &password.0)?;

    if !session.authenticated() {
        return Err("SSH authentication failed".into());
    }

    println!("[SSH] Authentication successful");
    Ok(())
}

pub fn is_transport_error(error: &(dyn Error + 'static)) -> bool {
    if let Some(error) = error.downcast_ref::<io::Error>() {
        return matches!(
            error.kind(),
            io::ErrorKind::ConnectionRefused
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::NotConnected
                | io::ErrorKind::TimedOut
                | io::ErrorKind::BrokenPipe
                | io::ErrorKind::NetworkUnreachable
                | io::ErrorKind::HostUnreachable
                | io::ErrorKind::UnexpectedEof
        );
    }

    error.downcast_ref::<ssh2::Error>().is_some_and(|error| {
        matches!(
            error.code(),
            ErrorCode::Session(
                LIBSSH2_ERROR_BANNER_RECV
                    | LIBSSH2_ERROR_BANNER_SEND
                    | LIBSSH2_ERROR_SOCKET_SEND
                    | LIBSSH2_ERROR_SOCKET_RECV
                    | LIBSSH2_ERROR_SOCKET_DISCONNECT
                    | LIBSSH2_ERROR_SOCKET_TIMEOUT
                    | LIBSSH2_ERROR_TIMEOUT
                    | LIBSSH2_ERROR_CHANNEL_CLOSED
                    | LIBSSH2_ERROR_CHANNEL_FAILURE
                    | LIBSSH2_ERROR_EAGAIN
            )
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use libssh2_sys::LIBSSH2_ERROR_AUTHENTICATION_FAILED;

    #[test]
    fn only_transport_failures_are_retried() {
        let refused = || io::Error::from(io::ErrorKind::ConnectionRefused);
        assert!(is_transport_error(&refused()));
        assert!(is_transport_error(&ssh2::Error::new(
            ErrorCode::Session(LIBSSH2_ERROR_SOCKET_DISCONNECT),
            "transport closed",
        )));
        assert!(!is_transport_error(&ssh2::Error::new(
            ErrorCode::Session(LIBSSH2_ERROR_AUTHENTICATION_FAILED),
            "authentication rejected",
        )));
        assert!(!is_transport_error(&HostVerificationError(Box::new(
            refused(),
        ))));
    }
}
