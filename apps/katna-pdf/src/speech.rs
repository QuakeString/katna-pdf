// SPDX-License-Identifier: GPL-3.0-or-later

//! The desktop's voice, for reading aloud (docs/PLAN.md §4.1, P1). On
//! Linux that is Speech Dispatcher, spoken to over its socket in its own
//! plain-text protocol (SSIP), or `espeak-ng` when it is not running; on
//! Windows, the system's voice through PowerShell. No C libraries: each
//! is a socket or a program.
//!
//! [`Voice::say`] blocks until the words are spoken or [`Voice::stop`] is
//! called from another thread, so a reader can go a sentence at a time
//! and know where it is.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

/// A voice to speak with.
pub struct Voice {
    kind: Kind,
}

enum Kind {
    /// Speech Dispatcher: the connection, read and written apart so a
    /// stop can be sent while a sentence is spoken.
    #[cfg(unix)]
    Ssip {
        reader: Mutex<BufReader<std::os::unix::net::UnixStream>>,
        writer: Mutex<std::os::unix::net::UnixStream>,
    },
    /// A program run for each sentence (`espeak-ng`).
    #[cfg_attr(windows, allow(dead_code))]
    Program {
        program: String,
        running: Mutex<Option<Child>>,
    },
    /// One PowerShell kept running, a sentence a line on its input and
    /// a line back when it is spoken. Its answers are read apart from the
    /// process, so a stop can end it while a sentence is spoken.
    #[cfg_attr(not(windows), allow(dead_code))]
    PowerShell {
        child: Mutex<Option<Child>>,
        answers: Mutex<Option<BufReader<std::process::ChildStdout>>>,
    },
}

/// Speaks each line of its input and answers `done` after each.
#[cfg_attr(not(windows), allow(dead_code))]
const POWERSHELL: &str = "[Console]::InputEncoding = [Text.Encoding]::UTF8; Add-Type -AssemblyName System.Speech; \
    $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
    while (($l = [Console]::In.ReadLine()) -ne $null) { $s.Speak($l); \
    [Console]::Out.WriteLine('done'); [Console]::Out.Flush() }";

/// Windows' flag for a program started without a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The app's one voice, opened the first time it is asked for: slow (it
/// may start Speech Dispatcher), so asked for off the UI thread.
pub fn voice() -> Option<&'static Voice> {
    static VOICE: std::sync::OnceLock<Option<Voice>> = std::sync::OnceLock::new();
    VOICE.get_or_init(Voice::open).as_ref()
}

impl Voice {
    /// The desktop's voice, when it has one.
    pub fn open() -> Option<Voice> {
        #[cfg(windows)]
        {
            Some(Voice {
                kind: Kind::PowerShell {
                    child: Mutex::new(None),
                    answers: Mutex::new(None),
                },
            })
        }
        #[cfg(unix)]
        {
            if let Some(voice) = ssip() {
                return Some(voice);
            }
            for program in ["espeak-ng", "espeak"] {
                if Command::new(program)
                    .arg("--version")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
                {
                    return Some(Voice {
                        kind: Kind::Program {
                            program: program.to_owned(),
                            running: Mutex::new(None),
                        },
                    });
                }
            }
            None
        }
    }

