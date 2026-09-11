//! A single-client remote framebuffer server, driven by presented frames.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::io::{self, ErrorKind, Read, Write};
use std::mem::replace;
use std::net::{TcpListener, TcpStream};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use atria_software_output::{Frame, FrameReport, FrameSink, SinkError};

use crate::input::{Input, InputQueue};
use crate::protocol::{
    PixelFormat, SECURITY_NONE, VERSION, changed_regions, client_message, client_message_length,
    encoding, pixel_format, read_exact, usage_of_keysym, write_update,
};

/// Why the framebuffer could not be served.
#[derive(Debug)]
pub enum VncError {
    /// The client asked for a protocol version this server does not speak.
    UnsupportedVersion([u8; 12]),
    /// The client chose a security type that was not offered.
    UnsupportedSecurity(u8),
    /// The network refused.
    Io(io::Error),
}

impl From<io::Error> for VncError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl fmt::Display for VncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => write!(
                formatter,
                "the viewer asked for {}, and this server speaks RFB 3.8",
                String::from_utf8_lossy(version).trim_end()
            ),
            Self::UnsupportedSecurity(chosen) => {
                write!(formatter, "the viewer chose security type {chosen}")
            }
            Self::Io(error) => write!(formatter, "the connection failed: {error}"),
        }
    }
}

impl Error for VncError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// The most recently composed frame, and a count of how many have been composed.
///
/// One frame is kept, not a queue. A remote viewer wants the current state of the output, so a
/// frame that has been superseded has no value: showing it would be showing something that is no
/// longer true. The serial is what lets a viewer tell "nothing new yet" from "the same pixels
/// again".
/// The most recently composed frame, and a count of how many have been composed.
///
/// One frame is kept, not a queue. A remote viewer wants the current state of the output, so a
/// frame that has been superseded has no value: showing it would be showing something that is no
/// longer true. The serial is what lets a viewer tell "nothing new yet" from "the same pixels
/// again".
#[derive(Default)]
struct Latest {
    /// Shared, not copied: a viewer holds these bytes while the next frame is composed.
    frame: Mutex<(u64, Arc<Vec<u8>>)>,
    composed: Condvar,
    input: Mutex<InputQueue>,
    /// Keysyms this server could not place, so each is reported once rather than every press.
    unplaced: Mutex<BTreeSet<u32>>,
}

/// A sink that shows the composed output to one connected viewer.
///
/// The viewer runs on its own thread, so a slow one falls behind rather than holding the
/// compositor at the write.
pub struct VncSink {
    latest: Arc<Latest>,
    frames_composed: u64,
    /// Frame allocations no viewer holds, reused rather than faulted in again.
    spare: Vec<Vec<u8>>,
    /// Frames handed out and not yet let go of. A viewer holds the last frame it drew, so the
    /// one just replaced becomes reusable a frame later.
    retiring: Vec<Arc<Vec<u8>>>,
    /// Readable when the viewer has done something, so a server waiting on descriptors wakes.
    waker: UnixStream,
}

/// The viewer side: everything that touches the network, on its own thread.
struct Viewer {
    listener: TcpListener,
    latest: Arc<Latest>,
    size: (u16, u16),
    name: String,
    waker: UnixStream,
}

impl VncSink {
    /// Listen on `address` and serve viewers from a thread of its own.
    ///
    /// Returns as soon as the address is bound, without waiting for anyone to connect.
    ///
    /// # Errors
    ///
    /// Returns [`VncError::Io`] when the address cannot be bound.
    pub fn bind(address: &str, width: u16, height: u16, name: &str) -> Result<Self, VncError> {
        let listener = TcpListener::bind(address)?;
        let latest = Arc::new(Latest::default());
        let (waker, signal) = UnixStream::pair()?;
        waker.set_nonblocking(true)?;
        signal.set_nonblocking(true)?;
        let viewer = Viewer {
            listener,
            latest: Arc::clone(&latest),
            size: (width, height),
            name: String::from(name),
            waker: signal,
        };
        thread::Builder::new()
            .name(String::from("atria-vnc"))
            .spawn(move || viewer.serve())?;
        Ok(Self {
            latest,
            frames_composed: 0,
            spare: Vec::new(),
            retiring: Vec::new(),
            waker,
        })
    }

    /// How many frames this sink has been given.
    #[must_use]
    pub const fn frames_composed(&self) -> u64 {
        self.frames_composed
    }

