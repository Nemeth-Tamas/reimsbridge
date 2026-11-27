use crate::{clipboard, mac_ipc, sync_engine::Direction, sync_read::ReadOutcome};
use sha2::{Digest, Sha256};
use ssh2::{ErrorCode, HashType, OpenFlags, OpenType, RenameFlags, Session, Sftp};
use std::collections::HashSet;
use std::error::Error;
use std::ffi::OsStr;
use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, GetDiskFreeSpaceExW};

const BUFFER_BYTES: usize = 256 * 1024;
const MAX_DEPTH: usize = 128;
const REPARSE_POINT: u32 = 0x400;
const SPACE_CHECK_THRESHOLD: u64 = 32 * 1024 * 1024;
static BATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub enum ResultState {
    MacPublished(i64),
    WindowsPublished(u32),
    Changed,
    Unavailable,
}

pub struct TransferOptions<'a> {
    pub stop: Option<&'a AtomicBool>,
    pub notice: Option<&'a dyn Fn(&str)>,
}

#[derive(Debug)]
pub struct SelectionChanged;

impl std::fmt::Display for SelectionChanged {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Clipboard changed during file transfer")
    }
}

impl Error for SelectionChanged {}

pub fn transfer(
    session: &Session,
    direction: Direction,
    windows_counter: u32,
    mac_counter: i64,
    options: TransferOptions<'_>,
    still_current: impl Fn() -> Result<bool, Box<dyn Error>>,
) -> Result<ResultState, Box<dyn Error>> {
    let paths = match direction {
        Direction::WindowsToMac => clipboard::read_files(windows_counter)?,
        Direction::MacToWindows => match mac_ipc::file_offer(session, mac_counter)? {
            ReadOutcome::Data(paths) => {
                ReadOutcome::Data(paths.into_iter().map(PathBuf::from).collect())
            }
            ReadOutcome::Unavailable => ReadOutcome::Unavailable,
            ReadOutcome::Changed => ReadOutcome::Changed,
        },
    };
    let paths = match paths {
        ReadOutcome::Data(paths) => paths,
        ReadOutcome::Changed => return Ok(ResultState::Changed),
        ReadOutcome::Unavailable => return Ok(ResultState::Unavailable),
    };
    let mut names = HashSet::new();
    let roots = paths
        .iter()
        .map(|path| {
            let name = if direction == Direction::WindowsToMac {
                if !path.is_absolute() {
                    return Err("Windows file list contains a relative path".into());
                }
                portable_name(path.file_name().ok_or("Cannot copy a filesystem root")?)?
            } else {
                let path = validate_mac_path(path)?;
                portable_name(OsStr::new(
                    path.rsplit('/')
                        .next()
                        .ok_or("Cannot copy a filesystem root")?,
                ))?
            };
            if !names.insert(name.to_lowercase()) {
                return Err("File selection contains conflicting destination names".into());
            }
            Ok((path, name))
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let sftp = session.sftp()?;
    let batch = batch_id()?;
    let mut progress = Progress::new(options, &still_current);
    progress.report("Transferring clipboard files");
    if !still_current()? {
        return Ok(ResultState::Changed);
    }

    match direction {
        Direction::WindowsToMac => {
            let root = mac_ipc::file_stage(session)?;
            validate_mac_path(&root)?;
            let destination = remote_child(&root, &batch)?;
            let pending = remote_child(&root, &format!("incoming-{batch}"))?;
            let journal = journal_root(session, &root)?.join(&batch);
            let mut stage = RemoteStage::create(&sftp, pending, journal)?;
            for (source, name) in roots {
                upload(
                    &sftp,
                    source,
                    &remote_child(&stage.path, name)?,
                    0,
                    &mut progress,
                )?;
            }
            progress.check_cancelled()?;
            if !still_current()? {
                return Ok(ResultState::Changed);
            }

            stage.promote(destination)?;
            progress.check_cancelled()?;
            if !still_current()? {
                return Ok(ResultState::Changed);
            }
            // A lost acknowledgement can leave the clipboard pointing here.
            stage.keep = true;
            let result = mac_ipc::file_publish(session, mac_counter, &batch)?;
            if let Some(counter) = result {
                progress.complete();
                Ok(ResultState::MacPublished(counter))
            } else {
                stage.keep = false;
                Ok(ResultState::Changed)
            }
        }
        Direction::MacToWindows => {
            let root = windows_stage_root()?;
            let mut stage = LocalStage::create(root.join(format!("incoming-{batch}")))?;
            for (source, name) in &roots {
                download(&sftp, source, &stage.path.join(name), 0, &mut progress)?;
            }
            progress.check_cancelled()?;
            if !still_current()? {
                return Ok(ResultState::Changed);
            }

            stage.promote(root.join(&batch))?;
            progress.check_cancelled()?;
            if !still_current()? {
                return Ok(ResultState::Changed);
            }
            let published: Vec<_> = roots
                .iter()
                .map(|(_, name)| stage.path.join(name))
                .collect();
            stage.keep = true;
            let result = clipboard::write_files_checked(&published, windows_counter)?;
            if let Some(counter) = result {
                progress.complete();
                Ok(ResultState::WindowsPublished(counter))
            } else {
                stage.keep = false;
                Ok(ResultState::Changed)
            }
        }
    }
}

pub fn batch_id() -> Result<String, Box<dyn Error>> {
    let time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let sequence = BATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(format!(
        "batch-{:x}-{time:x}-{sequence:x}",
        std::process::id()
    ))
}

fn validate_mac_path(path: &Path) -> Result<&str, Box<dyn Error>> {
    let value = path.to_str().ok_or("Mac file path is not Unicode")?;
    if !value.starts_with('/')
        || value.contains(['\\', '\0'])
        || value[1..]
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("Mac file selection contains an invalid absolute path".into());
    }
    Ok(value)
}

fn portable_name(name: &OsStr) -> Result<&str, Box<dyn Error>> {
    let name = name.to_str().ok_or("File name is not Unicode")?;
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.ends_with([' ', '.'])
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
    {
        return Err("A file or folder name is incompatible with the destination".into());
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    }) {
        return Err("A file name is reserved on Windows".into());
    }
    Ok(name)
}

