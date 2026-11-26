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

## Roadmap

- [x] M0: SSH clipboard proof of concept
- [x] M1.1: Native Rust SSH transport
- [x] M1.2: Native Windows clipboard access
- [x] M1.3: Manual bidirectional synchronization
- [ ] M1.4: Transfer validation and hardening
- [ ] M2: Automatic bidirectional synchronization
- [ ] M3: Native SwiftUI macOS menu bar application
- [ ] M4: Native Windows system tray companion
- [ ] M5: Reconnection, security and packaging