    /// Readable when a viewer has done something, to be waited on alongside a server's sockets.
    #[must_use]
    pub fn wakeup(&self) -> BorrowedFd<'_> {
        self.waker.as_fd()
    }

    /// Take what the viewer has done since this was last called.
    /// Whether what the compositor believes is held should be let go of.
    #[must_use]
    pub fn stale_input(&self) -> bool {
        self.latest
            .input
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_stale()
    }

    /// Whether a transition was lost. The answer is a new routing epoch, not a guess.
    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.latest
            .input
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .overflowed()
    }

    /// Take what the viewer has done since this was last called.
    #[must_use]
    pub fn take_input(&self) -> Vec<Input> {
        // Drain whatever woke us, so the descriptor stops being readable until the next report.
        let mut discard = [0_u8; 64];
        while (&self.waker).read(&mut discard).is_ok_and(|read| read > 0) {}

        self.latest
            .input
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

impl Viewer {
    /// Accept one viewer at a time, forever. A viewer that fails is dropped and the next is
    /// accepted; nothing about a broken connection reaches the compositor.
    fn serve(self) {
        // Non-blocking: one viewer at a time, and the newest wins. A reconnecting viewer would
        // otherwise wait behind its own dead socket.
        if self.listener.set_nonblocking(true).is_err() {
            return;
        }
        loop {
            let Some(mut stream) = self.accept() else {
                thread::sleep(VIEWER_TICK);
                continue;
            };
            // A viewer connecting after the last composition still sees it.
            let mut shown = 0;
            let mut asked = false;
            // A full request must be answered whole, whatever the diff says. A viewer asks after
            // a resize or an expose, and a diff leaves it showing nothing until something moves.
            let mut asked_whole = false;
            let mut format = PixelFormat::declared();
            // What this viewer was last sent. Per viewer: a rectangle list is only correct
            // against the frame it was computed from.
            let mut sent: Option<Arc<Vec<u8>>> = None;
            // Raw until the viewer asks for something else.
            let mut hextile = false;
            loop {
                // A newer viewer replaces this one. Nothing is shared between them, so the new
                // one starts from a whole frame.
                if let Some(replacement) = self.accept() {
                    stream = replacement;
                    shown = 0;
                    asked = false;
                    asked_whole = false;
                    format = PixelFormat::declared();
                    sent = None;
                    hextile = false;
                }
                // Read first: input arrives here, and so does a departure.
                if let Err(error) = self.drain_requests(
                    &mut stream,
                    &mut asked,
                    &mut asked_whole,
                    &mut format,
                    &mut hextile,
                ) {
                    report(&error);
                    break;
                }
                // Only when asked: pushing at a client that is not reading blocks this thread.
                if asked && let Some(frame) = self.frame_after(shown) {
                    shown = frame.0;
                    let previous = if asked_whole {
                        None
                    } else {
                        sent.as_deref().map(Vec::as_slice)
                    };
                    let regions = changed_regions(previous, &frame.1, self.size.0, self.size.1);
                    // Nothing changed. The request stays outstanding rather than being answered
                    // empty, so the next frame that does change something reaches it unasked.
                    if !regions.is_empty() {
                        asked = false;
                        asked_whole = false;
                        if let Err(error) = write_update(
                            &mut stream,
                            &regions,
                            &frame.1,
                            self.size.0,
                            format,
                            hextile,
                        ) {
                            report(&error);
                            break;
                        }
                    }
                    sent = Some(frame.1);
                }
                // Waits unless there is something to send, or a quiet viewer burns a core.
                self.wait_briefly(if asked { shown } else { u64::MAX });
            }
        }
    }
}

/// The most cut text this server will read from a viewer before dropping it.
const MAX_CUT_TEXT: usize = 1 << 20;

/// How many distinct unplaced keysyms are named before the rest are passed over in silence.
const MAX_REPORTED_KEYSYMS: usize = 64;

/// How long the viewer sleeps before reading its connection again. This bounds how long a pointer
/// report waits before the compositor is told, so it is short.
const VIEWER_TICK: Duration = Duration::from_millis(2);

/// A viewer holds at most the frame it last drew, so two is enough and the pool cannot grow.
const SPARE_FRAMES: usize = 2;

impl Viewer {
    /// Take a waiting viewer, if one is there. Never waits.
    fn accept(&self) -> Option<TcpStream> {
        let (mut stream, _) = self.listener.accept().ok()?;
        // One write with nothing to coalesce it with; Nagle would cost 40ms a frame.
        let _unused = stream.set_nodelay(true);
        match self.handshake(&mut stream) {
            Ok(()) => {
                // A new viewer holds nothing, whatever the last one left behind.
                self.queue(InputQueue::forget_held);
                Some(stream)
            }
            Err(error) => {
                eprintln!("atria-vnc: a viewer could not be served: {error}");
                None
            }
        }
    }

    /// The composed frame, if one newer than `shown` exists. Never waits.
    fn frame_after(&self, shown: u64) -> Option<(u64, Arc<Vec<u8>>)> {
        let held = self
            .latest
            .frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if held.0 == shown || held.1.is_empty() {
            return None;
        }
        Some((held.0, Arc::clone(&held.1)))
    }

    /// Sleep until something is composed, or briefly, whichever comes first.
    ///
    /// `shown` is the frame already sent, or `u64::MAX` when there is nothing to send.
    fn wait_briefly(&self, shown: u64) {
        let held = self
            .latest
            .frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if held.0 != shown && shown != u64::MAX {
            return;
        }
        let _unused = self
            .latest
            .composed
            .wait_timeout(held, VIEWER_TICK)
            .unwrap_or_else(PoisonError::into_inner);
    }

    /// RFB 3.8: version, security, then the server's description of the framebuffer.
    fn handshake(&self, stream: &mut TcpStream) -> Result<(), VncError> {
        stream.write_all(VERSION)?;
        let mut theirs = [0_u8; 12];
        read_exact(stream, &mut theirs)?;
        if !theirs.starts_with(b"RFB 003.") {
            return Err(VncError::UnsupportedVersion(theirs));
        }

        stream.write_all(&[1, SECURITY_NONE])?;
        let mut chosen = [0_u8; 1];
        read_exact(stream, &mut chosen)?;
        if chosen[0] != SECURITY_NONE {
            return Err(VncError::UnsupportedSecurity(chosen[0]));
        }
        // SecurityResult: zero is success.
        stream.write_all(&0_u32.to_be_bytes())?;

        // ClientInit carries a shared flag this server ignores: it serves one viewer, so there is
        // nothing to share or to displace.
        let mut shared = [0_u8; 1];
        read_exact(stream, &mut shared)?;

        let mut init = Vec::with_capacity(24 + self.name.len());
        init.extend_from_slice(&self.size.0.to_be_bytes());
        init.extend_from_slice(&self.size.1.to_be_bytes());
        let mut format = [0_u8; 16];
        pixel_format(&mut format);
        init.extend_from_slice(&format);
        init.extend_from_slice(&(self.name.len() as u32).to_be_bytes());
        init.extend_from_slice(self.name.as_bytes());
        stream.write_all(&init)?;
        stream.flush()?;
        Ok(())
    }

    /// Consume whatever the viewer has sent, so the stream stays aligned.
    ///
    /// Every message is read whole even when it changes nothing. Skipping one by its number
    /// without consuming its body leaves the next read starting mid-message, and everything after
    /// that is misread — a failure that looks like a corrupt frame rather than a parsing bug.
    fn drain_requests(
        &self,
        stream: &mut TcpStream,
        asked: &mut bool,
        asked_whole: &mut bool,
        format: &mut PixelFormat,
        hextile: &mut bool,
    ) -> io::Result<()> {
        stream.set_nonblocking(true)?;
        let outcome = self.drain(stream, asked, asked_whole, format, hextile);
        stream.set_nonblocking(false)?;
        outcome
    }

    /// Hand a `PointerEvent` body to the queue.
    fn record_pointer(&self, body: &[u8]) {
        self.queue(|queue| queue.pointer(body));
    }

    /// Hand a `KeyEvent` to the queue, if the key is one this server can place.
    fn record_key(&self, body: &[u8]) {
        let pressed = body[0] != 0;
        let keysym = u32::from_be_bytes([body[3], body[4], body[5], body[6]]);
        let Some(usage) = usage_of_keysym(keysym) else {
            self.report_unplaced(keysym);
            return;
        };
        self.queue(|queue| queue.key(usage, pressed));
    }

    /// Name a keysym this server has no position for, once. A key that does nothing silently is
    /// indistinguishable from one the viewer never sent.
    fn report_unplaced(&self, keysym: u32) {
        let mut seen = self
            .latest
            .unplaced
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if seen.len() >= MAX_REPORTED_KEYSYMS || !seen.insert(keysym) {
            return;
        }
        eprintln!("atria-vnc: the viewer sent keysym {keysym:#06x}, which has no known position");
    }

    fn queue(&self, act: impl FnOnce(&mut InputQueue)) {
        let mut queue = self
            .latest
            .input
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        act(&mut queue);
        drop(queue);
        let _unused = (&self.waker).write(&[1]);
    }

    fn drain(
        &self,
        stream: &mut TcpStream,
        asked: &mut bool,
        asked_whole: &mut bool,
        format: &mut PixelFormat,
        hextile: &mut bool,
    ) -> io::Result<()> {
        loop {
            let mut number = [0_u8; 1];
            match stream.read(&mut number) {
                Ok(0) => return Err(io::Error::from(ErrorKind::UnexpectedEof)),
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }

            // The rest of a message must be read with the socket blocking: it has already begun
            // arriving, and treating a partial message as "nothing to read" would lose it.
            stream.set_nonblocking(false)?;
            if number[0] == client_message::FRAMEBUFFER_UPDATE_REQUEST {
                *asked = true;
            }
            let result = self.consume_body(stream, number[0], asked_whole, format, hextile);
            stream.set_nonblocking(true)?;
            result?;
        }
    }

    fn consume_body(
        &self,
        stream: &mut TcpStream,
        number: u8,
        asked_whole: &mut bool,
        format: &mut PixelFormat,
        hextile: &mut bool,
    ) -> io::Result<()> {
        if let Some(length) = client_message_length(number) {
            let mut body = vec![0_u8; length];
            stream.read_exact(&mut body)?;
            if number == client_message::POINTER_EVENT {
                self.record_pointer(&body);
            }
            if number == client_message::KEY_EVENT {
                self.record_key(&body);
            }
            if number == client_message::FRAMEBUFFER_UPDATE_REQUEST && body[0] == 0 {
                *asked_whole = true;
                // A viewer asks for the whole frame when it is exposed or has just regained
                // focus. Whatever it was holding when it lost focus, it is not holding now.
                self.queue(InputQueue::forget_held);
            }
            if number == client_message::SET_PIXEL_FORMAT {
                // Three bytes of padding, then the sixteen the format occupies.
                let mut declared = [0_u8; 16];
                declared.copy_from_slice(&body[3..19]);
                *format = PixelFormat::decode(&declared)?;
            }
            return Ok(());
        }
        match number {
            client_message::SET_ENCODINGS => {
                let mut head = [0_u8; 3];
                stream.read_exact(&mut head)?;
                let count = u16::from_be_bytes([head[1], head[2]]) as usize;
                let mut body = vec![0_u8; count * 4];
                stream.read_exact(&mut body)?;
                // Read rather than discarded: which encodings a viewer understands is the
                // difference between sending a window and sending four bytes per square of it.
                *hextile = body.chunks_exact(4).any(|listed| {
                    i32::from_be_bytes([listed[0], listed[1], listed[2], listed[3]])
                        == encoding::HEXTILE
                });
                Ok(())
            }
            client_message::CLIENT_CUT_TEXT => {
                let mut head = [0_u8; 7];
                stream.read_exact(&mut head)?;
                let length = u32::from_be_bytes([head[3], head[4], head[5], head[6]]) as usize;
                // Discarded in fixed-size pieces rather than allocated. The length is a
                // four-byte number from an unauthenticated socket: believing it would let one
                // eight-byte message ask this server for four gigabytes.
                discard(stream, length)
            }
            // An unknown message number cannot be skipped, because its length is unknown, so the
            // connection is the only thing that can be resynchronised. Named rather than silent:
            // a viewer dropped for speaking something this server does not know looks exactly
            // like a viewer showing a black screen for no reason.
            _ => Err(io::Error::new(
                ErrorKind::InvalidData,
                alloc_message(number),
            )),
        }
    }
}

/// Read and throw away `length` bytes without allocating for them. Past [`MAX_CUT_TEXT`] the
/// connection is dropped instead.
fn discard(stream: &mut TcpStream, length: usize) -> io::Result<()> {
    if length > MAX_CUT_TEXT {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "the viewer offered more cut text than this server will read",
        ));
    }
    let mut piece = [0_u8; 4096];
    let mut left = length;
    while left > 0 {
        let want = left.min(piece.len());
        stream.read_exact(&mut piece[..want])?;
        left -= want;
    }
    Ok(())
}