    /// Speaks `text`, returning once it is spoken or stopped; `Err` when
    /// the voice went away.
    pub fn say(&self, text: &str) -> Result<(), ()> {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return Ok(());
        }
        match &self.kind {
            #[cfg(unix)]
            Kind::Ssip { reader, writer } => {
                {
                    let mut w = writer.lock().map_err(|_| ())?;
                    send(&mut *w, "SPEAK").map_err(|_| ())?;
                }
                let mut reader = reader.lock().map_err(|_| ())?;
                // `230 OK RECEIVING DATA` first, then the text.
                wait_for(&mut *reader, |code| code == 230)?;
                {
                    let mut w = writer.lock().map_err(|_| ())?;
                    // A line that starts with a dot gets a second one.
                    let body = if text.starts_with('.') {
                        format!(".{text}")
                    } else {
                        text
                    };
                    send(&mut *w, &body).map_err(|_| ())?;
                    send(&mut *w, ".").map_err(|_| ())?;
                }
                // 702 END, or 703 CANCELLED by a stop.
                wait_for(&mut *reader, |code| code == 702 || code == 703)
            }
            Kind::Program { program, running } => {
                // On its input, so words that start with `-` are not
                // taken for options.
                let mut child = Command::new(program)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|_| ())?;
                if let Some(mut input) = child.stdin.take() {
                    input.write_all(text.as_bytes()).map_err(|_| ())?;
                }
                *running.lock().map_err(|_| ())? = Some(child);
                loop {
                    let mut guard = running.lock().map_err(|_| ())?;
                    let Some(child) = guard.as_mut() else {
                        // Stopped.
                        return Ok(());
                    };
                    if child.try_wait().map_err(|_| ())?.is_some() {
                        *guard = None;
                        return Ok(());
                    }
                    drop(guard);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
            Kind::PowerShell { child, answers } => {
                {
                    let mut child = child.lock().map_err(|_| ())?;
                    if child.is_none() {
                        let mut command = Command::new("powershell");
                        // No console window over the app's.
                        #[cfg(windows)]
                        std::os::windows::process::CommandExt::creation_flags(
                            &mut command,
                            CREATE_NO_WINDOW,
                        );
                        let mut started = command
                            .args(["-NoProfile", "-NonInteractive", "-Command", POWERSHELL])
                            .stdin(Stdio::piped())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::null())
                            .spawn()
                            .map_err(|_| ())?;
                        let out = started.stdout.take().ok_or(())?;
                        *answers.lock().map_err(|_| ())? = Some(BufReader::new(out));
                        *child = Some(started);
                    }
                    let stdin = child.as_mut().and_then(|c| c.stdin.as_mut()).ok_or(())?;
                    writeln!(stdin, "{text}").map_err(|_| ())?;
                    stdin.flush().map_err(|_| ())?;
                }
                // The answer comes once it is spoken, or the read ends
                // when a stop ends the process.
                let mut answers = answers.lock().map_err(|_| ())?;
                let mut line = String::new();
                let spoken = answers
                    .as_mut()
                    .is_some_and(|a| a.read_line(&mut line).is_ok_and(|n| n > 0));
                if !spoken {
                    *answers = None;
                    if let Ok(mut child) = child.lock()
                        && let Some(mut gone) = child.take()
                    {
                        let _ = gone.kill();
                        let _ = gone.wait();
                    }
                }
                Ok(())
            }
        }
    }

    /// Stops what is being said now.
    pub fn stop(&self) {
        match &self.kind {
            #[cfg(unix)]
            Kind::Ssip { writer, .. } => {
                if let Ok(mut w) = writer.lock() {
                    let _ = send(&mut *w, "CANCEL self");
                }
            }
            Kind::Program { running, .. } => {
                if let Ok(mut guard) = running.lock()
                    && let Some(mut child) = guard.take()
                {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            Kind::PowerShell { child, .. } => {
                // PowerShell cannot be told to stop mid-sentence: it is
                // ended, and a new one started for the next.
                if let Ok(mut child) = child.lock()
                    && let Some(mut child) = child.take()
                {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

/// Speech Dispatcher's socket, connected, when it runs (or starts when
/// asked: it is socket-activated on most desktops).
#[cfg(unix)]
fn ssip() -> Option<Voice> {
    use std::os::unix::net::UnixStream;
    let path = match std::env::var_os("SPEECHD_ADDRESS") {
        Some(address) => {
            let address = address.to_string_lossy().into_owned();
            std::path::PathBuf::from(address.strip_prefix("unix_socket:")?)
        }
        None => {
            let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
            std::path::PathBuf::from(runtime).join("speech-dispatcher/speechd.sock")
        }
    };
    let stream = UnixStream::connect(&path).ok().or_else(|| {
        // Not running: ask it to start, as `spd-say` does.
        Command::new("speech-dispatcher")
            .arg("--spawn")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        UnixStream::connect(&path).ok()
    })?;
    connect_ssip(stream)
}

/// Says who we are, and asks to be told when each message ends.
#[cfg(unix)]
fn connect_ssip(stream: std::os::unix::net::UnixStream) -> Option<Voice> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(120)))
        .ok()?;
    let mut writer = stream.try_clone().ok()?;
    let mut reader = BufReader::new(stream);
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    for line in [
        format!("SET self CLIENT_NAME {user}:katna-pdf:read-aloud"),
        "SET self NOTIFICATION end on".into(),
        "SET self NOTIFICATION cancel on".into(),
    ] {
        send(&mut writer, &line).ok()?;
        wait_for(&mut reader, |code| (200..300).contains(&code)).ok()?;
    }
    Some(Voice {
        kind: Kind::Ssip {
            reader: Mutex::new(reader),
            writer: Mutex::new(writer),
        },
    })
}

#[cfg(unix)]
fn send(w: &mut impl Write, line: &str) -> std::io::Result<()> {
    write!(w, "{line}\r\n")?;
    w.flush()
}

/// Reads replies until one whose last line has a code `done` accepts;
/// `Err` on an error reply (4xx, 5xx) or a closed connection.
#[cfg(unix)]
fn wait_for(reader: &mut impl BufRead, done: impl Fn(u16) -> bool) -> Result<(), ()> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|_| ())? == 0 {
            return Err(());
        }
        // `NNN-…` continues a reply; `NNN …` ends it.
        let (Some(code), Some(sep)) = (line.get(..3), line.as_bytes().get(3)) else {
            continue;
        };
        let Ok(code) = code.parse::<u16>() else {
            continue;
        };
        if *sep != b' ' {
            continue;
        }
        if done(code) {
            return Ok(());
        }
        if (300..600).contains(&code) {
            return Err(());
        }
    }
}

