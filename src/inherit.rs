//! The two process-wide calls that keep the shepherd's link from the sandboxed
//! command, and the only `unsafe` in the crate.

use std::{io, os::fd::RawFd};

use rustix::{
    fd::BorrowedFd,
    io::{FdFlags, fcntl_getfd, fcntl_setfd},
};

/// Marks `fd` close-on-exec, so no program this process starts holds it.
///
/// # Errors
///
/// The error `fcntl` fails with, such as `EBADF` for a descriptor this process
/// does not hold.
pub(crate) fn close_on_exec(fd: RawFd) -> io::Result<()> {
    // SAFETY: the borrow ends with this function and the descriptor is only
    // read and flagged, never closed. A number that names no open descriptor
    // makes `fcntl` fail with `EBADF` and nothing else.
    #[allow(unsafe_code)]
    let fd = unsafe { BorrowedFd::borrow_raw(fd) };
    let flags = fcntl_getfd(fd)?;
    fcntl_setfd(fd, flags | FdFlags::CLOEXEC)?;
    Ok(())
}

/// Removes `name` from the environment every later child inherits.
///
/// Call it while this process runs one thread, because another thread reading
/// the environment meanwhile is undefined behaviour.
pub(crate) fn unset_var(name: &str) {
    // SAFETY: the caller runs on the process's only thread that touches the
    // environment, which is what `remove_var` requires.
    #[allow(unsafe_code)]
    unsafe {
        std::env::remove_var(name);
    }
}
