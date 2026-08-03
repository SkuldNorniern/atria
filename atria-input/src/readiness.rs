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

/// Selects only descriptors for which the kernel reported readable data.
pub fn readable_indices(revents: &[i16]) -> impl Iterator<Item = usize> + '_ {
    revents
        .iter()
        .enumerate()
        .filter_map(|(index, revents)| classify_descriptor(*revents).is_readable().then_some(index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_readable_descriptors_are_selected() {
        let revents = [0, POLLIN, POLLERR, POLLIN | POLLHUP];
        assert_eq!(readable_indices(&revents).collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn readable_data_survives_a_simultaneous_hangup() {
        let readiness = classify_descriptor(POLLIN | POLLHUP);
        assert!(readiness.is_readable());
        assert!(readiness.has_failed());
    }
}
