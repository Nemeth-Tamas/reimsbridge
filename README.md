# ReimsBridge

Lightweight native clipboard integration between a Windows
host and a macOS guest running under Reims vGPU/QEMU.

## Architecture

- Windows companion written in Rust.
- Native macOS menu bar companion written in Swift and SwiftUI.
- Authenticated SSH transport.
- Private Unix-domain socket for native macOS clipboard IPC.
- No additional network ports or external services.
- No Electron or unnecessary application frameworks.

## Current features

- Automatic bidirectional plain-text clipboard synchronization.
- Native Windows and macOS clipboard access.
- Ordinary keyboard shortcuts and application Copy/Paste menus.
- Exact Unicode and multiline text preservation.
- UTF-8 transport over authenticated SSH.
- Password authentication and SSH host-key verification.
- Clipboard metadata and native change-counter inspection.
- Counter-aware conditional clipboard writes.
- Automatic clipboard feedback-loop prevention.
- Identical payload suppression.
- Simultaneous-change detection and conflict preservation.
- Startup baseline initialization without clipboard modification.
- Clipboard payload validation.
- 1 MiB plain-text clipboard size limit.
- Native macOS menu bar application.
- Windows notification-area companion and foreground synchronization command.
- macOS menu bar icon for IPC state and recent watcher activity.
- Live macOS clipboard metadata and per-session IPC transfer statistics.
- Persistent macOS preferences for IPC startup and statistics visibility.
- No application-level clipboard history or content logging.

## Building

Requires Rust, Cargo, MSVC build tools and Perl for the
vendored OpenSSL build.

```powershell
cd windows
cargo build --release
```

## Usage

### Windows tray companion

Build the Windows executables as shown above, then launch the tray from
the `windows` directory.

```powershell
.\target\release\reimsbridge-tray.exe
```

Start IPC in the macOS app. In the Windows notification area, open
ReimsBridge and choose **Start Synchronization**. Enter the macOS SSH
username and password in the Windows prompt. The tray shows when the
watcher is connecting, synchronizing, stopping, or has encountered an
error. Open **View Error** for connection details.

**Stop Synchronization** finishes the current polling step and ends the
SSH session. Starting again establishes a fresh clipboard baseline. The
tray and the foreground watcher share a single watcher slot, including
during `watch-test`. The tray asks for credentials each time it starts
a watcher.

### Foreground synchronization and diagnostics

The current development endpoint is 127.0.0.1:2222.

The SSH server host key must already be trusted in the
Windows user's OpenSSH known_hosts file.

Build and launch the macOS ReimsBridge application using
the Xcode project located in `macos/ReimsBridge`.

Ensure the native IPC service is listening before starting
automatic synchronization on Windows.

```powershell
cargo run -- watch <username>
```

Once connected, ordinary Windows and macOS Copy/Paste
operations automatically synchronize supported text.

Press Ctrl+C in the Windows terminal to stop synchronization.

Updated macOS and Windows companions support watcher-specific
status requests. The macOS menu reports recent watcher activity
for 30 seconds after the last completed watcher status request.
Ordinary diagnostic requests do not activate the indicator.
The indicator is a liveness hint, not proof of a successful
transfer or a persistent connection. Older companions remain
compatible with the original protocol 2 status request.

The macOS menu shows the current IPC state and recent Windows watcher
activity. Clipboard metadata updates automatically, including while IPC is
stopped. Clipboard validation and the private-pasteboard self-test are
available under **Diagnostics**.

Under **Preferences**, enable **Start IPC on Launch** to start the
listener whenever ReimsBridge opens. Manual startup is the default.
**Show Transfer Statistics** controls whether the menu displays session
counters and is on by default. Both preferences persist across launches.
Quitting the app stops the listener.

Transfer statistics count text payloads and UTF-8 bytes for completed IPC
exchanges. Sent counts report reads from the Mac pasteboard. Received
counts report successful writes to it. A read can also serve a diagnostic
or a watcher comparison. Counters reset when the listener starts and are
held in memory.

Diagnostic and manual transfer commands:

```powershell
cargo run -- ssh-test <username>
cargo run -- clipboard-check
cargo run -- clipboard-metadata
cargo run -- clipboard-test
cargo run -- mac-status <username>
cargo run -- mac-send <username>
cargo run -- mac-receive <username>
cargo run -- mac-conditional-test <username>
cargo run -- watch-test <username>
```

