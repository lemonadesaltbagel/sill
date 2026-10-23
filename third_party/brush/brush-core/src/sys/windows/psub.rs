//! sill: process substitution arguments (`diff <(a) <(b)`) on Windows.
//!
//! On Unix the argument is `/dev/fd/N`, a descriptor the child inherits. A Windows program
//! inherits no descriptor beyond the standard three and has no `/dev/fd`, so the argument
//! is instead a named pipe, `\\.\pipe\sill-psub-<pid>-<n>`, which any program can open like
//! a file (GNU diff, cmp, Python, Rust). A thread copies between it and the substituted
//! command, so the data streams as in bash.
//!
//! (A redirection from a process substitution, as in `done < <(cmd)`, doesn't go through
//! here: the shell uses the pipe itself.)

use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_GENERIC_READ, FlushFileBuffers,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, PeekNamedPipe,
};

use crate::openfiles::OpenFile;

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// How long a connection must stay open, before any data moves, to count as the program's
/// real open rather than a look at the path (see `serve`).
const PROBE_GRACE: Duration = Duration::from_millis(20);

/// How long to wait, after a look at the path, for the program to open it for real.
const REOPEN_WAIT: Duration = Duration::from_secs(5);

/// Serves `file` (the substituted command's end of its pipe) as a named pipe and returns
/// the pipe's path, to pass as the argument.
pub(crate) fn named_pipe_for(file: OpenFile) -> std::io::Result<String> {
    if !matches!(file, OpenFile::PipeReader(_) | OpenFile::PipeWriter(_)) {
        return Err(std::io::ErrorKind::Unsupported.into());
    }
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!(r"\\.\pipe\sill-psub-{}-{n}", std::process::id());
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let (tx, rx) = mpsc::channel();
    let pipe = Arc::new(Pipe { wide, waiting: AtomicUsize::new(0), done: AtomicBool::new(false) });
    // Instances wait from the start, and each starts the next as a program connects to it;
    // see `serve`.
    let first = instance(&pipe.wide, true)?;
    listen(first, &pipe, &tx)?;
    for _ in 1..LISTENERS {
        listen(instance(&pipe.wide, false)?, &pipe, &tx)?;
    }
    std::thread::Builder::new()
        .name("sill-psub".into())
        .spawn(move || serve(&pipe, &rx, file))?;
    Ok(name)
}

/// Creates one instance of the pipe named `wide`. Duplex, whichever way the data goes, so
/// that `PeekNamedPipe` can tell whether the program has closed its end.
fn instance(wide: &[u16], first: bool) -> std::io::Result<std::fs::File> {
    let flags = if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
    // SAFETY: `wide` is a NUL-terminated UTF-16 string; checked against
    // INVALID_HANDLE_VALUE below.
    let handle = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_DUPLEX | flags,
            PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a valid handle we own, closed when the File drops.
    Ok(unsafe { std::fs::File::from_raw_handle(handle) })
}

/// One substitution's pipe, shared by the threads serving it.
struct Pipe {
    /// The pipe's name, NUL-terminated UTF-16.
    wide: Vec<u16>,
    /// Instances waiting for a program to connect.
    waiting: AtomicUsize,
    /// Set once the data has moved (or the wait is over): no more instances start.
    done: AtomicBool,
}

/// Instances waiting for a program at any time.
const LISTENERS: usize = 3;

