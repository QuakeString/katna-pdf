// SPDX-License-Identifier: GPL-3.0-or-later

//! Pages drawn in worker processes (docs/PLAN.md §2.1, Phase 2), so a
//! file that crashes or hangs the engine takes down a worker, not the
//! app: the page is left undrawn and the worker is started again.
//!
//! [`Workers`] is an [`Engine`]. Its documents answer sizes and
//! transforms themselves, from a copy opened in the app, and send
//! drawing, pictures and text to a small pool of workers: the app's own
//! program started with an argument that makes it [`serve`] instead.
//! Each worker opens a document the first time it is asked to draw from
//! it and closes it when the document is dropped. Each message is a
//! head and a blob (a file opened, a page drawn), each after its length,
//! in little-endian, so bitmaps go across without being copied again;
//! the format is private to one build of the app, which talks only to
//! itself.

use std::collections::HashSet;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::text::PageText;
use crate::{Bitmap, Document, Engine, Hayro, OpenError, Region, recolor};

/// A request taking longer than this is taken for a hang: the worker is
/// stopped and the page left undrawn.
const TIMEOUT: Duration = Duration::from_secs(60);
/// The largest message read, a little over the largest bitmap
/// ([`crate::MAX_PIXELS`] of RGBA) or a big file.
const MAX_MESSAGE: u64 = 1 << 31;

/// Draws in a pool of worker processes.
pub struct Workers {
    pool: Arc<Pool>,
}

impl Workers {
    /// A pool of at most `count` workers, each started as the program
    /// `program` gives at the time, with `args`. They start when first
    /// needed.
    pub fn new(
        program: impl Fn() -> PathBuf + Send + Sync + 'static,
        args: Vec<String>,
        count: usize,
    ) -> Self {
        Self {
            pool: Arc::new(Pool {
                program: Box::new(program),
                args,
                slots: (0..count.max(1)).map(|_| Mutex::new(None)).collect(),
                closed: Mutex::new(HashSet::new()),
                broken: OnceLock::new(),
            }),
        }
    }
}

impl Engine for Workers {
    fn name(&self) -> &'static str {
        "hayro-workers"
    }

    fn open(&self, bytes: Arc<Vec<u8>>, password: &str) -> Result<Arc<dyn Document>, OpenError> {
        let local = Hayro.open(bytes.clone(), password)?;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Ok(Arc::new(RemoteDocument {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            bytes,
            password: password.to_owned(),
            local,
            pool: self.pool.clone(),
        }))
    }
}

struct RemoteDocument {
    id: u64,
    bytes: Arc<Vec<u8>>,
    password: String,
    /// The document opened in the app, for its sizes and transforms, and
    /// for drawing when no worker can be started.
    local: Arc<dyn Document>,
    pool: Arc<Pool>,
}

impl RemoteDocument {
    /// `request`'s answer from a worker, after its first byte (`OK`);
    /// `None` when the worker failed. `Err` when no worker can be started
    /// at all.
    fn ask(&self, page: usize, request: &[u8]) -> Result<Option<Frame>, ()> {
        self.pool.ask(self, page, request)
    }
}

impl Drop for RemoteDocument {
    fn drop(&mut self) {
        self.pool.close(self.id);
    }
}

impl Document for RemoteDocument {
    fn page_count(&self) -> usize {
        self.local.page_count()
    }

    fn page_size(&self, page: usize) -> Option<(f32, f32)> {
        self.local.page_size(page)
    }

    fn render(&self, page: usize, scale: f32) -> Option<Bitmap> {
        let mut m = Message::new(RENDER);
        m.u64(self.id).u32(page as u32).f32(scale);
        match self.ask(page, &m.0) {
            Ok(answer) => bitmap(answer?),
            Err(()) => self.local.render(page, scale),
        }
    }

    fn render_region(&self, page: usize, scale: f32, region: Region) -> Option<Bitmap> {
        let mut m = Message::new(REGION);
        m.u64(self.id)
            .u32(page as u32)
            .f32(scale)
            .u32(region.x)
            .u32(region.y)
            .u32(region.width)
            .u32(region.height);
        match self.ask(page, &m.0) {
            Ok(answer) => bitmap(answer?),
            Err(()) => self.local.render_region(page, scale, region),
        }
    }

    fn pictures(&self, page: usize, scale: f32) -> Vec<recolor::Area> {
        let mut m = Message::new(PICTURES);
        m.u64(self.id).u32(page as u32).f32(scale);
        match self.ask(page, &m.0) {
            Ok(answer) => answer
                .and_then(|(head, _)| areas(&head))
                .unwrap_or_default(),
            Err(()) => self.local.pictures(page, scale),
        }
    }