Legacy pbcopy/pbpaste development commands:

```powershell
cargo run -- send <username>
cargo run -- receive <username>
cargo run -- roundtrip-test <username>
```

Passwords are requested interactively and are not embedded
in the application or configuration files.

## Testing

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -- clipboard-test
cargo run -- mac-conditional-test <username>
cargo run -- watch-test <username>
```

The macOS application also provides a native clipboard
self-test using a private pasteboard.

The integration test commands deliberately replace existing
clipboard contents. Stop automatic synchronization before
running these tests.

The automatic synchronization acceptance test covers:

- Startup baseline preservation.
- Bidirectional automatic synchronization.
- Exact Unicode and formatting preservation.
- Feedback-loop prevention.
- Identical payload suppression.
- Simultaneous-change detection.
- Preservation of conflicting clipboard contents.
- Automatic synchronization after conflicts.
- Maximum-size 1 MiB transfers in both directions.
- Recovery after maximum-size transfers.
- Idle clipboard counter stability.

### Initial performance baseline

Development environment: AMD Ryzen 9 7900 Windows host
and virtualized macOS guest with 12 virtual CPUs.

| Measurement | Observed result |
|---|---:|
| Windows idle CPU, total host capacity | 0.011% |
| Windows idle CPU, single-core equivalent | 0.255% |
| Windows working set | 9.52 MiB |
| Windows private memory | 1.96 MiB |
| Windows threads | 3 |
| macOS idle CPU, observed sample | 0.2% |
| macOS reported memory | 27 MiB |
| macOS threads | 5 |
| Automatic 1 MiB Windows to macOS | 2.121 s |
| Automatic 1 MiB macOS to Windows | 6.700 s |

These are preliminary development measurements rather than
formal benchmarks. Application-level resource measurements
exclude SSH and netcat subprocess overhead.

## Security

Clipboard contents are transferred as data through SSH.
They are never interpolated into executable shell commands.

Automatic synchronization uses native clipboard APIs on
both operating systems and a private Unix-domain socket
on macOS, accessed through authenticated SSH.

The local socket directory and socket use restricted
filesystem permissions. Local IPC clients are checked
against the application user's identity.

Native clipboard change counters and conditional writes
mitigate clipboard races and prevent synchronization loops.

Simultaneous detected changes preserve both clipboards
rather than arbitrarily selecting a winning copy.

Conditional writes provide best-effort race protection,
not atomic cross-platform compare-and-swap semantics.

Uncertain writes are not automatically retried.

Privacy markers are detected on a best-effort basis.
Marked text is synchronized by default. Optional privacy
marker enforcement is planned for a future release.

Privacy markers themselves are not currently preserved
during text transfers.

The current foreground synchronization implementation
stops on transport failures. Automatic reconnection and
secure credential storage remain future milestones.

Only validated plain text is currently synchronized.
Rich text, images and file transfers remain future work.

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
- Best-effort privacy marker detection.
- Optional privacy marker enforcement, disabled by default.
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
- [x] M2.3: Automatic bidirectional text synchronization
- [x] M2.3A: Native clipboard metadata
- [x] M2.3B: Synchronization decision engine
- [x] M2.3C.1A: Counter-aware Windows clipboard writes
- [x] M2.3C.1B: Counter-aware macOS clipboard writes
- [x] M2.3C.2A: Conditional macOS IPC writes
- [x] M2.3C.2B: Rust conditional IPC integration
- [x] M2.3C.3: Foreground automatic synchronization
- [x] M2.3D: Automatic synchronization acceptance
- [x] M3: Complete native macOS menu bar interface
- [x] M3.1: Correct service and privacy status display
- [x] M3.2: Watcher activity reporting
- [x] M3.3: Dynamic menu bar icon
- [x] M3.4: Live clipboard metadata
- [x] M3.5: IPC transfer statistics
- [x] M3.6: Persistent preferences
- [x] M3.7: Menu bar integration acceptance
- [ ] M4: Native Windows system tray companion (interactive acceptance pending)
- [ ] M5: Reconnection and credential management
- [ ] M6: Multi-format clipboard protocol
- [ ] M7: Native image and rich-text synchronization
- [ ] M8: Streaming file transfer protocol
- [ ] M9: Transparent Explorer and Finder integration
- [ ] M10: Performance, security and reliability hardening
- [ ] M11: Production packaging and release