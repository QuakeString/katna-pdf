// SPDX-License-Identifier: GPL-3.0-or-later

//! One Katna PDF per person (docs/PLAN.md §7, Phase 0): opening a file
//! while Katna PDF runs opens it as a tab in the running window, as
//! SumatraPDF and Okular do, instead of starting a second app.
//!
//! The first instance holds a lock file for as long as it runs and
//! listens on a loopback port with a random token, both written in the
//! person's own runtime folder. A later launch finds the lock taken, reads
//! the port and token, sends its files and exits. The operating system
//! drops the lock when a process ends, however it ends, so a crashed
//! instance never blocks the next one. Plain `std`, the same on Linux and
//! Windows. No GPUI here.

use std::fs::{File, TryLockError};
use std::hash::{BuildHasher, RandomState};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_channel::{Receiver, Sender};

/// The first line of every hand-off, before the token.
const HELLO: &str = "katna-pdf 1";
/// A hand-off longer than this is not one of ours.
const MAX_REQUEST: u64 = 256 * 1024;
/// How long a new launch waits for the running one, at each step.
const WAIT: Duration = Duration::from_secs(3);

/// Something asked of the running app by a later launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    /// Files to open, as absolute paths. None: just come to the front.
    pub files: Vec<PathBuf>,
    /// The launcher's activation token, so the window may take focus on
    /// Wayland (`XDG_ACTIVATION_TOKEN`).
    pub activation_token: Option<String>,
}

/// How this process started.
pub enum Started {
    /// This is the app. Later launches' requests arrive on the receiver;
    /// `None` when the hand-off could not be set up (the app then simply
    /// runs on its own).
    Running(Option<Receiver<Request>>),
    /// The running app took the request; exit.
    HandedOff,
}

/// Becomes the running instance, or hands `files` to the one already
/// running.
pub fn start(files: &[PathBuf]) -> Started {
    match runtime_dir() {
        Some(dir) => start_in(&dir, &crate::ids::app_id(), files),
        None => Started::Running(None),
    }
}

/// [`start`] with its lock and port files in `dir`, named after `name`.
fn start_in(dir: &Path, name: &str, files: &[PathBuf]) -> Started {
    if std::fs::create_dir_all(dir).is_err() {
        return Started::Running(None);
    }
    let lock_path = dir.join(format!("{name}.lock"));
    let port_path = dir.join(format!("{name}.port"));
    let Ok(lock) = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
    else {
        return Started::Running(None);
    };
    match lock.try_lock() {
        Ok(()) => Started::Running(serve(lock, &port_path)),
        Err(TryLockError::WouldBlock) => {
            let request = Request {
                files: files.to_vec(),
                activation_token: std::env::var("XDG_ACTIVATION_TOKEN").ok(),
            };
            if hand_off(&port_path, &request) {
                Started::HandedOff
            } else {
                // The running one did not answer: run beside it.
                Started::Running(None)
            }
        }
        Err(TryLockError::Error(_)) => Started::Running(None),
    }
}

/// Listens for later launches; the lock is held by the listening thread
/// until the process ends.
fn serve(lock: File, port_path: &Path) -> Option<Receiver<Request>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    let token = new_token();
    write_private(port_path, format!("{port} {token}\n").as_bytes()).ok()?;
    let (sender, requests) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("katna-pdf-instance".into())
        .spawn(move || {
            let _lock = lock;
            for stream in listener.incoming().flatten() {
                answer(stream, &token, &sender);
            }
        })
        .ok()?;
    Some(requests)
}

/// Reads one launch's request and passes it on if its token is right.
fn answer(stream: TcpStream, token: &str, sender: &Sender<Request>) {
    let _ = stream.set_read_timeout(Some(WAIT));
    let _ = stream.set_write_timeout(Some(WAIT));
    let mut text = String::new();
    let Ok(reader) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(reader.take(MAX_REQUEST));
    loop {
        let before = text.len();
        match reader.read_line(&mut text) {
            Ok(0) | Err(_) => return,
            Ok(_) if text[before..].trim_end() == "end" => break,
            Ok(_) => {}
        }
    }
    if let Some(request) = parse(&text, token)
        && sender.send_blocking(request).is_ok()
    {
        let mut stream = stream;
        let _ = stream.write_all(b"ok\n");
    }
}

