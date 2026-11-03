//! sill: process lifetimes on Windows, through Job Objects.
//!
//! Windows has no process groups, and a process tree followed through parent PIDs loses
//! every process whose parent has exited: `npx` and `cmd /c start` wrappers leave the real
//! server orphaned, and `taskkill /T` (how harnesses stop a command) misses it, so the port
//! stays taken. A Job Object holds every process started inside it, whatever happens to
//! their parents, so:
//!
//! - Each foreground program runs in a job of its own that kills everything in it when its
//!   last handle closes. If the shell is killed while the program runs (a harness's
//!   timeout), Windows closes the handle and the whole tree ends. When the program exits
//!   normally, that setting is cleared first, so what it deliberately leaves running (`code
//!   .`, `start <url>`, a build daemon) goes on, as it would from a terminal.
//! - A background job (`cmd &`) runs its programs in one job that outlives the shell
//!   (`server &` must keep running after a harness's `bash -c` returns). `kill` of the job's
//!   `$!` ends the whole job.
//!
//! Every program starts suspended, joins its job, and only then runs: nothing it starts can
//! escape before it's in the job.

#[cfg(windows)]
pub use imp::*;
#[cfg(not(windows))]
pub use stub::*;

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;
    use std::os::windows::io::RawHandle;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

    use crate::sys::process::ProcessId;

    /// The creation flag every program starts with (see [`start`]).
    pub const SPAWN_FLAGS: u32 = CREATE_SUSPENDED;

    /// A Job Object.
    #[derive(Debug)]
    pub struct Job(isize);

    // SAFETY: a job handle can be used from any thread.
    unsafe impl Send for Job {}
    // SAFETY: as above; the calls made with it are thread-safe.
    unsafe impl Sync for Job {}

    impl Job {
        /// Creates a job; with `kill_on_close`, closing its last handle ends every process
        /// in it.
        pub fn new(kill_on_close: bool) -> std::io::Result<Self> {
            // SAFETY: no name, default security; checked below.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = Self(handle as isize);
            job.set_kill_on_close(kill_on_close);
            Ok(job)
        }

        const fn handle(&self) -> HANDLE {
            self.0 as HANDLE
        }

        /// Sets or clears kill-on-close.
        pub fn set_kill_on_close(&self, on: bool) {
            // SAFETY: a zeroed limit structure is valid; only the flag is set.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            if on {
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            }
            // SAFETY: a valid job handle; `info` is the structure the class names.
            unsafe {
                SetInformationJobObject(
                    self.handle(),
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    u32::try_from(std::mem::size_of_val(&info)).unwrap_or(0),
                );
            }
        }

        /// Puts a process in the job. False if Windows refuses (the process is in a job
        /// that forbids nesting).
        pub fn assign(&self, process: RawHandle) -> bool {
            // SAFETY: both handles are valid.
            unsafe { AssignProcessToJobObject(self.handle(), process.cast()) != 0 }
        }

        /// How many processes in the job are still running.
        pub fn active_processes(&self) -> u32 {
            self.accounting().map_or(0, |info| info.ActiveProcesses)
        }

        fn accounting(&self) -> Option<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION> {
            // SAFETY: a zeroed accounting structure is valid to fill.
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: a valid job handle; `info` is the structure the class names.
            let ok = unsafe {
                QueryInformationJobObject(
                    self.handle(),
                    JobObjectBasicAccountingInformation,
                    (&raw mut info).cast(),
                    u32::try_from(std::mem::size_of_val(&info)).unwrap_or(0),
                    std::ptr::null_mut(),
                )
            } != 0;
            ok.then_some(info)
        }

        /// Adds the CPU time of the job's processes so far to [`children_times`].
        fn account(&self) {
            if let Some(info) = self.accounting() {
                let user = u64::try_from(info.TotalUserTime).unwrap_or(0);
                let kernel = u64::try_from(info.TotalKernelTime).unwrap_or(0);
                CHILD_USER.fetch_add(user, Ordering::Relaxed);
                CHILD_KERNEL.fetch_add(kernel, Ordering::Relaxed);
            }
        }

        /// Ends every process in the job with `exit_code`.
        pub fn terminate(&self, exit_code: u32) -> bool {
            // SAFETY: a valid job handle.
            unsafe { TerminateJobObject(self.handle(), exit_code) != 0 }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: a handle this owns, closed once.
            unsafe { CloseHandle(self.handle()) };
        }
    }

    /// A foreground program's job, kill-on-close while the program runs.
    #[derive(Debug)]
    pub struct ForegroundJob(Option<Job>);

    impl ForegroundJob {
        /// The program exited: what it leaves running stays (see the module docs).
        pub fn release(mut self) {
            if let Some(job) = self.0.take() {
                job.account();
                job.set_kill_on_close(false);
            }
        }
    }

    impl Drop for ForegroundJob {
        fn drop(&mut self) {
            if let Some(job) = &self.0 {
                job.account();
            }
        }
    }

    /// CPU time of finished foreground programs and what they started, in 100 ns units:
    /// Windows' counterpart of `RUSAGE_CHILDREN`, for `time` and `times`.
    static CHILD_USER: AtomicU64 = AtomicU64::new(0);
    static CHILD_KERNEL: AtomicU64 = AtomicU64::new(0);

    /// The user and system CPU time of the shell's finished programs (see [`CHILD_USER`]).
    pub fn children_times() -> (std::time::Duration, std::time::Duration) {
        let d = |t: &AtomicU64| std::time::Duration::from_nanos(t.load(Ordering::Relaxed).saturating_mul(100));
        (d(&CHILD_USER), d(&CHILD_KERNEL))
    }

    /// Starts `cmd` (created with [`SPAWN_FLAGS`]) in `background`'s job, or in a new
    /// foreground job, and lets it run. A process that can't join a job (Windows refuses a
    /// nesting) still runs, without one.
    pub fn start(
        cmd: &mut tokio::process::Command,
        background: Option<&Arc<Job>>,
    ) -> std::io::Result<(tokio::process::Child, Option<ForegroundJob>)> {
        let mut child = cmd.spawn()?;
        let foreground = match background {
            Some(job) => {
                if let Some(h) = child.raw_handle() {
                    job.assign(h);
                }
                None
            }
            None => Job::new(true).ok().map(|job| {
                let joined = child.raw_handle().is_some_and(|h| job.assign(h));
                ForegroundJob(joined.then_some(job))
            }),
        };
        let resumed = match child.raw_handle() {
            Some(handle) => resume(handle),
            None => Err(std::io::Error::other("the started process has no handle")),
        };
        if let Err(e) = resumed {
            let _ = child.start_kill();
            return Err(e);
        }
        Ok((child, foreground))
    }

    /// As [`start`], for a `std` process (`exec`): always a foreground job.
    pub fn start_std(
        cmd: &mut std::process::Command,
    ) -> std::io::Result<(std::process::Child, Option<ForegroundJob>)> {
        use std::os::windows::io::AsRawHandle as _;
        let mut child = cmd.spawn()?;
        let foreground = Job::new(true).ok().map(|job| {
            let joined = job.assign(child.as_raw_handle());
            ForegroundJob(joined.then_some(job))
        });
        if let Err(e) = resume(child.as_raw_handle()) {
            let _ = child.kill();
            return Err(e);
        }
        Ok((child, foreground))
    }

    #[link(name = "ntdll", kind = "raw-dylib")]
    unsafe extern "system" {
        fn NtResumeProcess(process: HANDLE) -> i32;
    }

    /// Resumes a process created suspended, by its handle: one call, where finding its
    /// thread would take a snapshot of every thread on the machine.
    fn resume(process: RawHandle) -> std::io::Result<()> {
        // SAFETY: a valid process handle (the child's own).
        let status = unsafe { NtResumeProcess(process.cast()) };
        if status >= 0 {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("couldn't resume the process (NTSTATUS {status:#x})")))
        }
    }

    /// A new background job (see the module docs).
    pub fn new_background() -> Option<Arc<Job>> {
        Job::new(false).ok().map(Arc::new)
    }

    /// Background jobs by their `$!`, for `kill`. Kept until the shell exits: a job's
    /// processes can outlive the task that started them (`cmd /c start /b server &`).
    static BACKGROUND: Mutex<Option<HashMap<ProcessId, Arc<Job>>>> = Mutex::new(None);

    /// Records `pid` as the `$!` of `job`.
    pub fn register_background(pid: ProcessId, job: &Arc<Job>) {
        let mut map = BACKGROUND.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        map.get_or_insert_with(HashMap::new).insert(pid, Arc::clone(job));
    }

    /// The background job whose `$!` is `pid`, if any.
    pub fn background_job_of(pid: ProcessId) -> Option<Arc<Job>> {
        let map = BACKGROUND.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        map.as_ref()?.get(&pid).cloned()
    }

    /// The `$!` of each background job with processes still running.
    pub fn running_background() -> Vec<ProcessId> {
        let map = BACKGROUND.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut pids: Vec<ProcessId> = map
            .iter()
            .flatten()
            .filter(|(_, job)| job.active_processes() > 0)
            .map(|(&pid, _)| pid)
            .collect();
        pids.sort_unstable();
        pids
    }

    /// Whether `exe` is Windows' own `sudo.exe` (in System32) while sudo is turned off in
    /// Settings (`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Sudo`, `Enabled` 0 or
    /// missing).
    pub fn is_disabled_windows_sudo(exe: &std::path::Path) -> bool {
        use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
        let in_system32 = std::env::var_os("SystemRoot").is_some_and(|root| {
            let sys32 = std::path::Path::new(&root).join("System32");
            exe.parent().is_some_and(|dir| same_dir(dir, &sys32))
        });
        if !in_system32 {
            return false;
        }
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let key = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Sudo");
        let value = wide("Enabled");
        let mut data = 0u32;
        let mut size = 4u32;
        // SAFETY: a DWORD read into a buffer of its size.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&raw mut data).cast(),
                &mut size,
            )
        };
        status != 0 || data == 0
    }

    /// Whether two directory paths are the same, whichever separators and case they use:
    /// the shell finds programs through PATH entries in `C:/WINDOWS/system32` form.
    pub(crate) fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
        let norm = |p: &std::path::Path| {
            p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
        };
        norm(a) == norm(b)
    }

    /// Whether this process's standard output is a pipe (a harness reading it).
    pub fn stdout_is_pipe() -> bool {
        use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType};
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};
        // SAFETY: plain Win32 calls; an invalid handle is just not a pipe.
        unsafe { GetFileType(GetStdHandle(STD_OUTPUT_HANDLE)) == FILE_TYPE_PIPE }
    }
}

