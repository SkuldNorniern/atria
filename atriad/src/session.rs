//! One client's connection, and the pump that serves it.

use atria_compositor::{
    CompositorState, ConnectionId, EmitError, ResolveError, StateError, decode, encode_event,
    resolve,
};
use atria_protocol::wire::{Frame, MAX_MESSAGE_SIZE};
use atria_transport::{Envelope, EnvelopeResolver, SharedMemorySource, Transport, TransportError};

/// Why serving one message stopped.
///
/// The three stages keep their own errors all the way out here, so a caller can tell a client
/// that sent nonsense from one whose transport dropped a handle from one that asked for something
/// its state did not allow.
#[derive(Debug)]
pub enum SessionError {
    /// The connection ended. Not a failure: every client eventually does this.
    Closed,
    /// The transport could not carry the message.
    Transport(TransportError),
}

impl From<TransportError> for SessionError {
    fn from(error: TransportError) -> Self {
        match error {
            TransportError::Closed => Self::Closed,
            other => Self::Transport(other),
        }
    }
}

/// What serving one message did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Served {
    /// Whether the message became a request the compositor accepted.
    pub dispatched: bool,
    /// Events written back to the client.
    pub events_sent: usize,
    /// Events the compositor produced that the draw path has no message for. Counted rather than
    /// dropped silently: a client waiting on one of these would wait forever, and a number that
    /// climbs is where to look.
    pub events_unassigned: usize,
}

/// One client: its transport, its connection in the compositor, and the memory it handed over.
///
/// Generic over the transport because the handle that carries a client's memory is a platform
/// decision. Where the memory is kept follows from the transport rather than being assumed, so a
/// session over Artery capabilities is the same code as a session over a Unix socket.
pub struct Session<T: Transport> {
    transport: T,
    connection: ConnectionId,
    memory: T::Memory,
    sequence: u32,
}

impl<T: Transport> Session<T> {
    /// Attach a transport to a connection the compositor has already accepted.
    #[must_use]
    pub fn new(transport: T, connection: ConnectionId) -> Self {
        Self {
            transport,
            connection,
            memory: T::Memory::default(),
            sequence: 1,
        }
    }

    /// Which connection this session serves.
    #[must_use]
    pub const fn connection(&self) -> ConnectionId {
        self.connection
    }

    /// The transport underneath, for a caller that needs the platform's readiness primitive.
    ///
    /// Serving several clients means waiting on all of them at once, and what "ready" means is
    /// the transport's to say. The session does not know and does not need to.
    #[must_use]
    pub const fn transport(&self) -> &T {
        &self.transport
    }

    /// The memory this client has handed over, for a caller about to read pixels out of it.
    #[must_use]
    pub const fn memory(&self) -> &T::Memory {
        &self.memory
    }

    /// Write out whatever the compositor has queued for this client, outside serving a request.
    ///
    /// Presentation produces events — a buffer release above all — and they are decided between
    /// requests rather than during one.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Closed`] when the client has gone.
    pub fn deliver(&mut self, state: &mut CompositorState) -> Result<(usize, usize), SessionError> {
        self.flush(state)
    }

    /// How much shared memory this client has handed over and the compositor still holds.
    #[must_use]
    pub fn adopted_memory(&self) -> usize {
        self.memory.len()
    }

    /// Read one message, act on it, and write back whatever the compositor decided.
    ///
    /// A request the client got wrong is not an error here: the compositor turns it into an error
    /// event, and this delivers it. Only the transport failing stops the session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Closed`] when the client has gone.
    pub fn serve_one(&mut self, state: &mut CompositorState) -> Result<Served, SessionError> {
        let mut envelope = self.transport.receive()?;
        let mut served = Served::default();

        match self.act(state, &mut envelope) {
            Ok(()) => served.dispatched = true,
            Err(Refused::Protocol) | Err(Refused::Resolution) => served.dispatched = false,
        }

        let (sent, unassigned) = self.flush(state)?;
        served.events_sent = sent;
        served.events_unassigned = unassigned;
        Ok(served)
    }

    /// Decode, resolve and dispatch one message.
    fn act(
        &mut self,
        state: &mut CompositorState,
        envelope: &mut Envelope<T::Handle>,
    ) -> Result<(), Refused> {
        // The bytes come out of the envelope so decoding borrows them while resolution mutates
        // the handles that stayed behind.
        let bytes = envelope.take_bytes();
        let frame = Frame::decode(&bytes).map_err(|_| Refused::Protocol)?;
        let kind = state
            .object_kind(self.connection, frame.header.object_id)
            .ok_or(Refused::Protocol)?;
        let decoded = decode(kind, &frame).map_err(|_| Refused::Protocol)?;

        let request = {
            let mut resolver = EnvelopeResolver::new(envelope, &mut self.memory);
            resolve(decoded, &mut resolver).map_err(Refused::from)?
        };

        state
            .dispatch(self.connection, request)
            .map_err(Refused::from)
    }

    /// Write every event queued for this connection.
    fn flush(&mut self, state: &mut CompositorState) -> Result<(usize, usize), SessionError> {
        let mut buffer = [0_u8; MAX_MESSAGE_SIZE];
        let mut sent = 0;
        let mut unassigned = 0;

        for event in state.take_events_for(self.connection) {
            match encode_event(&event, self.sequence, &mut buffer) {
                Ok(size) => {
                    self.transport.send(&buffer[..size], &[])?;
                    self.sequence = self.sequence.wrapping_add(1);
                    sent += 1;
                }
                Err(EmitError::Unassigned) => unassigned += 1,
                // A buffer of the protocol's own maximum cannot be too small for a message the
                // protocol permits, so this is a compositor fault rather than a client one.
                Err(EmitError::Encode(_)) => unassigned += 1,
            }
        }
        Ok((sent, unassigned))
    }
}

/// Why one message did not become a dispatched request.
enum Refused {
    Protocol,
    Resolution,
}

impl From<ResolveError> for Refused {
    fn from(_: ResolveError) -> Self {
        Self::Resolution
    }
}

impl From<StateError> for Refused {
    fn from(_: StateError) -> Self {
        Self::Protocol
    }
}