    fn text(&self, page: usize) -> PageText {
        let mut m = Message::new(TEXT);
        m.u64(self.id).u32(page as u32);
        match self.ask(page, &m.0) {
            Ok(answer) => answer.and_then(|(head, _)| text(&head)).unwrap_or_default(),
            Err(()) => self.local.text(page),
        }
    }

    fn segments(&self, page: usize) -> Vec<crate::measure::Segment> {
        let mut m = Message::new(SEGMENTS);
        m.u64(self.id).u32(page as u32);
        match self.ask(page, &m.0) {
            Ok(answer) => answer
                .map(|(_, blob)| {
                    blob.as_chunks::<16>()
                        .0
                        .iter()
                        .map(|c| {
                            let f =
                                |i: usize| f32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
                            [f(0), f(4), f(8), f(12)]
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Err(()) => self.local.segments(page),
        }
    }

    fn page_transform(&self, page: usize) -> Option<[f64; 6]> {
        self.local.page_transform(page)
    }
}

// Requests.
const OPEN: u8 = 1;
const CLOSE: u8 = 2;
const RENDER: u8 = 3;
const REGION: u8 = 4;
const PICTURES: u8 = 5;
const TEXT: u8 = 6;
const SEGMENTS: u8 = 7;
// Answers.
const FAILED: u8 = 0;
const OK: u8 = 1;
const OPEN_FAILED: u8 = 2;

/// A message: its head, and its blob (often empty).
type Frame = (Vec<u8>, Vec<u8>);

struct Pool {
    program: Box<dyn Fn() -> PathBuf + Send + Sync>,
    args: Vec<String>,
    /// The workers, started when first needed. A page always goes to the
    /// same one, which keeps what it has read of the page (a drawing's
    /// scene) for the next draw: zooming never reads a page again.
    slots: Vec<Mutex<Option<Worker>>>,
    /// Documents dropped that a worker may still hold.
    closed: Mutex<HashSet<u64>>,
    /// Set when a worker could not be started: the app draws itself from
    /// then on.
    broken: OnceLock<()>,
}

impl Pool {
    fn ask(&self, doc: &RemoteDocument, page: usize, request: &[u8]) -> Result<Option<Frame>, ()> {
        if self.broken.get().is_some() {
            return Err(());
        }
        let slot = (doc.id as usize).wrapping_mul(7).wrapping_add(page) % self.slots.len();
        let mut slot = self.slots[slot].lock().unwrap_or_else(|e| e.into_inner());
        // A worker that crashes or hangs is stopped and the request tried
        // once more on a new one: it may have died of something else (a
        // page before, the system short of memory). Twice, the page is
        // taken to be the cause and left undrawn.
        for _ in 0..2 {
            if slot.is_none() {
                match Worker::start(&(self.program)(), &self.args) {
                    Ok(worker) => *slot = Some(worker),
                    Err(err) => {
                        eprintln!("katna-pdf: drawing in the app; no worker: {err}");
                        self.broken.set(()).ok();
                        return Err(());
                    }
                }
            }
            let worker = slot.as_mut().ok_or(())?;
            if let Some(answer) = self
                .close_dropped(worker)
                .then(|| worker.ask(doc, request))
                .flatten()
            {
                return Ok(answer);
            }
            if let Some(mut worker) = slot.take() {
                worker.stop();
            }
        }
        Ok(None)
    }

    /// Closes the documents dropped that `worker` holds; `false` when it
    /// is gone.
    fn close_dropped(&self, worker: &mut Worker) -> bool {
        let closed: Vec<u64> = {
            let closed = self.closed.lock().unwrap_or_else(|e| e.into_inner());
            worker.open.intersection(&closed).copied().collect()
        };
        closed.into_iter().all(|id| worker.close(id))
    }

    /// Document `id` is dropped: idle workers close it now, busy ones
    /// before their next request.
    fn close(&self, id: u64) {
        self.closed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id);
        for slot in &self.slots {
            let Ok(mut slot) = slot.try_lock() else {
                continue;
            };
            if let Some(worker) = slot.as_mut()
                && worker.open.contains(&id)
                && !worker.close(id)
                && let Some(mut worker) = slot.take()
            {
                worker.stop();
            }
        }
    }
}

/// A worker process, and its answers as they are read.
struct Worker {
    child: Child,
    to: BufWriter<ChildStdin>,
    answers: Receiver<io::Result<Frame>>,
    /// The documents it has open.
    open: HashSet<u64>,
}

impl Worker {
    fn start(program: &PathBuf, args: &[String]) -> io::Result<Self> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let to = BufWriter::new(child.stdin.take().ok_or(io::ErrorKind::BrokenPipe)?);
        let mut from = BufReader::new(child.stdout.take().ok_or(io::ErrorKind::BrokenPipe)?);
        let (send, answers) = mpsc::channel();
        std::thread::Builder::new()
            .name("katna-pdf-worker-reader".into())
            .spawn(move || {
                loop {
                    let answer = read_frame(&mut from);
                    let end = answer.is_err();
                    if send.send(answer).is_err() || end {
                        break;
                    }
                }
            })?;
        Ok(Self {
            child,
            to,
            answers,
            open: HashSet::new(),
        })
    }

    /// `request` for `doc`, opening it first if this worker has not:
    /// `Some` with the answer (`None` when the worker could not do it),
    /// or `None` when the worker crashed or hung.
    fn ask(&mut self, doc: &RemoteDocument, request: &[u8]) -> Option<Option<Frame>> {
        if !self.open.contains(&doc.id) {
            let mut m = Message::new(OPEN);
            m.u64(doc.id).bytes(doc.password.as_bytes());
            let (head, _) = self.round_trip(&m.0, &doc.bytes)?;
            if head.first() != Some(&OK) {
                return Some(None);
            }
            self.open.insert(doc.id);
        }
        let (mut head, blob) = self.round_trip(request, &[])?;
        Some((head.first() == Some(&OK)).then(|| {
            head.remove(0);
            (head, blob)
        }))
    }

    /// Closes document `id`; `false` when the worker is gone.
    fn close(&mut self, id: u64) -> bool {
        let mut m = Message::new(CLOSE);
        m.u64(id);
        self.open.remove(&id);
        self.round_trip(&m.0, &[]).is_some()
    }

    fn round_trip(&mut self, request: &[u8], blob: &[u8]) -> Option<Frame> {
        write_frame(&mut self.to, request, blob).ok()?;
        match self.answers.recv_timeout(TIMEOUT) {
            Ok(Ok(answer)) => Some(answer),
            Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => None,
            Err(RecvTimeoutError::Timeout) => {
                eprintln!("katna-pdf: a worker took too long and was stopped");
                None
            }
        }
    }

    fn stop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Its input closed, a worker ends by itself.
        self.to.flush().ok();
    }
}

/// Runs a worker: answers requests on standard input until it closes.
/// Engine panics are answered as failures; anything worse ends the
/// process, which the app sees.
pub fn serve() -> io::Result<()> {
    let mut from = BufReader::new(io::stdin().lock());
    let mut to = BufWriter::new(stdout()?);
    let mut docs: Vec<(u64, Arc<dyn Document>)> = Vec::new();
    loop {
        let (head, blob) = match read_frame(&mut from) {
            Ok(request) => request,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        let (head, blob) = catch_unwind(AssertUnwindSafe(|| answer(&mut docs, &head, blob)))
            .unwrap_or_else(|_| failed());
        write_frame(&mut to, &head, &blob)?;
    }
}

/// Standard output as a file of its own: unlike [`io::stdout`], not
/// looking for line ends in every bitmap written.
fn stdout() -> io::Result<std::fs::File> {
    #[cfg(unix)]
    let owned = std::os::fd::AsFd::as_fd(&io::stdout()).try_clone_to_owned()?;
    #[cfg(windows)]
    let owned = std::os::windows::io::AsHandle::as_handle(&io::stdout()).try_clone_to_owned()?;
    Ok(owned.into())
}

fn failed() -> Frame {
    (vec![FAILED], Vec::new())
}

fn answer(docs: &mut Vec<(u64, Arc<dyn Document>)>, request: &[u8], blob: Vec<u8>) -> Frame {
    let mut r = Reader(request);
    let (Some(kind), Some(id)) = (r.u8(), r.u64()) else {
        return failed();
    };
    if kind == OPEN {
        let Some(password) = r.bytes() else {
            return failed();
        };
        let password = String::from_utf8_lossy(password);
        return match Hayro.open(Arc::new(blob), &password) {
            Ok(doc) => {
                docs.retain(|(d, _)| *d != id);
                docs.push((id, doc));
                (vec![OK], Vec::new())
            }
            Err(_) => (vec![OPEN_FAILED], Vec::new()),
        };
    }
    if kind == CLOSE {
        docs.retain(|(d, _)| *d != id);
        return (vec![OK], Vec::new());
    }
    let Some(doc) = docs.iter().find(|(d, _)| *d == id).map(|(_, doc)| doc) else {
        return failed();
    };
    let Some(page) = r.u32().map(|p| p as usize) else {
        return failed();
    };
    let mut m = Message::new(OK);
    match kind {
        RENDER | REGION => {
            let Some(scale) = r.f32() else {
                return failed();
            };
            let drawn = if kind == RENDER {
                doc.render(page, scale)
            } else {
                let (Some(x), Some(y), Some(width), Some(height)) =
                    (r.u32(), r.u32(), r.u32(), r.u32())
                else {
                    return failed();
                };
                doc.render_region(
                    page,
                    scale,
                    Region {
                        x,
                        y,
                        width,
                        height,
                    },
                )
            };
            let Some(b) = drawn else {
                return failed();
            };
            m.u32(b.width).u32(b.height);
            return (m.0, b.rgba);
        }
        PICTURES => {
            let Some(scale) = r.f32() else {
                return failed();
            };
            let areas = doc.pictures(page, scale);
            m.u32(areas.len() as u32);
            for (a, b, c, d) in areas {
                m.u32(a).u32(b).u32(c).u32(d);
            }
        }
        TEXT => {
            let text = doc.text(page);
            m.bytes(text.text.as_bytes()).u32(text.boxes.len() as u32);
            for b in text.boxes {
                for v in b {
                    m.f32(v);
                }
            }
        }
        SEGMENTS => {
            // Many, so in the blob, four little-endian floats each.
            let segments = doc.segments(page);
            let mut blob = Vec::with_capacity(segments.len() * 16);
            for s in segments {
                for v in s {
                    blob.extend_from_slice(&v.to_le_bytes());
                }
            }
            return (m.0, blob);
        }
        _ => return failed(),
    }
    (m.0, Vec::new())
}

fn bitmap((head, rgba): Frame) -> Option<Bitmap> {
    let mut r = Reader(&head);
    let (width, height) = (r.u32()?, r.u32()?);
    (rgba.len() as u64 == u64::from(width) * u64::from(height) * 4).then_some(Bitmap {
        width,
        height,
        rgba,
    })
}

fn areas(answer: &[u8]) -> Option<Vec<recolor::Area>> {
    let mut r = Reader(answer);
    let n = r.u32()?;
    (0..n)
        .map(|_| Some((r.u32()?, r.u32()?, r.u32()?, r.u32()?)))
        .collect()
}

fn text(answer: &[u8]) -> Option<PageText> {
    let mut r = Reader(answer);
    let text = String::from_utf8(r.bytes()?.to_vec()).ok()?;
    let n = r.u32()?;
    let boxes = (0..n)
        .map(|_| Some([r.f32()?, r.f32()?, r.f32()?, r.f32()?]))
        .collect::<Option<Vec<_>>>()?;
    Some(PageText { text, boxes })
}

/// A message being written.
struct Message(Vec<u8>);

impl Message {
    fn new(kind: u8) -> Self {
        Self(vec![kind])
    }

    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u64(v.len() as u64);
        self.0.extend_from_slice(v);
        self
    }
}

/// A message being read.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = usize::try_from(self.u64()?).ok()?;
        self.take(n)
    }
}

