//! A single-client remote framebuffer server, driven by presented frames.

use std::error::Error;
use std::fmt;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;

use atria_software_output::{Frame, FrameReport, FrameSink, SinkError};

use crate::protocol::{
    SECURITY_NONE, VERSION, client_message, client_message_length, pixel_format, read_exact,
    write_full_update,
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
#[derive(Default)]
struct Latest {
    frame: Mutex<(u64, Vec<u8>)>,
    composed: Condvar,
}

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
}

impl Viewer {
    /// Accept one viewer at a time, forever. A viewer that fails is dropped and the next is
    /// accepted; nothing about a broken connection reaches the compositor.
    fn serve(self) {
        loop {
            let Ok((mut stream, _)) = self.listener.accept() else {
                return;
            };
            if self.handshake(&mut stream).is_err() {
                continue;
            }
            // A viewer that connects after the last composition still sees it. The output has a
            // current state whether or not anything changed while somebody was watching.
            let mut shown = 0;
            loop {
                let frame = self.wait_for_frame(shown);
                shown = frame.0;
                let written = Self::drain_requests(&mut stream).and_then(|()| {
                    write_full_update(&mut stream, self.size.0, self.size.1, &frame.1)
                });
                if written.is_err() {
                    break;
                }
            }
        }
    }

    /// Block until a frame newer than `shown` exists, then take a copy of it.
    fn wait_for_frame(&self, shown: u64) -> (u64, Vec<u8>) {
        let mut held = self
            .latest
            .frame
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while held.0 == shown || held.1.is_empty() {
            held = self
                .latest
                .composed
                .wait(held)
                .unwrap_or_else(PoisonError::into_inner);
        }
        (held.0, held.1.clone())
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
    fn drain_requests(stream: &mut TcpStream) -> io::Result<()> {
        stream.set_nonblocking(true)?;
        let outcome = Self::drain(stream);
        stream.set_nonblocking(false)?;
        outcome
    }

    fn drain(stream: &mut TcpStream) -> io::Result<()> {
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
            let result = Self::consume_body(stream, number[0]);
            stream.set_nonblocking(true)?;
            result?;
        }
    }

    fn consume_body(stream: &mut TcpStream, number: u8) -> io::Result<()> {
        if let Some(length) = client_message_length(number) {
            let mut body = vec![0_u8; length];
            return stream.read_exact(&mut body);
        }
        match number {
            client_message::SET_ENCODINGS => {
                let mut head = [0_u8; 3];
                stream.read_exact(&mut head)?;
                let count = u16::from_be_bytes([head[1], head[2]]) as usize;
                let mut body = vec![0_u8; count * 4];
                stream.read_exact(&mut body)
            }
            client_message::CLIENT_CUT_TEXT => {
                let mut head = [0_u8; 7];
                stream.read_exact(&mut head)?;
                let length = u32::from_be_bytes([head[3], head[4], head[5], head[6]]) as usize;
                let mut body = vec![0_u8; length];
                stream.read_exact(&mut body)
            }
            // An unknown message number cannot be skipped, because its length is unknown. The
            // connection is the only thing that can be resynchronised.
            _ => Err(io::Error::from(ErrorKind::InvalidData)),
        }
    }
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
