use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use atria_protocol::ObjectId;

use crate::error::StateError;
use crate::model::ObjectKind;

#[derive(Clone, Debug)]
pub(crate) struct RegistryEntry<T> {
    pub kind: ObjectKind,
    pub value: T,
}

/// Per-connection object allocation, lookup, and creation-order tracking.
#[derive(Clone, Debug)]
pub struct ObjectRegistry<T> {
    pub(crate) live: BTreeMap<ObjectId, RegistryEntry<T>>,
    used: BTreeSet<ObjectId>,
    creation_order: Vec<ObjectId>,
}

impl<T> ObjectRegistry<T> {
    #[must_use]
    pub fn new(display: T) -> Self {
        let mut live = BTreeMap::new();
        live.insert(
            ObjectId::DISPLAY,
            RegistryEntry {
                kind: ObjectKind::Display,
                value: display,
            },
        );
        let mut used = BTreeSet::new();
        used.insert(ObjectId::DISPLAY);
        Self {
            live,
            used,
            creation_order: alloc::vec![ObjectId::DISPLAY],
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.live.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// The kind of a live object, or `None` when this registry does not hold it.
    #[must_use]
    pub fn kind_of(&self, id: ObjectId) -> Option<ObjectKind> {
        self.live.get(&id).map(|entry| entry.kind)
    }

    pub(crate) fn allocate_client(
        &mut self,
        id: ObjectId,
        kind: ObjectKind,
        value: T,
    ) -> Result<(), StateError> {
        if !id.is_client_allocatable() {
            return Err(StateError::InvalidObjectId { object_id: id });
        }
        self.allocate(id, kind, value)
    }

    fn allocate(&mut self, id: ObjectId, kind: ObjectKind, value: T) -> Result<(), StateError> {
        // An identifier stays taken until the client has been told it was retired. Reusing one
        // before that would let a message already in flight name a different object than the one
        // its sender meant; `retire` is what closes that window, and it runs only after the
        // compositor has queued the notification.
        if self.used.contains(&id) {
            return Err(StateError::ObjectIdAlreadyUsed { object_id: id });
        }
        self.used.insert(id);
        self.creation_order.push(id);
        self.live.insert(id, RegistryEntry { kind, value });
        Ok(())
    }

    pub(crate) fn entry(&self, id: ObjectId) -> Result<&RegistryEntry<T>, StateError> {
        self.live
            .get(&id)
            .ok_or(StateError::InvalidObject { object_id: id })
    }

    pub(crate) fn entry_mut(&mut self, id: ObjectId) -> Result<&mut RegistryEntry<T>, StateError> {
        self.live
            .get_mut(&id)
            .ok_or(StateError::InvalidObject { object_id: id })
    }

    pub(crate) fn remove(&mut self, id: ObjectId) -> Option<RegistryEntry<T>> {
        self.live.remove(&id)
    }

    pub(crate) fn ids_reverse_creation(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.creation_order.iter().rev().copied()
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.live.keys().copied()
    }

    pub(crate) fn kind(&self, id: ObjectId) -> Result<ObjectKind, StateError> {
        Ok(self.entry(id)?.kind)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Teardown {
    /// Objects destroyed from newest to oldest, including protocol singletons.
    pub destroyed: Vec<(ObjectId, ObjectKind)>,
}