/// Say why a viewer was dropped. Losing one silently is indistinguishable from a blank screen.
fn report(error: &io::Error) {
    if error.kind() == ErrorKind::UnexpectedEof || error.kind() == ErrorKind::ConnectionReset {
        return;
    }
    eprintln!("atria-vnc: the viewer was dropped: {error}");
}

/// Name an unrecognised client message, so a log says which one arrived.
fn alloc_message(number: u8) -> String {
    format!("the viewer sent message type {number}, which this server does not speak")
}

impl FrameSink for VncSink {
    fn present(&mut self, frame: &Frame, _report: FrameReport) -> Result<(), SinkError> {
        self.frames_composed += 1;
        // Copied: the frame belongs to the next composition once this returns.
        let mut bytes = self.spare.pop().unwrap_or_default();
        bytes.clear();
        bytes.extend_from_slice(frame.bytes());

        let mut held = self
            .latest
            .frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        held.0 = self.frames_composed;
        let replaced = replace(&mut held.1, Arc::new(bytes));
        drop(held);
        self.latest.composed.notify_all();

        self.retiring.push(replaced);
        let mut index = self.retiring.len();
        while index > 0 {
            index -= 1;
            if Arc::strong_count(&self.retiring[index]) != 1 {
                continue;
            }
            let frame = self.retiring.swap_remove(index);
            if let Ok(bytes) = Arc::try_unwrap(frame)
                && self.spare.len() < SPARE_FRAMES
            {
                self.spare.push(bytes);
            }
        }
        Ok(())
    }
}
