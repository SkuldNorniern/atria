//! A single-client remote framebuffer server, driven by presented frames.

use std::error::Error;
use std::fmt;
use std::io::{self, ErrorKind, Read, Write};
use std::mem::take;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use atria_software_output::{Frame, FrameReport, FrameSink, SinkError};

use crate::protocol::{
    PixelFormat, SECURITY_NONE, VERSION, changed_regions, client_message, client_message_length,
    encoding, pixel_format, read_exact, write_update,
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
/// What a viewer did with its pointer.
///
/// The viewer is a real input source, not only a window onto the output. Coordinates are the
/// output's, because that is what a remote framebuffer protocol speaks in; turning them into a
/// surface's own coordinates is the compositor's job and not this package's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PointerInput {
    pub x: i32,
    pub y: i32,
    /// One bit per button, as RFB carries them. Bit zero is the primary button.
    pub buttons: u8,
}

#[derive(Default)]
struct Latest {
    frame: Mutex<(u64, Vec<u8>)>,
    composed: Condvar,
    /// What the viewer has done since anyone last looked.
    ///
    /// Bounded: a viewer that moves its pointer faster than the compositor reads loses the
    /// intermediate positions, which is the right thing to lose. A queue that grew without limit
    /// would let a viewer make the compositor's memory its own.
    input: Mutex<Vec<PointerInput>>,
}

/// How many pointer reports are kept between reads.
const MAX_PENDING_INPUT: usize = 256;

/// A sink that shows the composed output to one connected viewer.
///
/// The viewer lives on its own thread and reads from [`Latest`]. Two properties depend on that
/// separation: a viewer can complete its handshake while the compositor is idle, and a viewer too
/// slow to keep up falls behind instead of holding the compositor at the write. An observer that
/// can change what it observes is not an observer.
pub struct VncSink {
    latest: Arc<Latest>,
    frames_composed: u64,
}

/// The viewer side: everything that touches the network, on its own thread.
struct Viewer {
    listener: TcpListener,
    latest: Arc<Latest>,
    size: (u16, u16),
    name: String,
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
        let viewer = Viewer {
            listener,
            latest: Arc::clone(&latest),
            size: (width, height),
            name: String::from(name),
        };
        thread::Builder::new()
            .name(String::from("atria-vnc"))
            .spawn(move || viewer.serve())?;
        Ok(Self {
            latest,
            frames_composed: 0,
        })
    }

    /// How many frames this sink has been given.
    #[must_use]
    pub const fn frames_composed(&self) -> u64 {
        self.frames_composed
    }

    /// Take what the viewer has done since this was last called.
    #[must_use]
    pub fn take_input(&self) -> Vec<PointerInput> {
        let mut held = self
            .latest
            .input
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        take(&mut *held)
    }
}

