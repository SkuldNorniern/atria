//! What a shell may do to windows it did not create.
//!
//! A child of `state` because arranging a window is changing the same scene everything else
//! changes. Kept apart because the authority is different in kind: an application acts on its own
//! objects, and a shell acts on everyone's, by a handle it was given rather than an identifier it
//! could have guessed.

use alloc::string::String;
use alloc::vec::Vec;

use atria_protocol::ObjectId;
use atria_protocol::capability::Capability;

use super::{CompositorState, Object};
use crate::model::{ConnectionId, Point, SeatId, Size, SurfaceKey};
use crate::shell::{ShellError, ToplevelHandle, ToplevelRecord};

impl CompositorState {
    /// Every window that exists, as a shell refers to them.
    ///
    /// This is the restart contract in one call: a shell starting for the first time and a shell
    /// replacing one that died are handed the same thing, so neither has to be a special case and
    /// a replacement never begins from nothing.
    #[must_use]
    pub fn shell_toplevels(&self) -> Vec<ToplevelRecord> {
        self.toplevels
            .iter()
            .filter_map(|(handle, (connection, object_id))| {
                let entry = self
                    .connections
                    .get(connection)?
                    .registry
                    .entry(*object_id)
                    .ok()?;
                let Object::Toplevel(state) = &entry.value else {
                    return None;
                };
                Some(ToplevelRecord {
                    handle: *handle,
                    connection: *connection,
                    object_id: *object_id,
                    title: String::from(state.title.as_str()),
                    configured: state.configured,
                })
            })
            .collect()
    }

    /// Resolve a shell's handle, checking the shell holds the authority to use it.
    pub(super) fn shell_target(
        &self,
        shell: ConnectionId,
        handle: ToplevelHandle,
    ) -> Result<(ConnectionId, ObjectId), ShellError> {
        // Asked before the handle is looked up, so a program without the grant learns nothing
        // about which windows exist by probing handles.
        if !self
            .capabilities(shell)
            .is_some_and(|held| held.contains(Capability::ShellControl))
        {
            return Err(ShellError::NotGranted);
        }
        self.toplevels
            .get(&handle)
            .copied()
            .ok_or(ShellError::UnknownHandle { handle })
    }

    /// Ask a window to adopt a size and a set of states, on a shell's behalf.
    ///
    /// # Errors
    ///
    /// Returns [`ShellError::NotGranted`] without the shell-control grant, and
    /// [`ShellError::UnknownHandle`] for a window that has gone.
    pub fn shell_configure(
        &mut self,
        shell: ConnectionId,
        handle: ToplevelHandle,
        size: Size,
        state: u32,
    ) -> Result<u32, ShellError> {
        let (connection, object_id) = self.shell_target(shell, handle)?;
        self.configure_toplevel(connection, object_id, size, state)
            .map_err(|_| ShellError::UnknownHandle { handle })
    }

    /// Ask a window to close, on a shell's behalf. The window goes when its own client destroys it.
    ///
    /// # Errors
    ///
    /// As [`Self::shell_configure`].
    pub fn shell_close(
        &mut self,
        shell: ConnectionId,
        handle: ToplevelHandle,
    ) -> Result<(), ShellError> {
        let (connection, object_id) = self.shell_target(shell, handle)?;
        self.close_toplevel(connection, object_id)
            .map_err(|_| ShellError::UnknownHandle { handle })
    }

    /// Place a window's surface, on a shell's behalf.
    ///
    /// # Errors
    ///
    /// As [`Self::shell_configure`].
    pub fn shell_place(
        &mut self,
        shell: ConnectionId,
        handle: ToplevelHandle,
        position: Point,
    ) -> Result<(), ShellError> {
        let (connection, object_id) = self.shell_target(shell, handle)?;
        let surface = self
            .toplevel_surface(connection, object_id)
            .ok_or(ShellError::UnknownHandle { handle })?;
        self.place_surface(
            SurfaceKey {
                connection,
                object_id: surface,
            },
            position,
        )
        .map_err(|_| ShellError::UnknownHandle { handle })
    }

    /// Raise a window's surface to the front, on a shell's behalf.
    ///
    /// # Errors
    ///
    /// As [`Self::shell_configure`].
    pub fn shell_raise(
        &mut self,
        shell: ConnectionId,
        handle: ToplevelHandle,
    ) -> Result<(), ShellError> {
        let (connection, object_id) = self.shell_target(shell, handle)?;
        let surface = self
            .toplevel_surface(connection, object_id)
            .ok_or(ShellError::UnknownHandle { handle })?;
        self.raise_surface(SurfaceKey {
            connection,
            object_id: surface,
        })
        .map_err(|_| ShellError::UnknownHandle { handle })
    }

    /// Move keyboard focus to a window, on a shell's behalf.
    ///
    /// # Errors
    ///
    /// As [`Self::shell_configure`].
    pub fn shell_focus(
        &mut self,
        shell: ConnectionId,
        seat: SeatId,
        handle: ToplevelHandle,
    ) -> Result<(), ShellError> {
        self.known_seat(seat)?;
        let (connection, object_id) = self.shell_target(shell, handle)?;
        let surface = self
            .toplevel_surface(connection, object_id)
            .ok_or(ShellError::UnknownHandle { handle })?;
        self.set_keyboard_focus(Some(SurfaceKey {
            connection,
            object_id: surface,
        }))
        .map(|_| ())
        .map_err(|_| ShellError::UnknownHandle { handle })
    }

    /// Give a shell the pointer until the button that started it comes up. Refused when nothing
    /// is pressed: a grab with nothing to end it is global input observation.
    ///
    /// # Errors
    ///
    /// As [`Self::shell_configure`], and [`ShellError::UnknownHandle`] when nothing is pressed.
    pub fn shell_grab(
        &mut self,
        shell: ConnectionId,
        seat: SeatId,
        handle: ToplevelHandle,
        control: ObjectId,
    ) -> Result<(), ShellError> {
        self.known_seat(seat)?;
        // Resolved for its own sake: a grab names a window, and a handle that answers to nothing
        // is refused before the pointer is handed over.
        self.shell_target(shell, handle)?;
        if self.seat.pointer.buttons_held == 0 {
            return Err(ShellError::UnknownHandle { handle });
        }
        // The client stops seeing the pointer, rather than waiting for a release it will not get.
        if let Some(target) = self.seat.pointer.target.take() {
            self.send_pointer_leave(target);
        }
        self.seat.pointer.shell_grab = Some((shell, control));
        Ok(())
    }

    /// Refuse a seat this compositor does not have.
    ///
    /// There is one, and its name is checked rather than assumed. A shell that names another is
    /// asking about something that does not exist, and should be told so rather than having its
    /// request quietly applied to the only seat there is.
    pub(super) fn known_seat(&self, seat: SeatId) -> Result<(), ShellError> {
        (seat == SeatId(1))
            .then_some(())
            .ok_or(ShellError::UnknownSeat { seat })
    }
}