fn remote_child(parent: &Path, name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let parent = validate_mac_path(parent)?;
    Ok(PathBuf::from(format!("{parent}/{name}")))
}

fn service_root() -> Result<PathBuf, Box<dyn Error>> {
    let base = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA unavailable")?);
    if !base.is_absolute() {
        return Err("LOCALAPPDATA is not absolute".into());
    }
    let service = base.join("ReimsBridge");
    ensure_directory(&service)?;
    Ok(service)
}

fn windows_stage_root() -> Result<PathBuf, Box<dyn Error>> {
    let root = service_root()?.join("FileTransfers");
    ensure_directory(&root)?;
    Ok(root)
}

fn ensure_directory(directory: &Path) -> Result<(), Box<dyn Error>> {
    match fs::create_dir(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    if !ordinary_metadata(directory)?.is_dir() {
        return Err("ReimsBridge staging path is not a directory".into());
    }
    Ok(())
}

fn journal_root(session: &Session, remote_root: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let journal = service_root()?.join("TransferJournal");
    ensure_directory(&journal)?;
    let host_key = session
        .host_key_hash(HashType::Sha256)
        .ok_or("SSH host key hash unavailable")?;
    let mut identity = Sha256::new();
    identity.update(host_key);
    identity.update(validate_mac_path(remote_root)?.as_bytes());
    let account = journal.join(format!("{:x}", identity.finalize()));
    ensure_directory(&account)?;
    Ok(account)
}

fn valid_batch(batch: &str) -> bool {
    let Some(value) = batch.strip_prefix("batch-") else {
        return false;
    };
    let mut components = value.split('-');
    let valid = (0..3).all(|_| {
        components.next().is_some_and(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
    });
    valid && components.next().is_none() && batch.len() <= 64
}

pub fn recover_interrupted(session: &Session, stop: &AtomicBool) -> Result<usize, Box<dyn Error>> {
    let mut cleaned = 0;
    let local = windows_stage_root()?;
    for entry in fs::read_dir(local)? {
        if stop.load(Ordering::Relaxed) {
            return Ok(cleaned);
        }
        let entry = entry?;
        let name = entry.file_name();
        let pending = name
            .to_str()
            .and_then(|name| name.strip_prefix("incoming-"));
        if pending.is_some_and(valid_batch)
            && ordinary_metadata(&entry.path()).is_ok_and(|stat| stat.is_dir())
        {
            fs::remove_dir_all(entry.path())?;
            cleaned += 1;
        }
    }

    let remote = mac_ipc::file_stage(session)?;
    let journal = journal_root(session, &remote)?;
    let mut sftp = None;
    for entry in fs::read_dir(journal)? {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let entry = entry?;
        let name = entry.file_name();
        let Some(batch) = name.to_str().filter(|name| valid_batch(name)) else {
            continue;
        };
        if !ordinary_metadata(&entry.path()).is_ok_and(|stat| stat.is_file() && stat.len() == 0) {
            continue;
        }

        let transport = match &sftp {
            Some(transport) => transport,
            None => sftp.insert(session.sftp()?),
        };
        let pending = remote_child(&remote, &format!("incoming-{batch}"))?;
        if clear_remote_pending(transport, &pending)? {
            cleaned += 1;
        }
        fs::remove_file(entry.path())?;
    }
    Ok(cleaned)
}

pub fn clear_test_staging(
    session: &Session,
    windows_file: &Path,
    mac_file: &Path,
) -> Result<(), Box<dyn Error>> {
    let windows_stage = windows_file
        .parent()
        .ok_or("Windows test staging path unavailable")?;
    let mac_stage = validate_mac_path(mac_file)?
        .rsplit_once('/')
        .ok_or("Mac test staging path unavailable")?
        .0;
    let (mac_root, mac_batch) = mac_stage
        .rsplit_once('/')
        .ok_or("Mac test batch path unavailable")?;
    let windows_batch = windows_stage
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or("Windows test batch name unavailable")?;
    if !valid_batch(windows_batch)
        || !valid_batch(mac_batch)
        || windows_stage.parent() != Some(windows_stage_root()?.as_path())
        || Path::new(mac_root) != mac_ipc::file_stage(session)?
        || !ordinary_metadata(windows_stage)?.is_dir()
    {
        return Err("Test staging cleanup paths are outside their expected directories".into());
    }

    fs::remove_dir_all(windows_stage)?;
    clear_remote_pending(&session.sftp()?, Path::new(mac_stage))?;
    Ok(())
}

fn clear_remote_pending(sftp: &Sftp, pending: &Path) -> Result<bool, Box<dyn Error>> {
    match sftp.lstat(pending) {
        Ok(stat) if stat.is_dir() => {
            remove_remote_tree(sftp, pending, 0)?;
            Ok(true)
        }
        Ok(_) => Err("Remote pending path was not an ordinary directory".into()),
        Err(error)
            if matches!(
                error.code(),
                ErrorCode::SFTP(
                    libssh2_sys::LIBSSH2_FX_NO_SUCH_FILE | libssh2_sys::LIBSSH2_FX_NO_SUCH_PATH
                )
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

fn ordinary_metadata(path: &Path) -> Result<Metadata, Box<dyn Error>> {
    let stat = fs::symlink_metadata(path)?;
    if stat.file_attributes() & REPARSE_POINT != 0 {
        return Err(
            "Symbolic links, junctions, and reparse points are not supported for file copy".into(),
        );
    }
    Ok(stat)
}

pub(crate) fn check_windows_space(file: &Path, length: u64) -> Result<(), Box<dyn Error>> {
    if length < SPACE_CHECK_THRESHOLD {
        return Ok(());
    }
    let parent = file.parent().ok_or("Windows staging parent unavailable")?;
    let directory: Vec<u16> = parent.as_os_str().encode_wide().chain([0]).collect();
    let mut available = 0;
    if unsafe {
        GetDiskFreeSpaceExW(
            directory.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error().into());
    }
    if length > available {
        return Err("Insufficient space for the file in Windows staging".into());
    }
    Ok(())
}

fn check_mac_space(file: &mut ssh2::File, length: u64) -> Result<(), Box<dyn Error>> {
    if length < SPACE_CHECK_THRESHOLD {
        return Ok(());
    }
    let space = match file.statvfs() {
        Ok(space) => space,
        Err(error)
            if matches!(
                error.code(),
                ErrorCode::SFTP(libssh2_sys::LIBSSH2_FX_OP_UNSUPPORTED)
                    | ErrorCode::Session(libssh2_sys::LIBSSH2_ERROR_METHOD_NOT_SUPPORTED)
            ) =>
        {
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    if length > space.f_bavail.saturating_mul(space.f_frsize) {
        return Err("Insufficient space for the file in Mac staging".into());
    }
    Ok(())
}

struct LocalStage {
    path: PathBuf,
    keep: bool,
}

impl LocalStage {
    fn create(path: PathBuf) -> io::Result<Self> {
        fs::create_dir(&path)?;
        Ok(Self { path, keep: false })
    }

    fn promote(&mut self, destination: PathBuf) -> io::Result<()> {
        fs::rename(&self.path, &destination)?;
        self.path = destination;
        Ok(())
    }
}

impl Drop for LocalStage {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct RemoteStage<'a> {
    sftp: &'a Sftp,
    path: PathBuf,
    journal: Option<PathBuf>,
    keep: bool,
}

impl<'a> RemoteStage<'a> {
    fn create(sftp: &'a Sftp, path: PathBuf, journal: PathBuf) -> Result<Self, Box<dyn Error>> {
        sftp.mkdir(&path, 0o700)?;
        let mut stage = Self {
            sftp,
            path,
            journal: None,
            keep: false,
        };
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&journal)?;
        // The journal exists before the first file byte is written.
        stage.journal = Some(journal);
        Ok(stage)
    }

    fn promote(&mut self, destination: PathBuf) -> Result<(), ssh2::Error> {
        self.sftp
            .rename(&self.path, &destination, Some(RenameFlags::empty()))?;
        self.path = destination;
        self.clear_journal();
        Ok(())
    }

    fn clear_journal(&mut self) {
        if let Some(journal) = self.journal.take() {
            let _ = fs::remove_file(journal);
        }
    }
}

impl Drop for RemoteStage<'_> {
    fn drop(&mut self) {
        if !self.keep && clear_remote_pending(self.sftp, &self.path).is_ok() {
            self.clear_journal();
        }
    }
}

fn is_dot(path: &Path) -> bool {
    path == Path::new(".") || path == Path::new("..")
}

fn dir_entry(dir: &mut ssh2::File) -> Result<Option<PathBuf>, ssh2::Error> {
    loop {
        match dir.readdir() {
            Ok((path, _)) if is_dot(&path) => continue,
            Ok((path, _)) => return Ok(Some(path)),
            Err(error) if error.code() == ErrorCode::Session(libssh2_sys::LIBSSH2_ERROR_FILE) => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
    }
}

fn remove_remote_tree(sftp: &Sftp, path: &Path, depth: usize) -> Result<(), Box<dyn Error>> {
    if depth > MAX_DEPTH {
        return Err("Remote cleanup depth exceeded".into());
    }
    if sftp.lstat(path)?.is_dir() {
        let mut dir = sftp.opendir(path)?;
        while let Some(child) = dir_entry(&mut dir)? {
            let name = portable_name(child.as_os_str())?;
            remove_remote_tree(sftp, &remote_child(path, name)?, depth + 1)?;
        }
        drop(dir);
        sftp.rmdir(path)?;
    } else {
        sftp.unlink(path)?;
    }
    Ok(())
}

fn upload(
    sftp: &Sftp,
    source: &Path,
    destination: &Path,
    depth: usize,
    progress: &mut Progress<'_>,
) -> Result<(), Box<dyn Error>> {
    progress.start_entry(depth)?;
    let stat = ordinary_metadata(source)?;
    if stat.is_dir() {
        sftp.mkdir(destination, 0o700)?;
        let mut names = HashSet::new();
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let filename = entry.file_name();
            let name = portable_name(&filename)?;
            if !names.insert(name.to_lowercase()) {
                return Err("Folder has conflicting destination names".into());
            }
            upload(
                sftp,
                &entry.path(),
                &remote_child(destination, name)?,
                depth + 1,
                progress,
            )?;
        }
    } else if stat.is_file() {
        let mut input = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(source)?;
        let opened = input.metadata()?;
        if !opened.is_file()
            || opened.file_attributes() & REPARSE_POINT != 0
            || opened.len() != stat.len()
            || opened.modified()? != stat.modified()?
        {
            return Err("Windows source changed before it could be opened".into());
        }
        let mut output = sftp.open_mode(
            destination,
            OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUSIVE,
            0o600,
            OpenType::File,
        )?;
        check_mac_space(&mut output, stat.len())?;
        progress.copy(&mut input, &mut output, stat.len())?;
        output.close()?;
        let current = input.metadata()?;
        if current.len() != stat.len()
            || current.modified()? != stat.modified()?
            || sftp.lstat(destination)?.size != Some(stat.len())
        {
            return Err("Source file changed or staged file length did not match".into());
        }
    } else {
        return Err("File selection includes an unsupported filesystem object".into());
    }
    Ok(())
}

fn download(
    sftp: &Sftp,
    source: &Path,
    destination: &Path,
    depth: usize,
    progress: &mut Progress<'_>,
) -> Result<(), Box<dyn Error>> {
    progress.start_entry(depth)?;
    let stat = sftp.lstat(source)?;
    if stat.is_dir() {
        fs::create_dir(destination)?;
        let mut dir = sftp.opendir(source)?;
        while let Some(child) = dir_entry(&mut dir)? {
            let name = portable_name(child.as_os_str())?;
            download(
                sftp,
                &remote_child(source, name)?,
                &destination.join(name),
                depth + 1,
                progress,
            )?;
        }
    } else if stat.is_file() {
        let length = stat.size.ok_or("Remote file length unavailable")?;
        check_windows_space(destination, length)?;
        let mut input = sftp.open(source)?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        progress.copy(&mut input, &mut output, length)?;
        output.flush()?;
        let current = input.stat()?;
        if current.size != Some(length)
            || current.mtime != stat.mtime
            || output.metadata()?.len() != length
        {
            return Err("Source file changed or staged file length did not match".into());
        }
    } else {
        return Err(
            "Mac file selection includes a symbolic link or unsupported filesystem object".into(),
        );
    }
    Ok(())
}

struct Progress<'a> {
    options: TransferOptions<'a>,
    still_current: &'a dyn Fn() -> Result<bool, Box<dyn Error>>,
    buffer: Vec<u8>,
    bytes: u64,
    started: Instant,
    last_report: Instant,
}

impl<'a> Progress<'a> {
    fn new(
        options: TransferOptions<'a>,
        still_current: &'a dyn Fn() -> Result<bool, Box<dyn Error>>,
    ) -> Self {
        Self {
            options,
            still_current,
            buffer: vec![0; BUFFER_BYTES],
            bytes: 0,
            started: Instant::now(),
            last_report: Instant::now(),
        }
    }

    fn report(&self, text: &str) {
        println!("[FILES] {text}");
        if let Some(notice) = self.options.notice {
            notice(text);
        }
    }

    fn complete(&self) {
        self.report(&format!(
            "Ready to paste: {} bytes staged in {:.2}s ({:.2} MiB/s)",
            self.bytes,
            self.started.elapsed().as_secs_f64(),
            self.rate()
        ));
    }

    fn rate(&self) -> f64 {
        (self.bytes as f64 / 1_048_576.0) / self.started.elapsed().as_secs_f64().max(0.001)
    }

    fn check_cancelled(&self) -> io::Result<()> {
        if self
            .options
            .stop
            .is_some_and(|stop| stop.load(Ordering::Relaxed))
        {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "File transfer stopped",
            ));
        }
        Ok(())
    }

    fn start_entry(&mut self, depth: usize) -> Result<(), Box<dyn Error>> {
        self.check_cancelled()?;
        if depth > MAX_DEPTH {
            return Err("File selection exceeds the supported directory nesting depth".into());
        }
        self.check_and_report_progress()
    }

    fn copy(
        &mut self,
        input: &mut impl Read,
        output: &mut impl Write,
        expected: u64,
    ) -> Result<(), Box<dyn Error>> {
        let mut remaining = expected;
        while remaining > 0 {
            self.check_cancelled()?;
            let limit = remaining.min(self.buffer.len() as u64) as usize;
            let read = input.read(&mut self.buffer[..limit])?;
            if read == 0 {
                return Err("Source file shrank during transfer".into());
            }
            output.write_all(&self.buffer[..read])?;
            remaining -= read as u64;
            self.bytes = self.bytes.saturating_add(read as u64);

            self.check_and_report_progress()?;
        }
        self.check_cancelled()?;
        if input.read(&mut self.buffer[..1])? != 0 {
            return Err("Source file grew during transfer".into());
        }
        Ok(())
    }

    fn check_and_report_progress(&mut self) -> Result<(), Box<dyn Error>> {
        if self.last_report.elapsed() >= Duration::from_secs(2) {
            if !(self.still_current)()? {
                return Err(SelectionChanged.into());
            }
            self.report(&format!(
                "Transferring clipboard files: {} MiB ({:.2} MiB/s)",
                self.bytes / 1_048_576,
                self.rate()
            ));
            self.last_report = Instant::now();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_escape_or_alias_windows_devices() {
        for input in [
            "..",
            "outside\\file",
            "name:stream",
            "foo. ",
            "NUL.txt",
            "lpt¹",
            "/tmp",
        ] {
            assert!(portable_name(OsStr::new(input)).is_err(), "{input}");
        }
        assert_eq!(
            portable_name(OsStr::new("Árvíztűrő 🚀.bin")).unwrap(),
            "Árvíztűrő 🚀.bin"
        );
        assert!(validate_mac_path(Path::new("/Users/test/../escape")).is_err());
    }

    #[test]
    fn bytes_are_streamed_and_length_changes_detected() {
        let bytes = vec![0xac; BUFFER_BYTES * 3 + 1];
        let mut progress = Progress::new(
            TransferOptions {
                stop: None,
                notice: None,
            },
            &|| Ok(true),
        );
        let mut output = Vec::new();
        progress
            .copy(&mut bytes.as_slice(), &mut output, bytes.len() as u64)
            .unwrap();
        assert_eq!(output, bytes);
        assert!(progress.copy(&mut &[1, 2][..], &mut io::sink(), 1).is_err());
        assert!(progress.copy(&mut &[1, 2][..], &mut io::sink(), 3).is_err());
    }

    #[test]
    fn streams_beyond_four_gib_with_one_buffer() {
        let length = (4u64 << 30) + 33;
        let mut progress = Progress::new(
            TransferOptions {
                stop: None,
                notice: None,
            },
            &|| Ok(true),
        );
        progress
            .copy(&mut io::repeat(0x42).take(length), &mut io::sink(), length)
            .unwrap();
        assert_eq!(progress.bytes, length);
        assert_eq!(progress.buffer.len(), BUFFER_BYTES);
    }

    #[test]
    fn staging_is_promoted_and_unpublished_batches_are_removed() {
        let parent =
            std::env::temp_dir().join(format!("ReimsBridge-staging-{}", batch_id().unwrap()));
        fs::create_dir(&parent).unwrap();
        let pending = parent.join("incoming-batch-1-2-3");
        let ready = parent.join("batch-1-2-3");
        {
            let mut stage = LocalStage::create(pending.clone()).unwrap();
            fs::write(stage.path.join("fixture"), b"ready").unwrap();
            stage.promote(ready.clone()).unwrap();
            stage.keep = true;
        }
        assert!(!pending.exists());
        assert_eq!(fs::read(ready.join("fixture")).unwrap(), b"ready");

        {
            let stage = LocalStage::create(pending.clone()).unwrap();
            fs::write(stage.path.join("fixture"), b"partial").unwrap();
        }
        assert!(!pending.exists());
        assert!(ready.is_dir());
        assert!(valid_batch("batch-1a-02-f00"));
        assert!(!valid_batch("batch-1-../2-3"));
        assert!(!valid_batch("batch-1-2-3-extra"));
        fs::remove_dir_all(parent).unwrap();
    }
}
