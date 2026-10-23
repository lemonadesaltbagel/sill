//! sill: signal names and numbers on Windows.
//!
//! Windows has no signals, but scripts name them: `trap cleanup EXIT INT TERM` is the usual
//! way to clean up, and with no signals known the whole `trap` failed, so not even its EXIT
//! handler was registered. These are Linux's names and numbers (`kill -9`, `-15` and `-2`
//! mean the same everywhere). A handler registered for one is kept and shown by `trap -p`.

use crate::error;

/// A signal, by its Linux number.
#[allow(missing_docs, reason = "the signal names")]
#[allow(unnameable_types)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(i32)]
pub enum Signal {
    SIGHUP = 1,
    SIGINT = 2,
    SIGQUIT = 3,
    SIGILL = 4,
    SIGTRAP = 5,
    SIGABRT = 6,
    SIGBUS = 7,
    SIGFPE = 8,
    SIGKILL = 9,
    SIGUSR1 = 10,
    SIGSEGV = 11,
    SIGUSR2 = 12,
    SIGPIPE = 13,
    SIGALRM = 14,
    SIGTERM = 15,
    SIGSTKFLT = 16,
    SIGCHLD = 17,
    SIGCONT = 18,
    SIGSTOP = 19,
    SIGTSTP = 20,
    SIGTTIN = 21,
    SIGTTOU = 22,
    SIGURG = 23,
    SIGXCPU = 24,
    SIGXFSZ = 25,
    SIGVTALRM = 26,
    SIGPROF = 27,
    SIGWINCH = 28,
    SIGIO = 29,
    SIGPWR = 30,
    SIGSYS = 31,
}

const ALL: [Signal; 31] = {
    use Signal::*;
    [
        SIGHUP, SIGINT, SIGQUIT, SIGILL, SIGTRAP, SIGABRT, SIGBUS, SIGFPE, SIGKILL, SIGUSR1,
        SIGSEGV, SIGUSR2, SIGPIPE, SIGALRM, SIGTERM, SIGSTKFLT, SIGCHLD, SIGCONT, SIGSTOP,
        SIGTSTP, SIGTTIN, SIGTTOU, SIGURG, SIGXCPU, SIGXFSZ, SIGVTALRM, SIGPROF, SIGWINCH,
        SIGIO, SIGPWR, SIGSYS,
    ]
};

impl Signal {
    /// Returns an iterator over all signals.
    pub fn iterator() -> impl Iterator<Item = Self> {
        ALL.into_iter()
    }

    /// The signal's name, such as `SIGINT`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SIGHUP => "SIGHUP",
            Self::SIGINT => "SIGINT",
            Self::SIGQUIT => "SIGQUIT",
            Self::SIGILL => "SIGILL",
            Self::SIGTRAP => "SIGTRAP",
            Self::SIGABRT => "SIGABRT",
            Self::SIGBUS => "SIGBUS",
            Self::SIGFPE => "SIGFPE",
            Self::SIGKILL => "SIGKILL",
            Self::SIGUSR1 => "SIGUSR1",
            Self::SIGSEGV => "SIGSEGV",
            Self::SIGUSR2 => "SIGUSR2",
            Self::SIGPIPE => "SIGPIPE",
            Self::SIGALRM => "SIGALRM",
            Self::SIGTERM => "SIGTERM",
            Self::SIGSTKFLT => "SIGSTKFLT",
            Self::SIGCHLD => "SIGCHLD",
            Self::SIGCONT => "SIGCONT",
            Self::SIGSTOP => "SIGSTOP",
            Self::SIGTSTP => "SIGTSTP",
            Self::SIGTTIN => "SIGTTIN",
            Self::SIGTTOU => "SIGTTOU",
            Self::SIGURG => "SIGURG",
            Self::SIGXCPU => "SIGXCPU",
            Self::SIGXFSZ => "SIGXFSZ",
            Self::SIGVTALRM => "SIGVTALRM",
            Self::SIGPROF => "SIGPROF",
            Self::SIGWINCH => "SIGWINCH",
            Self::SIGIO => "SIGIO",
            Self::SIGPWR => "SIGPWR",
            Self::SIGSYS => "SIGSYS",
        }
    }

    /// The signal named `s` (with its `SIG` prefix, any case).
    pub fn from_str(s: &str) -> Result<Self, error::Error> {
        let upper = s.to_ascii_uppercase();
        let name = match upper.as_str() {
            "SIGIOT" => "SIGABRT",
            "SIGPOLL" => "SIGIO",
            "SIGCLD" => "SIGCHLD",
            other => other,
        };
        ALL.into_iter()
            .find(|sig| sig.as_str() == name)
            .ok_or_else(|| error::ErrorKind::InvalidSignal(s.into()).into())
    }
}

impl TryFrom<i32> for Signal {
    type Error = error::Error;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        ALL.into_iter()
            .find(|sig| *sig as i32 == value)
            .ok_or_else(|| error::ErrorKind::InvalidSignal(value.to_string()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traps::TrapSignal;

    #[test]
    fn names_and_numbers() {
        assert_eq!("INT".parse::<TrapSignal>().unwrap(), TrapSignal::Signal(Signal::SIGINT));
        assert_eq!("sigterm".parse::<TrapSignal>().unwrap(), TrapSignal::Signal(Signal::SIGTERM));
        assert_eq!("15".parse::<TrapSignal>().unwrap(), TrapSignal::Signal(Signal::SIGTERM));
        assert_eq!("0".parse::<TrapSignal>().unwrap(), TrapSignal::Exit);
        assert_eq!(TrapSignal::Signal(Signal::SIGUSR1).to_string(), "SIGUSR1");
        assert!("NOPE".parse::<TrapSignal>().is_err());
        assert!("64".parse::<TrapSignal>().is_err());
        assert_eq!(Signal::iterator().count(), 31);
    }
}
