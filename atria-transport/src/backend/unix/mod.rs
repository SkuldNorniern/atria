//! `SOCK_SEQPACKET` over a Unix domain socket, with handles passed as `SCM_RIGHTS`.
//!
//! `SEQPACKET` is what makes framing free: the kernel preserves message boundaries, so one
//! `recvmsg` is exactly one protocol message and the length in the header is checked against
//! what arrived rather than used to find the end of it.
//!
//! All of this file's `unsafe` is the two `sendmsg`/`recvmsg` calls and the control-message walk
//! between them. Everything above it sees owned descriptors and slices.

use std::io;
use std::mem;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::ptr;

use libc::{
    CMSG_DATA, CMSG_FIRSTHDR, CMSG_LEN, CMSG_NXTHDR, CMSG_SPACE, MSG_CMSG_CLOEXEC, MSG_CTRUNC,
    MSG_NOSIGNAL, MSG_TRUNC, SCM_RIGHTS, SOL_SOCKET, c_void, iovec, msghdr, recvmsg, sendmsg,
};

use crate::error::TransportError;

/// Handles this transport carries with one message.
///
/// The wire allows 256 slots. Linux caps `SCM_RIGHTS` well below that, and nothing in the
/// protocol names more than one handle in a single message, so the transport bounds it here
/// rather than accepting a count it would have to fail on later.
pub const MAX_HANDLES: usize = 8;

/// Space for the control message carrying `MAX_HANDLES` descriptors.
const CONTROL_BYTES: usize = 64 + MAX_HANDLES * mem::size_of::<RawFd>();

/// Send one complete message, with its handles attached out of band.
///
/// # Errors
///
/// Returns the platform's error, or [`TransportError::Closed`] if the peer has gone.
pub fn send(
    socket: BorrowedFd<'_>,
    bytes: &[u8],
    handles: &[BorrowedFd<'_>],
) -> Result<(), TransportError> {
    if handles.len() > MAX_HANDLES {
        return Err(TransportError::TooManyHandles {
            count: handles.len(),
            maximum: MAX_HANDLES,
        });
    }

    let mut iov = iovec {
        iov_base: bytes.as_ptr().cast::<c_void>().cast_mut(),
        iov_len: bytes.len(),
    };
    let mut control = [0_u8; CONTROL_BYTES];
    let mut header: msghdr = unsafe { mem::zeroed() };
    header.msg_iov = &raw mut iov;
    header.msg_iovlen = 1;

    if !handles.is_empty() {
        let payload = mem::size_of_val(handles);
        // SAFETY: `control` is at least `CMSG_SPACE(MAX_HANDLES * size_of::<RawFd>())` bytes and
        // `handles` is no longer than `MAX_HANDLES`, checked above.
        unsafe {
            header.msg_control = control.as_mut_ptr().cast::<c_void>();
            header.msg_controllen = CMSG_SPACE(payload as u32) as _;
            let control_header = CMSG_FIRSTHDR(&raw const header);
            (*control_header).cmsg_level = SOL_SOCKET;
            (*control_header).cmsg_type = SCM_RIGHTS;
            (*control_header).cmsg_len = CMSG_LEN(payload as u32) as _;
            let target = CMSG_DATA(control_header).cast::<RawFd>();
            for (index, handle) in handles.iter().enumerate() {
                ptr::write_unaligned(target.add(index), handle.as_raw_fd());
            }
        }
    }

    // SAFETY: `header` describes one iovec over `bytes` and, when handles are attached, a control
    // message wholly inside `control`. Both outlive the call.
    let sent = unsafe { sendmsg(socket.as_raw_fd(), &raw const header, MSG_NOSIGNAL) };
    if sent < 0 {
        let error = io::Error::last_os_error();
        return match error.kind() {
            io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset => {
                Err(TransportError::Closed)
            }
            _ => Err(TransportError::Io(error)),
        };
    }
    Ok(())
}

/// One received message: the bytes that arrived and the descriptors that came with them.
pub struct Received {
    pub bytes: Vec<u8>,
    pub handles: Vec<OwnedFd>,
}

/// Receive one complete message.
///
/// `capacity` bounds what will be accepted; `SEQPACKET` truncates a longer message rather than
/// splitting it, which is reported rather than silently returning a partial message.
///
/// # Errors
///
/// Returns [`TransportError::Closed`] at end of stream, or the platform's error.
pub fn receive(socket: BorrowedFd<'_>, capacity: usize) -> Result<Received, TransportError> {
    let mut bytes = vec![0_u8; capacity];
    let mut iov = iovec {
        iov_base: bytes.as_mut_ptr().cast::<c_void>(),
        iov_len: bytes.len(),
    };
    let mut control = [0_u8; CONTROL_BYTES];
    let mut header: msghdr = unsafe { mem::zeroed() };
    header.msg_iov = &raw mut iov;
    header.msg_iovlen = 1;
    header.msg_control = control.as_mut_ptr().cast::<c_void>();
    header.msg_controllen = control.len() as _;

    // SAFETY: `header` describes one iovec over `bytes` and a control buffer of `control`, both
    // of which outlive the call and are exclusively borrowed for it.
    let received = unsafe { recvmsg(socket.as_raw_fd(), &raw mut header, MSG_CMSG_CLOEXEC) };
    if received < 0 {
        return Err(TransportError::Io(io::Error::last_os_error()));
    }
    if received == 0 {
        return Err(TransportError::Closed);
    }

    // Collected before any early return: a descriptor the kernel attached is owned by this
    // process the moment `recvmsg` succeeds, so failing to adopt one leaks it.
    let mut handles = Vec::new();
    // SAFETY: the walk uses the kernel's own macros over the control buffer `recvmsg` filled,
    // bounded by the `msg_controllen` it wrote back.
    unsafe {
        let mut control_header = CMSG_FIRSTHDR(&raw const header);
        while !control_header.is_null() {
            if (*control_header).cmsg_level == SOL_SOCKET
                && (*control_header).cmsg_type == SCM_RIGHTS
            {
                let payload = (*control_header).cmsg_len as usize - CMSG_LEN(0) as usize;
                let count = payload / mem::size_of::<RawFd>();
                let source = CMSG_DATA(control_header).cast::<RawFd>();
                for index in 0..count {
                    let raw = ptr::read_unaligned(source.add(index));
                    handles.push(OwnedFd::from_raw_fd(raw));
                }
            }
            control_header = CMSG_NXTHDR(&raw const header, control_header);
        }
    }

    if header.msg_flags & MSG_CTRUNC != 0 {
        return Err(TransportError::TooManyHandles {
            count: handles.len() + 1,
            maximum: MAX_HANDLES,
        });
    }

    let size = received as usize;
    if header.msg_flags & MSG_TRUNC != 0 {
        return Err(TransportError::MessageTooLarge {
            size,
            maximum: capacity,
        });
    }

    bytes.truncate(size);
    Ok(Received { bytes, handles })
}
