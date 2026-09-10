use core::fmt;
use std::error::Error;
use std::io;

/// Why a message could not be carried.
///
/// Separate from a decode failure: nothing here has looked at the message's contents. A transport
/// error says the bytes did not arrive intact, not that they were wrong.
#[derive(Debug)]
pub enum TransportError {
    /// The peer closed the connection.
    Closed,
    /// A message larger than the protocol's ceiling was offered or arrived.
    MessageTooLarge { size: usize, maximum: usize },
    /// A packet arrived that is too short to contain a header.
    Truncated { size: usize },
    /// More handles accompanied one message than this transport carries.
    TooManyHandles { count: usize, maximum: usize },
    /// The platform refused the operation.
    Io(io::Error),
}

impl From<io::Error> for TransportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("the peer closed the connection"),
            Self::MessageTooLarge { size, maximum } => write!(
                formatter,
                "a {size}-byte message exceeds the {maximum}-byte maximum"
            ),
            Self::Truncated { size } => {
                write!(
                    formatter,
                    "a {size}-byte packet cannot hold a message header"
                )
            }
            Self::TooManyHandles { count, maximum } => write!(
                formatter,
                "{count} handles accompanied one message, and at most {maximum} are carried"
            ),
            Self::Io(error) => write!(formatter, "the transport refused the operation: {error}"),
        }
    }
}

impl Error for TransportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}
