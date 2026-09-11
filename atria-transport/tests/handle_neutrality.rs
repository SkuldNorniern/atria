//! That the transport abstraction names no platform handle.
//!
//! The Unix backend is the only transport that exists, so the trait could describe file
//! descriptors and nothing would complain. This implements a transport whose handle is not a
//! descriptor and never becomes one. It compiles only while the abstraction stays neutral, which
//! is the property Artery needs before `atriad` can run on it.

use std::collections::BTreeMap;

use atria_compositor::{HandleResolver, SharedMemory};
use atria_protocol::wire::{HandleIndex, HandleKind};
use atria_transport::{Envelope, EnvelopeResolver, SharedMemorySource, Transport, TransportError};

/// A handle that is a name and a block of bytes, the way a capability is a name and an authority.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Grant {
    name: u64,
    bytes: Vec<u8>,
}

#[derive(Debug, Default)]
struct GrantStore {
    next_id: u64,
    live: BTreeMap<u64, Grant>,
}

#[derive(Debug, Eq, PartialEq)]
enum GrantError {
    Unknown,
    Short,
}

impl SharedMemorySource for GrantStore {
    type Handle = Grant;
    type Error = GrantError;

    fn adopt(&mut self, handle: Grant) -> Result<SharedMemory, GrantError> {
        let size = handle.bytes.len() as u64;
        self.next_id += 1;
        let id = self.next_id;
        self.live.insert(id, handle);
        Ok(SharedMemory::new(id, size))
    }

    fn read(&self, memory: SharedMemory, offset: u32, len: usize) -> Result<Vec<u8>, GrantError> {
        let grant = self.live.get(&memory.id()).ok_or(GrantError::Unknown)?;
        let start = offset as usize;
        let end = start.checked_add(len).ok_or(GrantError::Short)?;
        grant
            .bytes
            .get(start..end)
            .map(<[u8]>::to_vec)
            .ok_or(GrantError::Short)
    }

    fn release(&mut self, memory: SharedMemory) -> bool {
        self.live.remove(&memory.id()).is_some()
    }

    fn len(&self) -> usize {
        self.live.len()
    }
}

/// A transport that hands back whatever was put into it, carrying grants rather than descriptors.
#[derive(Debug, Default)]
struct GrantTransport {
    queued: Vec<(Vec<u8>, Vec<Grant>)>,
}

impl Transport for GrantTransport {
    type Handle = Grant;
    type Memory = GrantStore;

    fn send(&mut self, bytes: &[u8], handles: &[&Grant]) -> Result<(), TransportError> {
        self.queued.push((
            bytes.to_vec(),
            handles.iter().map(|grant| (*grant).clone()).collect(),
        ));
        Ok(())
    }

    fn receive(&mut self) -> Result<Envelope<Grant>, TransportError> {
        if self.queued.is_empty() {
            return Err(TransportError::Closed);
        }
        let (bytes, handles) = self.queued.remove(0);
        Ok(Envelope::new(bytes, handles))
    }
}

#[test]
fn a_transport_carries_a_handle_that_is_not_a_descriptor() {
    let mut transport = GrantTransport::default();
    let grant = Grant {
        name: 7,
        bytes: vec![0xa5; 64],
    };
    transport
        .send(b"a message", &[&grant])
        .expect("a grant travels beside a message");

    let mut envelope = transport.receive().expect("it arrives");
    assert_eq!(envelope.bytes(), b"a message");
    assert_eq!(envelope.handle_count(), 1);

    let mut store = GrantStore::default();
    let mut resolver = EnvelopeResolver::new(&mut envelope, &mut store);
    let memory = resolver
        .shared_memory(HandleIndex::new(0, HandleKind::SharedMemory))
        .expect("the slot resolves to memory");

    assert_eq!(memory.size(), 64, "the size comes from the grant itself");
    assert_eq!(store.len(), 1, "and the store keeps what it adopted");
    assert_eq!(
        store.read(memory, 0, 4),
        Ok(vec![0xa5; 4]),
        "the compositor reads through the identity, never through the handle"
    );
}

#[test]
fn a_claimed_slot_is_empty_for_the_next_reader() {
    let mut transport = GrantTransport::default();
    let grant = Grant {
        name: 1,
        bytes: vec![0; 8],
    };
    transport.send(b"once", &[&grant]).expect("send");
    let mut envelope = transport.receive().expect("receive");

    let mut store = GrantStore::default();
    let slot = HandleIndex::new(0, HandleKind::SharedMemory);
    {
        let mut resolver = EnvelopeResolver::new(&mut envelope, &mut store);
        resolver.shared_memory(slot).expect("the first claim");
    }
    let mut resolver = EnvelopeResolver::new(&mut envelope, &mut store);
    assert!(
        resolver.shared_memory(slot).is_err(),
        "a slot names a resource once, so naming it twice cannot hand it over twice"
    );
}
