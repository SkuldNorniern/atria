//! Keeping the shared memory a client handed over, so the compositor need not.
//!
//! The compositor holds a [`SharedMemory`] — an identity and a size. The descriptor behind it
//! lives here, for as long as the object that named it. Nothing above this maps anything, which
//! is what lets an Artery memory capability sit behind the same contract.

use std::collections::BTreeMap;
use std::io;
use std::mem::zeroed;
use std::os::fd::{AsRawFd, OwnedFd};

use libc::{S_IFMT, S_IFREG, fstat, stat};

use atria_compositor::SharedMemory;

/// Descriptors adopted from messages, keyed by the identity the compositor holds.
#[derive(Debug, Default)]
pub struct SharedMemoryStore {
    next_id: u64,
    live: BTreeMap<u64, OwnedFd>,
}

impl SharedMemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: 1,
            live: BTreeMap::new(),
        }
    }

    /// Take a descriptor and record how much memory it holds.
    ///
    /// The size is read from the descriptor rather than taken from the message, so a client
    /// cannot describe more memory than it handed over. A descriptor that is not a sized
    /// memory object is refused here rather than at first use.
    ///
    /// # Errors
    ///
    /// Returns the platform's error when the descriptor cannot be measured, or when it is not a
    /// regular sized object — a socket or a pipe has no size to carve buffers out of.
    pub fn adopt(&mut self, handle: OwnedFd) -> io::Result<SharedMemory> {
        let size = measure(&handle)?;
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("shared-memory identities exhausted"))?;
        self.live.insert(id, handle);
        Ok(SharedMemory::new(id, size))
    }

    /// Release what an identity named. Returns whether it was held.
    pub fn release(&mut self, memory: SharedMemory) -> bool {
        self.live.remove(&memory.id()).is_some()
    }

    /// How many descriptors this store holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }
}

/// Bytes the descriptor holds, refusing anything that is not a sized memory object.
fn measure(handle: &OwnedFd) -> io::Result<u64> {
    // SAFETY: `fstat` writes one `stat` and reads only the descriptor, which is borrowed here.
    let status = unsafe {
        let mut status = zeroed::<stat>();
        if fstat(handle.as_raw_fd(), &raw mut status) < 0 {
            return Err(io::Error::last_os_error());
        }
        status
    };
    if status.st_mode & S_IFMT != S_IFREG {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    u64::try_from(status.st_size).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
}