/// `text` cut into sentences, as ranges of its `char`s: at a full stop,
/// question or exclamation mark (or a danda) followed by a space, and
/// at a blank line. Long runs without one are cut at a line's end.
pub fn sentences(text: &str) -> Vec<std::ops::Range<usize>> {
    const LONGEST: usize = 400;
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let ends = matches!(c, '.' | '?' | '!' | '।' | '॥') && next.is_none_or(char::is_whitespace)
            || c == '\n' && (next == Some('\n') || i + 1 - start > LONGEST);
        if ends {
            out.push(start..i + 1);
            start = i + 1;
        }
        i += 1;
    }
    if start < chars.len() {
        out.push(start..chars.len());
    }
    // Only spaces: nothing to say.
    out.retain(|r| chars[r.clone()].iter().any(|c| !c.is_whitespace()));
    out
}

/// A sentence as it is spoken: a word broken over a line by a hyphen
/// joined again, line breaks made spaces.
pub fn spoken(text: &str, range: std::ops::Range<usize>) -> String {
    let s: String = text.chars().skip(range.start).take(range.len()).collect();
    s.replace("-\n", "").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_into_sentences() {
        let text = "One. Two? Three!\nA hyphen-\nated word.\n\nNo stop at the end";
        let found: Vec<String> = sentences(text)
            .into_iter()
            .map(|r| spoken(text, r).trim().to_owned())
            .collect();
        assert_eq!(
            found,
            [
                "One.",
                "Two?",
                "Three!",
                "A hyphenated word.",
                "No stop at the end"
            ]
        );
        // A number's point is not a sentence's end.
        assert_eq!(sentences("Pi is 3.14 here.").len(), 1);
        assert_eq!(sentences("यह वाक्य है। दूसरा।").len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn speaks_to_speech_dispatcher() {
        use std::os::unix::net::UnixStream;
        let (ours, theirs) = UnixStream::pair().unwrap();
        // A pretend Speech Dispatcher that answers as the real one does.
        let server = std::thread::spawn(move || {
            let mut w = theirs.try_clone().unwrap();
            let mut r = BufReader::new(theirs);
            let mut heard = Vec::new();
            let mut line = String::new();
            while r.read_line(&mut line).unwrap() > 0 {
                let l = line.trim_end().to_owned();
                line.clear();
                heard.push(l.clone());
                let reply: &str = match l.as_str() {
                    "SPEAK" => "230 OK RECEIVING DATA\r\n",
                    "." => "225-1\r\n225 OK MESSAGE QUEUED\r\n702-1\r\n702-1\r\n702 END\r\n",
                    l if l.starts_with("SET") => "208 OK\r\n",
                    "QUIT" => break,
                    _ => "",
                };
                w.write_all(reply.as_bytes()).unwrap();
            }
            heard
        });
        let voice = connect_ssip(ours).unwrap();
        voice.say("Hello   there.\n").unwrap();
        voice.say(".dot first").unwrap();
        if let Kind::Ssip { writer, .. } = &voice.kind {
            send(&mut *writer.lock().unwrap(), "QUIT").unwrap();
        }
        let heard = server.join().unwrap();
        assert_eq!(
            &heard[3..],
            [
                "SPEAK",
                "Hello there.",
                ".",
                "SPEAK",
                "..dot first",
                ".",
                "QUIT"
            ]
        );
    }
}
