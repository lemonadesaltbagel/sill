//! Environment variable retrieval for Unix platforms.

/// Retrieves environment variables from the host process.
///
/// On Unix, this is a direct passthrough to [`std::env::vars()`].
pub(crate) fn get_host_env_vars() -> impl Iterator<Item = (String, String)> {
    std::env::vars()
}

/// sill: converts a variable's value for a child process's environment. No change here.
#[allow(clippy::missing_const_for_fn, reason = "signature shared with Windows")]
pub(crate) fn child_env_value(_name: &str, value: String) -> String {
    value
}