impl Viewer {
    /// Accept one viewer at a time, forever. A viewer that fails is dropped and the next is
    /// accepted; nothing about a broken connection reaches the compositor.
    fn serve(self) {
        loop {
            let Ok((mut stream, _)) = self.listener.accept() else {
                return;
            };
            if let Err(error) = self.handshake(&mut stream) {
                eprintln!("atria-vnc: a viewer could not be served: {error}");
                continue;
            }
            // A viewer that connects after the last composition still sees it. The output has a
            // current state whether or not anything changed while somebody was watching.
            let mut shown = 0;
            let mut asked = false;
            let mut format = PixelFormat::declared();
            // What this viewer was last sent, so the next frame can be compared against it. Per
            // viewer rather than per output: two viewers may be at different frames, and a
            // rectangle list is only correct against the frame it was computed from.
            let mut sent: Option<Vec<u8>> = None;
            // Raw until the viewer says otherwise. Every viewer understands raw, and a server
            // that assumed anything else would be unreadable to one that had not asked for it.
            let mut hextile = false;
            loop {
                // Read first, always. This is where a viewer's input arrives and where its
                // departure is noticed, and a server that only read before writing would stop
                // hearing from a viewer that had stopped asking for frames.
                if let Err(error) =
                    self.drain_requests(&mut stream, &mut asked, &mut format, &mut hextile)
                {
                    report(&error);
                    break;
                }
                // Sent only when asked for. RFB puts the client in charge of when a frame
                // arrives, and pushing one at a client that is not reading fills the socket and
                // blocks this thread — which is how a viewer stops being able to send input at
                // all.
                if asked && let Some(frame) = self.frame_after(shown) {
                    shown = frame.0;
                    let regions =
                        changed_regions(sent.as_deref(), &frame.1, self.size.0, self.size.1);
                    // Nothing changed where this viewer could see it. Its request stays
                    // outstanding rather than being answered with an empty update, so the next
                    // frame that does change something reaches it without being asked again.
                    if !regions.is_empty() {
                        asked = false;
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
                self.wait_briefly(shown);
            }
        }
    }
}

/// How long the viewer sleeps before looking at its connection again.
///
/// Short, because this is also how often a viewer's pointer is read. A frame being composed wakes
/// it sooner, so the interval only bounds how long an idle compositor takes to notice input.
const VIEWER_TICK: Duration = Duration::from_millis(8);

impl Viewer {
    /// The composed frame, if one newer than `shown` exists. Never waits.
    fn frame_after(&self, shown: u64) -> Option<(u64, Vec<u8>)> {
        let held = self
            .latest
            .frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if held.0 == shown || held.1.is_empty() {
            return None;
        }
        Some((held.0, held.1.clone()))
    }

    /// Sleep until something is composed, or briefly, whichever comes first.
    ///
    /// Bounded so that a viewer's input and its departure are still noticed while nothing is
    /// being composed. Waiting on the condvar rather than sleeping means a frame wakes this
    /// immediately instead of after the interval.
    fn wait_briefly(&self, shown: u64) {
        let held = self
            .latest
            .frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if held.0 != shown {
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
        format: &mut PixelFormat,
        hextile: &mut bool,
    ) -> io::Result<()> {
        stream.set_nonblocking(true)?;
        let outcome = self.drain(stream, asked, format, hextile);
        stream.set_nonblocking(false)?;
        outcome
    }

    /// Record what a `PointerEvent` said, dropping the oldest when the bound is reached.
    fn record_pointer(&self, body: &[u8]) {
        // Button mask, then x and y, big-endian, as RFB 3.8 defines the message.
        let buttons = body[0];
        let x = i32::from(u16::from_be_bytes([body[1], body[2]]));
        let y = i32::from(u16::from_be_bytes([body[3], body[4]]));
        let mut held = self
            .latest
            .input
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if held.len() >= MAX_PENDING_INPUT {
            held.remove(0);
        }
        held.push(PointerInput { x, y, buttons });
    }

    fn drain(
        &self,
        stream: &mut TcpStream,
        asked: &mut bool,
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
            let result = self.consume_body(stream, number[0], format, hextile);
            stream.set_nonblocking(true)?;
            result?;
        }
    }

    fn consume_body(
        &self,
        stream: &mut TcpStream,
        number: u8,
        format: &mut PixelFormat,
        hextile: &mut bool,
    ) -> io::Result<()> {
        if let Some(length) = client_message_length(number) {
            let mut body = vec![0_u8; length];
            stream.read_exact(&mut body)?;
            if number == client_message::POINTER_EVENT {
                self.record_pointer(&body);
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
                let mut body = vec![0_u8; length];
                stream.read_exact(&mut body)
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
        let mut held = self
            .latest
            .frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Copied rather than borrowed: the frame belongs to the next composition the moment this
        // call returns. The copy is into the buffer already held, so a steady stream of frames
        // does not allocate.
        held.0 = self.frames_composed;
        held.1.clear();
        held.1.extend_from_slice(frame.bytes());
        drop(held);
        self.latest.composed.notify_all();
        Ok(())
    }
}
