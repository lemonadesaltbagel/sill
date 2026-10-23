#![allow(clippy::missing_const_for_fn)]
#![allow(clippy::unnecessary_wraps)]

use crate::error;
use std::path::PathBuf;

/// Placeholder UID for Windows processes.
///
/// Real Unix-style UIDs don't exist on Windows; this value is a
/// conventional non-root sentinel (matching the typical first
/// regular-user UID on Linux).
///
/// sill: also when elevated (brush reported 0 then). Developers on Windows commonly work
/// as administrators, and a root uid made scripts that refuse to run as root (`[ "$EUID"
/// -eq 0 ] && exit 1`) stop there, where Git Bash, whose uid never changes with
/// elevation, runs them. Windows itself still enforces what the token allows.
const USER_UID: u32 = 1000;

/// Placeholder GID for Windows processes (see [`USER_UID`]).
const USER_GID: u32 = 1000;

pub(crate) fn get_user_home_dir(_username: &str) -> Option<PathBuf> {
    // std::env::home_dir() doesn't support getting home dir for arbitrary users
    // For now, we only support getting the current user's home dir
    None
}

pub(crate) fn get_current_user_home_dir() -> Option<PathBuf> {
    std::env::home_dir()
}

/// sill: Windows has no login shell, so this shell (as Git Bash sets `SHELL` to its own
/// bash): `SHELL` was left unset, and scripts that run `"$SHELL" -c ...` ran nothing.
pub(crate) fn get_current_user_default_shell() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(PathBuf::from(crate::sys::fs::display_path(&exe)))
}

pub(crate) fn is_root() -> bool {
    false
}

pub(crate) fn get_current_uid() -> Result<u32, error::Error> {
    Ok(USER_UID)
}

pub(crate) fn get_current_gid() -> Result<u32, error::Error> {
    Ok(USER_GID)
}

pub(crate) fn get_effective_uid() -> Result<u32, error::Error> {
    Ok(USER_UID)
}

pub(crate) fn get_effective_gid() -> Result<u32, error::Error> {
    Ok(USER_GID)
}

pub(crate) fn get_current_username() -> Result<String, error::Error> {
    let username = whoami::username().map_err(std::io::Error::from)?;
    Ok(username)
}

#[allow(clippy::unnecessary_wraps)]
pub(crate) fn get_user_group_ids() -> Result<Vec<u32>, error::Error> {
    // TODO(windows): implement some version of this for Windows
    Ok(vec![])
}

#[expect(clippy::unnecessary_wraps)]
pub(crate) fn get_all_users() -> Result<Vec<String>, error::Error> {
    // TODO(windows): implement some version of this for Windows
    Ok(vec![])
}

#[expect(clippy::unnecessary_wraps)]
pub(crate) fn get_all_groups() -> Result<Vec<String>, error::Error> {
    // TODO(windows): implement some version of this for Windows
    Ok(vec![])
}
