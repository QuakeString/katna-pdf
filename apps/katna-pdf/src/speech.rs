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
use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

/// How fast to speak, 1.0 the voice's own pace, as Settings > Read
/// aloud sets it (hundredths, so it fits an atomic).
static SPEED: AtomicU32 = AtomicU32::new(100);

/// Sets how fast the voice speaks from the next sentence, from 0.5 to 2.
pub fn set_speed(speed: f32) {
    SPEED.store(
        (speed.clamp(0.5, 2.0) * 100.0).round() as u32,
        Ordering::Relaxed,
    );
}

fn speed() -> f32 {
    SPEED.load(Ordering::Relaxed) as f32 / 100.0
}

/// How high to speak, -100 to 100, 0 the voice's own pitch.
static PITCH: AtomicI32 = AtomicI32::new(0);

/// The voice chosen in Settings > Read aloud, by the id [`Voice::voices`]
/// gives it; empty for the desktop's own.
static CHOSEN: Mutex<String> = Mutex::new(String::new());

/// Sets the voice's pitch, from -1 (low) to 1 (high), and which voice
/// speaks, from the next sentence.
pub fn set_tone(pitch: f32, voice: &str) {
    PITCH.store(
        (pitch.clamp(-1.0, 1.0) * 100.0).round() as i32,
        Ordering::Relaxed,
    );
    if let Ok(mut chosen) = CHOSEN.lock() {
        voice.clone_into(&mut chosen);
    }
}

fn pitch() -> i32 {
    PITCH.load(Ordering::Relaxed)
}

fn chosen() -> String {
    CHOSEN.lock().map(|c| c.clone()).unwrap_or_default()
}

/// What reads aloud, once a reading has opened it: Speech Dispatcher,
/// a program's name, or Windows' voice. `None` before then, as opening it
/// may start Speech Dispatcher and is kept off the UI thread.
pub fn opened_name() -> Option<&'static str> {
    match VOICE.get()? {
        None => Some(""),
        Some(voice) => Some(match &voice.kind {
            #[cfg(unix)]
            Kind::Ssip { .. } => "Speech Dispatcher",
            Kind::Program { program, .. } => program.as_str(),
            Kind::PowerShell { .. } => "Windows",
        }),
    }
}

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
/// A line that starts with \u{1} sets the pace, \u{2} the voice by name
/// and \u{3} the pitch (below 0 low, above high); those are not spoken.
const POWERSHELL: &str = "[Console]::InputEncoding = [Text.Encoding]::UTF8; Add-Type -AssemblyName System.Speech; \
    $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $p = 0; \
    while (($l = [Console]::In.ReadLine()) -ne $null) { \
    if ($l.StartsWith([string][char]1)) { $s.Rate = [int]$l.Substring(1); continue }; \
    if ($l.StartsWith([string][char]2)) { if ($l.Length -gt 1) { try { $s.SelectVoice($l.Substring(1)) } catch {} }; continue }; \
    if ($l.StartsWith([string][char]3)) { $p = [int]$l.Substring(1); continue }; \
    if ($p -eq 0) { $s.Speak($l) } else { \
    $h = if ($p -gt 0) { 'high' } else { 'low' }; \
    $s.SpeakSsml(\"<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='\" + $s.Voice.Culture.Name + \"'><prosody pitch='\" + $h + \"'>\" + [Security.SecurityElement]::Escape($l) + '</prosody></speak>') }; \
    [Console]::Out.WriteLine('done'); [Console]::Out.Flush() }";

/// Writes the names of Windows' voices, a line each.
#[cfg_attr(not(windows), allow(dead_code))]
const POWERSHELL_VOICES: &str = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Add-Type -AssemblyName System.Speech; \
    (New-Object System.Speech.Synthesis.SpeechSynthesizer).GetInstalledVoices() | \
    Where-Object { $_.Enabled } | ForEach-Object { $_.VoiceInfo.Name }";

/// Windows' flag for a program started without a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The app's one voice, opened the first time it is asked for: slow (it
/// may start Speech Dispatcher), so asked for off the UI thread.
pub fn voice() -> Option<&'static Voice> {
    VOICE.get_or_init(Voice::open).as_ref()
}

