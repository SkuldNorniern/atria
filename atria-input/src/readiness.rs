use libc::{POLLERR, POLLHUP, POLLIN, POLLNVAL};

/// The actions implied by one descriptor's `poll(2)` result.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DescriptorReadiness {
    readable: bool,
    failed: bool,
}

impl DescriptorReadiness {
    #[must_use]
    pub const fn is_readable(self) -> bool {
        self.readable
    }

    #[must_use]
    pub const fn has_failed(self) -> bool {
        self.failed
    }
}

/// Classifies kernel readiness flags without performing I/O.
///
/// Readability and failure are independent because a hung-up descriptor can still have
/// queued records. Its owner may drain those records before retiring the descriptor.
#[must_use]
pub fn classify_descriptor(revents: i16) -> DescriptorReadiness {
    DescriptorReadiness {
        readable: revents & POLLIN != 0,
        failed: revents & (POLLERR | POLLHUP | POLLNVAL) != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_data_survives_a_simultaneous_hangup() {
        let readiness = classify_descriptor(POLLIN | POLLHUP);
        assert!(readiness.is_readable());
        assert!(readiness.has_failed());
    }
}