fn write_frame(to: &mut impl Write, head: &[u8], blob: &[u8]) -> io::Result<()> {
    to.write_all(&(head.len() as u64).to_le_bytes())?;
    to.write_all(head)?;
    to.write_all(&(blob.len() as u64).to_le_bytes())?;
    to.write_all(blob)?;
    to.flush()
}

fn read_frame(from: &mut impl Read) -> io::Result<Frame> {
    Ok((read_part(from)?, read_part(from)?))
}

/// A length, then that many bytes.
fn read_part(from: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0; 8];
    from.read_exact(&mut len)?;
    let len = u64::from_le_bytes(len);
    if len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too long",
        ));
    }
    let mut part = Vec::with_capacity(len as usize);
    from.take(len).read_to_end(&mut part)?;
    if part.len() as u64 != len {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(part)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_read_back_as_written() {
        let mut m = Message::new(OK);
        m.u32(7).u64(1 << 40).f32(1.5).bytes(b"abc");
        let mut framed = Vec::new();
        write_frame(&mut framed, &m.0, b"blob").unwrap();
        let (read, blob) = read_frame(&mut framed.as_slice()).unwrap();
        assert_eq!(blob, b"blob");
        // Cut short, it is not read.
        assert!(read_frame(&mut &framed[..framed.len() - 1]).is_err());
        let mut r = Reader(&read);
        assert_eq!(r.u8(), Some(OK));
        assert_eq!(r.u32(), Some(7));
        assert_eq!(r.u64(), Some(1 << 40));
        assert_eq!(r.f32(), Some(1.5));
        assert_eq!(r.bytes(), Some(&b"abc"[..]));
        assert_eq!(r.u8(), None);
        // A length past the end is not read.
        assert_eq!(Reader(&[9, 0, 0, 0, 0, 0, 0, 0, 1]).bytes(), None);
    }
}