static VOICE: std::sync::OnceLock<Option<Voice>> = std::sync::OnceLock::new();

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
                let mut reader = reader.lock().map_err(|_| ())?;
                // -100 to 100, 0 the voice's own pace.
                let rate = ((speed() - 1.0) * 100.0).round().clamp(-100.0, 100.0);
                let mut sets = vec![
                    format!("SET self RATE {rate}"),
                    format!("SET self PITCH {}", pitch()),
                ];
                let name = chosen();
                if !name.is_empty() {
                    sets.push(format!("SET self SYNTHESIS_VOICE {name}"));
                }
                for line in sets {
                    {
                        let mut w = writer.lock().map_err(|_| ())?;
                        send(&mut *w, &line).map_err(|_| ())?;
                    }
                    // A voice it doesn't have (one chosen when espeak-ng
                    // read aloud) is turned down, and the sentence still
                    // read in the voice it has.
                    let voice = line.contains("SYNTHESIS_VOICE");
                    wait_for(&mut *reader, |code| {
                        (200..300).contains(&code) || voice && code >= 300
                    })?;
                }
                {
                    let mut w = writer.lock().map_err(|_| ())?;
                    send(&mut *w, "SPEAK").map_err(|_| ())?;
                }
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
                // espeak's own pace is 175 words a minute.
                let words = (175.0 * speed()).round() as u32;
                // 0 to 99, 50 the voice's own pitch.
                let high = (50 + pitch() * 2 / 5).clamp(0, 99);
                let mut command = Command::new(program);
                command.args(["-s", &words.to_string(), "-p", &high.to_string()]);
                let name = chosen();
                if !name.is_empty() {
                    command.args(["-v", &name]);
                }
                let mut child = command
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
                    // -10 to 10, 0 the voice's own pace; a line that
                    // starts with \u{1} sets it and is not spoken.
                    let rate = ((speed() - 1.0) * 10.0).round().clamp(-10.0, 10.0);
                    writeln!(stdin, "\u{1}{rate}").map_err(|_| ())?;
                    writeln!(stdin, "\u{2}{}", chosen()).map_err(|_| ())?;
                    writeln!(stdin, "\u{3}{}", pitch()).map_err(|_| ())?;
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

    /// The voices that can be chosen, as an id to set with
    /// [`set_tone`] and words to show; slow, so asked for off the UI
    /// thread.
    pub fn voices(&self) -> Vec<(String, String)> {
        match &self.kind {
            #[cfg(unix)]
            Kind::Ssip { reader, writer } => {
                let Ok(mut reader) = reader.lock() else {
                    return Vec::new();
                };
                if writer
                    .lock()
                    .map_err(|_| ())
                    .and_then(|mut w| send(&mut *w, "LIST SYNTHESIS_VOICES").map_err(|_| ()))
                    .is_err()
                {
                    return Vec::new();
                }
                ssip_voices(&mut *reader)
            }
            Kind::Program { program, .. } => Command::new(program)
                .arg("--voices")
                .stderr(Stdio::null())
                .output()
                .map(|out| espeak_voices(&String::from_utf8_lossy(&out.stdout)))
                .unwrap_or_default(),
            Kind::PowerShell { .. } => {
                let mut command = Command::new("powershell");
                #[cfg(windows)]
                std::os::windows::process::CommandExt::creation_flags(
                    &mut command,
                    CREATE_NO_WINDOW,
                );
                command
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        POWERSHELL_VOICES,
                    ])
                    .stdin(Stdio::null())
                    .stderr(Stdio::null())
                    .output()
                    .map(|out| {
                        String::from_utf8_lossy(&out.stdout)
                            .lines()
                            .map(str::trim)
                            .filter(|l| !l.is_empty())
                            .map(|l| (l.to_owned(), l.to_owned()))
                            .collect()
                    })
                    .unwrap_or_default()
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

/// Speech Dispatcher's answer to `LIST SYNTHESIS_VOICES`: lines of
/// `249-name<TAB>language<TAB>variant`, then `249 OK VOICE LIST SENT`.
#[cfg(unix)]
fn ssip_voices(reader: &mut impl BufRead) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return out;
        }
        let line = line.trim_end();
        let Some(code) = line.get(..3).and_then(|c| c.parse::<u16>().ok()) else {
            continue;
        };
        match line.as_bytes().get(3) {
            Some(b'-') if code == 249 => {
                let mut parts = line[4..].split('\t');
                let name = parts.next().unwrap_or_default().trim();
                let language = parts.next().unwrap_or_default().trim();
                let variant = parts.next().unwrap_or_default().trim();
                // espeak through Speech Dispatcher lists each language
                // again with each of its many variants: those are left out.
                if !name.is_empty() && matches!(variant, "" | "none") {
                    let words = match language {
                        "" | "none" => name.to_owned(),
                        l => format!("{name} ({l})"),
                    };
                    out.push((name.to_owned(), words));
                }
            }
            Some(b' ') => return out,
            _ => {}
        }
    }
}

/// `espeak-ng --voices`: a heading, then `Pty Language Age/Gender
/// VoiceName File Other`; chosen by language.
#[cfg_attr(windows, allow(dead_code))]
fn espeak_voices(list: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = list
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut cols = l.split_whitespace();
            let language = cols.nth(1)?;
            let name = cols.nth(1)?.replace('_', " ");
            Some((language.to_owned(), format!("{name} ({language})")))
        })
        .collect();
    out.dedup_by(|a, b| a.0 == b.0);
    out
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

    #[test]
    fn voice_lists_are_read() {
        let espeak = "Pty Language       Age/Gender VoiceName          File                 Other Languages\n \
            5  af              --/M      Afrikaans          gmw/af\n \
            2  en-gb           --/M      English_(Great_Britain) gmw/en           (en 2)\n \
            5  hi              --/M      Hindi              inc/hi\n";
        assert_eq!(
            espeak_voices(espeak),
            [
                ("af".to_string(), "Afrikaans (af)".to_string()),
                ("en-gb".into(), "English (Great Britain) (en-gb)".into()),
                ("hi".into(), "Hindi (hi)".into()),
            ]
        );
        #[cfg(unix)]
        {
            let ssip = "249-Alan\ten\tnone\r\n249-Alan+Max\ten\tMax\r\n249-Kal\thi\tnone\r\n249 OK VOICE LIST SENT\r\n";
            let voices = ssip_voices(&mut std::io::Cursor::new(ssip));
            assert_eq!(
                voices,
                [
                    ("Alan".to_string(), "Alan (en)".to_string()),
                    ("Kal".into(), "Kal (hi)".into())
                ]
            );
        }
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
                "SET self RATE 0",
                "SET self PITCH 0",
                "SPEAK",
                "Hello there.",
                ".",
                "SET self RATE 0",
                "SET self PITCH 0",
                "SPEAK",
                "..dot first",
                ".",
                "QUIT"
            ]
        );
    }
}
