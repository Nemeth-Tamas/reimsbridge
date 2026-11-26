# ReimsBridge

Lightweight native clipboard integration between a Windows
host and a macOS guest running under Reims vGPU/QEMU.

## Architecture

- Windows companion written in Rust.
- Planned macOS companion written in Swift and SwiftUI.
- Authenticated SSH transport.
- No additional network ports or external services.
- No Electron or unnecessary application frameworks.

## Current features

- Manual bidirectional plain-text clipboard synchronization.
- Native Windows Unicode clipboard access.
- UTF-8 transport over SSH.
- Password authentication.
- SSH host-key verification.
- Clipboard payload validation.
- 1 MiB clipboard size limit.
- No application-level clipboard history or content logging.

## Building

Requires Rust, Cargo, MSVC build tools and Perl for the
vendored OpenSSL build.

```powershell
cd windows
cargo build --release
```

## Usage

The current development endpoint is 127.0.0.1:2222.

The SSH server host key must already be trusted in the
Windows user's OpenSSH known_hosts file.

```powershell
cargo run -- ssh-test <username>
cargo run -- clipboard-check
cargo run -- clipboard-test
cargo run -- send <username>
cargo run -- receive <username>
cargo run -- roundtrip-test <username>
```

Passwords are requested interactively and are not embedded
in the application or configuration files.

## Testing

```powershell
cargo test
cargo clippy -- -D warnings
cargo run -- roundtrip-test <username>
```

The clipboard-test and roundtrip-test commands deliberately
replace existing clipboard contents.

## Security

Clipboard contents are transferred as data through SSH.
They are never interpolated into executable shell commands.

Automatic synchronization is not currently implemented.

The current macOS transport uses pbcopy and pbpaste.
Reliable identification of sensitive clipboard formats
requires further native clipboard integration.

## Production acceptance criteria

ReimsBridge aims to provide transparent, native clipboard
integration between Windows and macOS.

After initial setup, users must be able to copy and paste
between operating systems using their ordinary keyboard
shortcuts and application menus.

No additional transfer buttons, special keyboard shortcuts
or manual confirmation should be required during normal
clipboard synchronization.

Production targets:

- Automatic bidirectional plain-text synchronization.
- Exact Unicode and multiline text preservation.
- Compatible rich-text and HTML clipboard formats.
- Native image and screenshot synchronization.
- Image transparency preservation where supported.
- Bidirectional file transfers of at least 1 GiB per file.
- Multiple-file and recursive directory transfers.
- Bounded-memory streaming for large payloads.
- Native Explorer and Finder copy/paste integration.
- Transfer progress and cancellation for large files.
- File integrity verification and safe temporary staging.
- Automatic clipboard feedback-loop prevention.
- Unchanged-content suppression.
- Deterministic simultaneous-change handling.
- Automatic recovery following VM or SSH disconnection.
- Secure credential storage after initial setup.
- Optional automatic application startup.
- User-controlled synchronization pause and resume.
- Best-effort exclusion of sensitive clipboard contents.
- No permanent clipboard content history.
- No clipboard content logging or telemetry.
- No additional network ports or external services.

Large file transfers must not require loading the entire
file into memory.

Native delayed rendering and file-promise mechanisms
should be investigated to support transparent file
transfers without unnecessary eager copying.

## Roadmap

- [x] M0: SSH clipboard proof of concept
- [x] M1.1: Native Rust SSH transport
- [x] M1.2: Native Windows clipboard access
- [x] M1.3: Manual bidirectional synchronization
- [x] M1.4: Transfer validation and hardening
- [x] M2.0: Native SwiftUI macOS application
- [x] M2.1: Native macOS clipboard service
- [x] M2.2A: Private Unix-domain socket IPC
- [x] M2.2B: Rust IPC integration
- [x] M2.2C.1: Native macOS clipboard retrieval
- [x] M2.2C.2: Native bidirectional clipboard transfers
- [x] M2.2D: Maximum-size text transfer validation
- [ ] M2.3: Automatic bidirectional text synchronization
- [ ] M3: Complete native macOS menu bar interface
- [ ] M4: Native Windows system tray companion
- [ ] M5: Reconnection and credential management
- [ ] M6: Multi-format clipboard protocol
- [ ] M7: Native image and rich-text synchronization
- [ ] M8: Streaming file transfer protocol
- [ ] M9: Transparent Explorer and Finder integration
- [ ] M10: Performance, security and reliability hardening
- [ ] M11: Production packaging and release