#[cfg(not(windows))]
mod stub {
    use std::sync::Arc;

    use crate::sys::process::ProcessId;

    /// No Job Objects here; a placeholder.
    #[derive(Debug)]
    pub struct Job;

    /// No background jobs of this kind here.
    pub const fn new_background() -> Option<Arc<Job>> {
        None
    }

    /// Nothing to record here.
    pub const fn register_background(_pid: ProcessId, _job: &Arc<Job>) {}

    /// Windows only.
    pub const fn is_disabled_windows_sudo(_exe: &std::path::Path) -> bool {
        false
    }

    /// None here.
    pub const fn running_background() -> Vec<ProcessId> {
        Vec::new()
    }

    /// Not asked here (on Unix this is no shell's business).
    pub const fn stdout_is_pipe() -> bool {
        false
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn system32_in_either_separator() {
        use std::path::Path;
        assert!(same_dir(Path::new("C:/WINDOWS/system32"), Path::new(r"C:\Windows\System32")));
        assert!(same_dir(Path::new(r"C:\Windows\System32\"), Path::new(r"c:\windows\system32")));
        assert!(!same_dir(Path::new("C:/WINDOWS"), Path::new(r"C:\Windows\System32")));
    }

    #[test]
    fn killing_the_job_ends_an_orphaned_grandchild() {
        // cmd starts ping and exits at once: ping's parent is gone, so only the job has it.
        let job = new_background().unwrap();
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.args(["/c", "start", "/b", "ping", "-n", "30", "127.0.0.1"]);
        cmd.stdout(std::process::Stdio::null());
        std::os::windows::process::CommandExt::creation_flags(cmd.as_std_mut(), SPAWN_FLAGS);
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (mut child, fg) = start(&mut cmd, Some(&job)).unwrap();
            assert!(fg.is_none());
            child.wait().await.unwrap();
        });
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(job.active_processes(), 1, "ping, orphaned, still in the job");
        assert!(job.terminate(1));
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(job.active_processes(), 0);
    }
}
