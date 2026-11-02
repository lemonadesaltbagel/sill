//! sill: CPU times for `time` and `times` on Windows (they printed 0 for user and sys).

use crate::error;

/// Returns the user and system CPU time used by the current process.
pub fn get_self_user_and_system_time()
-> Result<(std::time::Duration, std::time::Duration), error::Error> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the pseudo-handle of this process and four FILETIMEs to fill.
    let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) } != 0;
    let d = |t: FILETIME| {
        std::time::Duration::from_nanos(((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)).saturating_mul(100))
    };
    Ok(if ok { (d(user), d(kernel)) } else { (std::time::Duration::ZERO, std::time::Duration::ZERO) })
}

/// Returns the user and system CPU time used by child processes: the finished foreground
/// programs' Job Objects (see `sys::job::children_times`).
pub fn get_children_user_and_system_time()
-> Result<(std::time::Duration, std::time::Duration), error::Error> {
    Ok(crate::sys::job::children_times())
}