/// Waits, on a thread of its own, for a program to open `server`; then starts another
/// instance waiting (so a program that opens the path again at once finds one) and sends
/// the connection on `tx`.
fn listen(server: std::fs::File, pipe: &Arc<Pipe>, tx: &Sender<std::fs::File>) -> std::io::Result<()> {
    let (pipe, tx) = (Arc::clone(pipe), tx.clone());
    pipe.waiting.fetch_add(1, Ordering::SeqCst);
    let spawned = std::thread::Builder::new().name("sill-psub-listen".into()).spawn({
        let pipe = Arc::clone(&pipe);
        move || {
            // SAFETY: `server` is a valid pipe handle; no OVERLAPPED, so this blocks.
            let connected =
                unsafe { ConnectNamedPipe(server.as_raw_handle(), std::ptr::null_mut()) } != 0;
            // SAFETY: reads the calling thread's last error.
            let connected = connected || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
            if !pipe.done.load(Ordering::SeqCst) {
                let _ = instance(&pipe.wide, false).and_then(|next| listen(next, &pipe, &tx));
            }
            pipe.waiting.fetch_sub(1, Ordering::SeqCst);
            if connected {
                let _ = tx.send(server);
            }
        }
    });
    if spawned.is_err() {
        pipe.waiting.fetch_sub(1, Ordering::SeqCst);
    }
    spawned.map(|_| ())
}

/// Whether the program connected to `server` closes it within `PROBE_GRACE` without
/// writing anything.
fn closes_at_once(server: &std::fs::File) -> bool {
    let start = Instant::now();
    loop {
        let mut available = 0u32;
        // SAFETY: `server` is a valid pipe handle; only the count is asked for.
        let ok = unsafe {
            PeekNamedPipe(
                server.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &raw mut available,
                std::ptr::null_mut(),
            )
        } != 0;
        if !ok {
            // SAFETY: reads the calling thread's last error.
            return unsafe { GetLastError() } == ERROR_BROKEN_PIPE;
        }
        if available > 0 || start.elapsed() >= PROBE_GRACE {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn serve(pipe: &Pipe, rx: &Receiver<std::fs::File>, file: OpenFile) {
    // Many programs look at a path before they read it: Rust's `fs::metadata` (sill's own
    // `cat` and `cp`, ...) opens it, and closes it again within microseconds. With one
    // instance of the pipe, that look took it and the real open failed with "All pipe
    // instances are busy". So several instances wait, and data moves only on a connection
    // that stays open; one closed at once, before any data, was a look.
    //
    // A connection closed at once may also have been a program that read or wrote nothing
    // at all; when no other open follows within REOPEN_WAIT, the pipe closes, so the
    // substituted command sees the end of its input (or output) rather than waiting forever.
    // (A program that never opens the pipe leaves these threads waiting; they hold nothing
    // else.)
    let mut looked = false;
    let server = loop {
        let next = if looked { rx.recv_timeout(REOPEN_WAIT).ok() } else { rx.recv().ok() };
        let Some(server) = next else {
            end_listeners(pipe);
            return;
        };
        if closes_at_once(&server) {
            looked = true;
            continue;
        }
        break server;
    };
    end_listeners(pipe);

    let mut server_ref = &server;
    match file {
        OpenFile::PipeReader(reader) => {
            let _ = copy(&mut &*reader, &mut server_ref);
            // Wait until the program has read everything before closing our end.
            // SAFETY: `server` is a valid pipe handle.
            unsafe { FlushFileBuffers(server.as_raw_handle()) };
        }
        OpenFile::PipeWriter(writer) => {
            let _ = copy(&mut server_ref, &mut &*writer);
        }
        _ => {}
    }
}

/// Ends the threads still waiting for a program to connect, by connecting to each.
fn end_listeners(pipe: &Pipe) {
    pipe.done.store(true, Ordering::SeqCst);
    // (Bounded, should a waiting thread never reach its connection.)
    for _ in 0..1000 {
        if pipe.waiting.load(Ordering::SeqCst) == 0 {
            break;
        }
        // SAFETY: a NUL-terminated UTF-16 string; the handle is closed at once.
        let handle = unsafe {
            CreateFileW(
                pipe.wide.as_ptr(),
                FILE_GENERIC_READ,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            // Busy or gone: let a waiting thread get to its connection first.
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        // SAFETY: a handle we just opened.
        drop(unsafe { std::fs::File::from_raw_handle(handle) });
    }
}

fn copy(from: &mut impl Read, to: &mut impl Write) -> std::io::Result<u64> {
    let n = std::io::copy(from, to)?;
    to.flush()?;
    Ok(n)
}
