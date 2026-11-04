//! sill: `uname` as Git Bash answers it. uutils said `Windows_NT`, which matched none of
//! the `MINGW*|MSYS*|CYGWIN*` branches scripts use to spot Windows, while `$OSTYPE` is
//! `msys` as in Git Bash.

use clap::Parser;
use std::io::Write;

use brush_core::{ExecutionResult, builtins};

/// Print system information.
#[derive(Parser)]
#[command(disable_help_flag = true, disable_version_flag = true)]
pub(crate) struct UnameCommand {
    /// All of the below.
    #[arg(short = 'a', long = "all")]
    all: bool,
    /// The kernel name (the default).
    #[arg(short = 's', long = "kernel-name")]
    kernel_name: bool,
    /// The host name.
    #[arg(short = 'n', long = "nodename")]
    nodename: bool,
    /// The kernel release.
    #[arg(short = 'r', long = "kernel-release")]
    kernel_release: bool,
    /// The kernel version.
    #[arg(short = 'v', long = "kernel-version")]
    kernel_version: bool,
    /// The machine.
    #[arg(short = 'm', long = "machine")]
    machine: bool,
    /// The processor.
    #[arg(short = 'p', long = "processor")]
    processor: bool,
    /// The hardware platform.
    #[arg(short = 'i', long = "hardware-platform")]
    hardware_platform: bool,
    /// The operating system.
    #[arg(short = 'o', long = "operating-system")]
    operating_system: bool,
}

brush_clap_adapter::impl_parse_args!(UnameCommand);

/// Windows' version, as `10.0.26200`.
fn windows_version() -> String {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let key = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        let dword = |name: &str| {
            let (mut v, mut n) = (0u32, 4u32);
            let name = wide(name);
            // SAFETY: a DWORD read into a buffer of its size.
            let ok = unsafe {
                RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&raw mut v).cast(), &mut n)
            } == 0;
            ok.then_some(v)
        };
        let build = {
            let mut buf = [0u16; 32];
            let mut n = u32::try_from(std::mem::size_of_val(&buf)).unwrap_or(0);
            let name = wide("CurrentBuildNumber");
            // SAFETY: a string read into a buffer whose size is passed.
            let ok = unsafe {
                RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut n)
            } == 0;
            if ok { String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]) } else { "0".into() }
        };
        format!("{}.{}.{build}", dword("CurrentMajorVersionNumber").unwrap_or(10), dword("CurrentMinorVersionNumber").unwrap_or(0))
    }
    #[cfg(not(windows))]
    {
        "10.0.0".into()
    }
}

/// The fields `uname` prints, in its order: kernel name, host, release, version,
/// machine, processor, platform, OS.
fn fields(version: &str, host: &str) -> [String; 8] {
    let machine = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    let (major_minor, build) = version.rsplit_once('.').unwrap_or((version, "0"));
    [
        format!("MINGW64_NT-{major_minor}-{build}"),
        host.to_owned(),
        version.to_owned(),
        "sill".to_owned(),
        machine.to_owned(),
        "unknown".to_owned(),
        "unknown".to_owned(),
        "Msys".to_owned(),
    ]
}

impl<SE: brush_core::ShellExtensions> builtins::Builtin<SE> for UnameCommand {
    type Error = brush_core::Error;

    async fn execute(
        self,
        context: brush_core::ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".into());
        let f = fields(&windows_version(), &host);
        let wanted = [
            self.kernel_name,
            self.nodename,
            self.kernel_release,
            self.kernel_version,
            self.machine,
            self.processor,
            self.hardware_platform,
            self.operating_system,
        ];
        let out: Vec<&str> = if self.all {
            // As GNU's -a: the processor and platform only when known.
            f.iter().enumerate().filter(|(i, _)| *i != 5 && *i != 6).map(|(_, s)| s.as_str()).collect()
        } else if wanted.iter().any(|w| *w) {
            f.iter().zip(wanted).filter(|(_, w)| *w).map(|(s, _)| s.as_str()).collect()
        } else {
            vec![f[0].as_str()]
        };
        writeln!(context.stdout(), "{}", out.join(" "))?;
        Ok(ExecutionResult::success())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn as_git_bash_says_it() {
        let f = super::fields("10.0.26200", "HOST");
        assert_eq!(f[0], "MINGW64_NT-10.0-26200");
        assert_eq!(f[7], "Msys");
    }
}
