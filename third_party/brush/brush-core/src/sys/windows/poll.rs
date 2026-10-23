//! sill: waiting for input with a timeout (`read -t`) on Windows.
//!
//! Windows has no `poll` on arbitrary handles, so each kind of input is asked its own way:
//! a file is always ready (as in bash, `-t` has no effect on one), a pipe has data when
//! `PeekNamedPipe` counts some (or has hit its end), and a console when its input handle is
//! signaled.

use std::os::windows::io::{AsRawHandle, RawHandle};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_CHAR, FILE_TYPE_PIPE, GetFileType};
use windows_sys::Win32::System::Console::GetConsoleMode;
use windows_sys::Win32::System::Pipes::PeekNamedPipe;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

use crate::openfiles::OpenFile;

/// Polls an open file for input readability with a timeout.
///
/// Returns `Ok(true)` if data is available for reading (or the input has ended, so that a
/// read sees that), `Ok(false)` if the timeout elapsed first. `Duration::ZERO` checks
/// without waiting.
pub fn poll_for_input(file: &OpenFile, timeout: Duration) -> std::io::Result<bool> {
    let handle: RawHandle = match file {
        OpenFile::Stdin(f) => f.as_raw_handle(),
        OpenFile::File(f) => f.as_raw_handle(),
        OpenFile::PipeReader(f) => f.as_raw_handle(),
        // Nothing to wait on; a read finds out.
        _ => return Ok(true),
    };
    let deadline = Instant::now() + timeout;

    // SAFETY: a handle the open file owns, alive for this call.
    match unsafe { GetFileType(handle) } {
        FILE_TYPE_PIPE => loop {
            let mut available = 0u32;
            // SAFETY: as above; only the count is asked for.
            let ok = unsafe {
                PeekNamedPipe(
                    handle,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &raw mut available,
                    std::ptr::null_mut(),
                )
            } != 0;
            // A failed peek is a closed pipe: a read sees its end.
            if !ok || available > 0 {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())));
        },
        FILE_TYPE_CHAR => {
            let mut mode = 0u32;
            // SAFETY: as above.
            if unsafe { GetConsoleMode(handle, &raw mut mode) } == 0 {
                // The null device, or another character device: a read doesn't wait.
                return Ok(true);
            }
            let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
            // SAFETY: as above. A console input handle is signaled while it holds input.
            match unsafe { WaitForSingleObject(handle, ms) } {
                WAIT_OBJECT_0 => Ok(true),
                WAIT_TIMEOUT => Ok(false),
                _ => Err(std::io::Error::last_os_error()),
            }
        }
        // A file (or a handle of no known type): always ready.
        _ => Ok(true),
    }
}
