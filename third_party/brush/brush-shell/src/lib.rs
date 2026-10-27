//! Create for brush, an executable bash-compatible shell.

#![allow(dead_code)]

pub mod args;
mod brushctl;
pub mod bundled;
pub mod config;
pub mod entry;
mod error_formatter;
pub mod events;
/// sill: public, for sill's `--version`.
pub mod productinfo;
pub mod session_state;
