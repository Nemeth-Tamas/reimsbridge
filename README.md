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
- Versioned clipboard format negotiation with legacy text compatibility.
- Native RTF and HTML with plain-text fallback when present at the source.
- Native PNG clipboard images with Windows bitmap and macOS TIFF representations.
- Explorer and Finder file, folder, and multi-selection copy via bounded SFTP streaming.
- Optional launch at login on Windows and macOS.
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
- Automatic tray reconnection after temporary SSH and macOS IPC failures.
- Optional SSH credential storage in Windows Credential Manager.
- macOS menu bar icon for IPC state and recent watcher activity.
- macOS clipboard metadata and per-session IPC transfer statistics.
- Persistent macOS preferences for IPC startup and statistics visibility.
- No application-level clipboard history or content logging.

## Build and install locally

Build from the root of the repository on each machine. Both scripts
produce unsigned packages in `dist` and print the SHA-256 of the
resulting file.

### Windows x64

Use PowerShell 7, Rust and Cargo with the x64 MSVC target, Visual
Studio C++ Build Tools, Perl on `PATH`, and
[Inno Setup 6](https://jrsoftware.org/isdl.php).
The script finds `ISCC.exe` on `PATH` or in its default Inno Setup 6
installation directory. The first release build also compiles vendored
OpenSSL.

```powershell
git pull --ff-only origin master
pwsh -NoProfile -ExecutionPolicy RemoteSigned -File .\packaging\windows\build.ps1
```

Quit any running ReimsBridge tray. Double-click
`dist\ReimsBridge-Windows-x64-Setup.exe` in Explorer, then launch
**ReimsBridge** from the Start menu. The installer is per-user.

### Intel macOS guest

Use full Xcode with the macOS 26.5 SDK or newer and support for Intel
builds. The package build uses Xcode 26.6. The script runs
`xcodebuild` with signing disabled. Xcode can stay closed during the
build.

```bash
git pull --ff-only origin master
xcodebuild -version
bash packaging/macos/build.sh
```

If the active developer directory points to standalone Command Line
Tools, select your Xcode installation for the build. Adjust the path
if Xcode has a different name or location.

```bash
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer bash packaging/macos/build.sh
```

Quit any running ReimsBridge app. In Finder, extract
`dist/ReimsBridge-macOS-x86_64.zip` and drag `ReimsBridge.app` into
Applications. Open it from Finder. If macOS blocks the first launch,
follow [Apple’s Open Anyway instructions](https://support.apple.com/en-us/102445).

Start IPC in the macOS menu and connect from the Windows tray with
your macOS SSH account. Each app offers a login startup option.

### Other builds

For Windows development builds without an installer:

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
ReimsBridge and choose **Start Synchronization**. Enter your macOS SSH
username and password if prompted. Select the remember checkbox to save
the login in Windows Credential Manager after successful SSH
authentication. **Start with Different Credentials** opens a fresh
prompt. Use **Forget Saved Credentials** while stopped to delete the
saved login. If Credential Manager is unavailable, the Windows prompt
still lets you start synchronization.

The tray shows **Reconnecting** during temporary SSH or macOS IPC
outages. Retry delays start at one second and increase to 15 seconds.
Each connection verifies the SSH host key. Once IPC is ready, the watcher
takes a fresh clipboard baseline and preserves both current copies.
Host-key, authentication, and invalid IPC protocol failures stop the
watcher. **View Connection Details** shows the latest failure or any
credential storage warning.

**Stop Synchronization** cancels a pending retry or file transfer,
finishes any current clipboard polling step, and ends the SSH session.
The tray and foreground watcher share a single watcher slot, including
during `watch-test` and `watch-test-large`.

Choose **Enable Start at Login** in the Windows tray after saving your
SSH login. This registers the current tray executable for the current
Windows user and starts synchronization from the saved login when it
launches. Run the tray from a location you intend to keep.

### File and folder copy

Copy existing files or folders in Explorer or Finder. Wait for
**Ready to paste** in the Windows tray's **View Connection Details**
or the foreground watch log, then paste on the other desktop.
A selection can contain multiple files, folders, and empty folders.
The transfer uses SFTP over the existing authenticated SSH connection,
checks large-file free space where supported, and verifies file lengths.
The clipboard receives the staged file list only after the full transfer
and a fresh clipboard-counter check.

The app uses a fixed streaming buffer and has no fixed per-file byte
ceiling in this path. Destination free space, filesystem restrictions,
and supported names govern the transfer. The initial path accepts
ordinary files and directories and skips symbolic links, junctions,
and other reparse points. It preserves file data and hierarchy. File
permissions, extended attributes, alternate data streams, and other
filesystem metadata are outside this initial path.

Completed staged files remain under `%LOCALAPPDATA%\ReimsBridge\FileTransfers`
on Windows and `~/.reimsbridge/transfers` on macOS. Keep them while a
clipboard selection or paste still uses them. Transfers write into
`incoming-` directories and promote the completed batch before placing
the file list on the destination clipboard. The watcher cleans up
interrupted batches at startup or reconnect. Mac cleanup uses a
journal tied to the verified SSH host and the Mac staging path.

The file progress display reports bytes transferred, elapsed time,
and average throughput.

`watch-test` exercises a multi-root selection, nested and empty
directories, and byte-for-byte file checks across both native
clipboards. `watch-test-large` generates a 1 GiB fixture, copies
it both ways through the clipboard path, and verifies SHA-256.
Both tests replace both clipboards with their own fixtures. The
large test reads the Mac file back for verification, and temporarily
uses about 2 GiB on Windows and 1 GiB on macOS. It releases the test
clipboard references and removes its own fixture and staging copies
after a successful run.

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

### Clipboard format negotiation

The Mac status response includes an optional `formats` object with
`version`, `supported`, and `available` fields. `supported` lists
the formats the IPC service understands. `available` lists formats
advertised by the current Mac pasteboard. Format protocol v1 currently
registers `text-utf8`. The watcher uses legacy text commands if its
peer has no compatible format capability. Manual protocol 2 text
commands remain available.

A named read includes the observed pasteboard counter. A named write
includes the expected destination counter and the length in bytes.
Successful responses echo the format. Writes include one trailing
NUL byte after the declared payload.

```text
read-format <format> <expected-counter>\n
write-format-if <format> <expected-counter> <byte-count>\n<payload>\0
OK <format> <byte-count>\n<payload>   (read)
OK <format> <new-counter>\n           (write)
```

The UTF-8 text format keeps the 1 MiB text limit. Updated peers
also negotiate `clipboard-bundle-v1`. A bundle begins with four
big-endian 32-bit byte lengths, followed by the plain UTF-8, RTF, HTML,
and PNG representations in that order. A zero length marks an absent
representation. Plain-only copies continue to use the text path.

The in-memory bundle permits up to 1 MiB each for plain text, RTF, and
HTML, plus up to 8 MiB for PNG at up to 16,777,216 pixels. Windows
accepts registered PNG and common DIBs. macOS accepts PNG and converts
TIFF when needed. Transfers preserve the PNG and publish a native
bitmap representation on the destination.

`watch-test` reports the framing it negotiated, verifies stale named
reads and writes, and exercises rich text and a transparent PNG in
both directions when both ends support bundles.

### macOS menu bar

The macOS menu shows the current IPC state and recent Windows watcher
activity. Clipboard metadata refreshes each time the menu opens, including
while IPC is stopped. Transfer statistics also refresh on open. Clipboard
validation and the private-pasteboard self-test are available under
**Diagnostics**.

Under **Preferences**, enable **Start IPC on Launch** to start the
listener whenever ReimsBridge opens. Manual startup is the default.
**Show Transfer Statistics** controls whether the menu displays session
counters and is on by default. Both preferences persist across launches.
Quitting the app stops the listener.

To launch the unsigned app at login, open **Preferences → Open Login
Item Settings**. Under **Open at Login**, click **+** and add the
installed `ReimsBridge.app` from Applications. Keep **Start IPC on Launch**
enabled in ReimsBridge so the listener starts when the app opens.

**Enable Start at Login** also tries to register the app directly. If macOS
denies the request, ReimsBridge opens Login Item Settings for you. To undo
a login item you added in System Settings, select it there and click **−**.

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
cargo run -- watch-test-large <username>
```

Legacy pbcopy/pbpaste development commands:

```powershell
cargo run -- send <username>
cargo run -- receive <username>
cargo run -- roundtrip-test <username>
```

The foreground commands prompt for the password interactively.

## Testing

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo run -- clipboard-test
cargo run -- mac-conditional-test <username>
cargo run -- watch-test <username>
cargo run -- watch-test-large <username>
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

The foreground synchronization command stops on transport failures.
The tray keeps the SSH password in memory while its worker runs so it can
authenticate again after an outage. Remembered logins are stored in
Windows Credential Manager for the current Windows user on this machine.

Text, rich text, and bitmap images use validated clipboard representations.
File data streams through the host-key-verified SSH session using SFTP.

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
- Bidirectional file transfers of at least 1 GiB per file as an acceptance floor.
- Streaming transfers whose size is constrained by available storage and platform file limits.
- Extensible transport for additional clipboard representations, including opaque formats.
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

Large files and other large clipboard payloads require a bounded-memory
streaming path. The current in-memory limits apply to the early text
and bitmap transports. Native paste interoperability depends on the
receiving platform and application's support for the representation.

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
- [x] M4: Native Windows system tray companion
- [ ] M5: Reconnection and credential management (saved credentials verified, reconnect acceptance pending)
- [x] M6: Multi-format clipboard protocol
- [x] M7: Native image and rich-text synchronization
- [x] M8: Streaming file transfer protocol (1 GiB SHA-256 verified in both directions)
- [ ] M9: Transparent Explorer and Finder integration (native file lists passed, desktop paste check pending)
- [ ] M10: Performance, security and reliability hardening (large-file acceptance passed, interruption test pending)
- [ ] M11: Production packaging and release (unsigned packages built, desktop acceptance pending)