/// Sends `request` to the instance whose port file is `port_path`;
/// `false` if it did not take it.
fn hand_off(port_path: &Path, request: &Request) -> bool {
    let Some(message) = encode(request) else {
        return false;
    };
    // The first instance may have the lock but not yet its port file.
    let mut port_file = None;
    for _ in 0..40 {
        if let Ok(text) = std::fs::read_to_string(port_path) {
            port_file = Some(text);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let Some((port, token)) = port_file.as_deref().and_then(|t| {
        let (port, token) = t.trim().split_once(' ')?;
        Some((port.parse::<u16>().ok()?, token.to_owned()))
    }) else {
        return false;
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, WAIT) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(WAIT * 2));
    let _ = stream.set_write_timeout(Some(WAIT));
    let message = format!("{HELLO} {token}\n{message}");
    if stream.write_all(message.as_bytes()).is_err() {
        return false;
    }
    let mut reply = String::new();
    BufReader::new(stream.take(16))
        .read_line(&mut reply)
        .is_ok_and(|_| reply.trim_end() == "ok")
}

/// The request's lines after the hello, ending with `end`. `None` when a
/// path cannot be written as one line of UTF-8; such a file is opened in a
/// window of its own instead.
fn encode(request: &Request) -> Option<String> {
    let mut out = String::new();
    if let Some(token) = &request.activation_token
        && !token.contains(['\n', '\r'])
    {
        out.push_str(&format!("activation {token}\n"));
    }
    for file in &request.files {
        let file = file.to_str()?;
        if file.contains(['\n', '\r']) {
            return None;
        }
        out.push_str(&format!("open {file}\n"));
    }
    out.push_str("end\n");
    Some(out)
}

/// A request as [`hand_off`] sends it, if it carries `token`.
fn parse(text: &str, token: &str) -> Option<Request> {
    let mut lines = text.lines();
    let hello = lines.next()?.strip_prefix(HELLO)?.strip_prefix(' ')?;
    if hello != token {
        return None;
    }
    let mut request = Request::default();
    for line in lines {
        match line.split_once(' ') {
            Some(("open", path)) => request.files.push(PathBuf::from(path)),
            Some(("activation", t)) => request.activation_token = Some(t.to_owned()),
            _ if line == "end" => return Some(request),
            _ => {}
        }
    }
    None
}

/// 128 random bits as hex, from the operating system's randomness that
/// seeds `std`'s hash maps.
fn new_token() -> String {
    let a = RandomState::new().hash_one(std::process::id());
    let b = RandomState::new().hash_one(std::time::SystemTime::now());
    format!("{a:016x}{b:016x}")
}

/// Writes `bytes` to `path` whole, readable by this person only.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let partial = path.with_extension("port.partial");
    let mut options = File::options();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&partial)?.write_all(bytes)?;
    std::fs::rename(partial, path)
}

/// Where the lock and port files live: the session's runtime folder on
/// Linux, the person's temporary folder on Windows (both private to them).
fn runtime_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
            return Some(PathBuf::from(dir).join("katna-pdf"));
        }
        let home = std::env::var_os("HOME").filter(|d| !d.is_empty())?;
        Some(PathBuf::from(home).join(".cache").join("katna-pdf"))
    }
    #[cfg(not(unix))]
    {
        Some(std::env::temp_dir().join("katna-pdf"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("katna-pdf-instance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn requests_round_trip() {
        let request = Request {
            files: vec!["/home/kay/Plan A.pdf".into(), "/tmp/b.pdf".into()],
            activation_token: Some("abc_123".into()),
        };
        let text = format!("{HELLO} t0k\n{}", encode(&request).unwrap());
        assert_eq!(parse(&text, "t0k"), Some(request));
        assert_eq!(parse(&text, "other"), None, "a wrong token is refused");
        assert_eq!(parse(&format!("{HELLO} t0k\nopen /a.pdf\n"), "t0k"), None);
    }

    #[test]
    fn paths_that_are_not_one_line_stay_here() {
        let request = Request {
            files: vec!["/tmp/two\nlines.pdf".into()],
            activation_token: None,
        };
        assert_eq!(encode(&request), None);
    }

    #[test]
    fn a_second_launch_hands_its_files_to_the_first() {
        let dir = scratch("hand-off");
        let Started::Running(Some(requests)) = start_in(&dir, "test", &[]) else {
            panic!("the first launch should run and listen");
        };
        let files = vec![PathBuf::from("/tmp/a.pdf")];
        assert!(matches!(start_in(&dir, "test", &files), Started::HandedOff));
        assert_eq!(requests.recv_blocking().unwrap().files, files);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_token_gets_no_answer() {
        let dir = scratch("token");
        let Started::Running(Some(requests)) = start_in(&dir, "test", &[]) else {
            panic!("the first launch should run and listen");
        };
        std::fs::write(
            dir.join("test.port"),
            std::fs::read_to_string(dir.join("test.port"))
                .unwrap()
                .replace(' ', " x"),
        )
        .unwrap();
        assert!(!hand_off(&dir.join("test.port"), &Request::default()));
        assert!(requests.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
