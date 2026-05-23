use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeliosError {
    SyscallError(String, i32),
    ParserError(String),
    ShellError(String),
    ContainerError(String),
    AllocatorError(String),
    InvalidConfig(String),
}

impl std::error::Error for HeliosError {}

impl fmt::Display for HeliosError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeliosError::SyscallError(op, code) => {
                write!(f, "Syscall error during '{}': errno = {} ({})", op, code, get_errno_string(*code))
            }
            HeliosError::ParserError(msg) => write!(f, "Parser error: {}", msg),
            HeliosError::ShellError(msg) => write!(f, "Shell error: {}", msg),
            HeliosError::ContainerError(msg) => write!(f, "Container error: {}", msg),
            HeliosError::AllocatorError(msg) => write!(f, "Allocator error: {}", msg),
            HeliosError::InvalidConfig(msg) => write!(f, "Invalid configuration: {}", msg),
        }
    }
}

pub type Result<T> = std::result::Result<T, HeliosError>;

/// Get human-readable representation of errno.
pub fn get_errno_string(errno: i32) -> String {
    unsafe {
        let err_ptr = libc::strerror(errno);
        if !err_ptr.is_null() {
            std::ffi::CStr::from_ptr(err_ptr)
                .to_string_lossy()
                .into_owned()
        } else {
            format!("Unknown error {}", errno)
        }
    }
}

/// An allocator-safe logging facility that directly invokes `libc::write`
/// to output to standard error. This prevents dynamic allocation loop/deadlock
/// if called from inside the Custom Memory Allocator.
pub struct RawStderrLogger;

impl RawStderrLogger {
    pub fn write_raw(msg: &str) {
        unsafe {
            let bytes = msg.as_bytes();
            let mut written = 0;
            while written < bytes.len() {
                let res = libc::write(
                    libc::STDERR_FILENO,
                    bytes.as_ptr().add(written) as *const libc::c_void,
                    bytes.len() - written,
                );
                if res < 0 {
                    let err = *libc::__errno_location();
                    if err == libc::EINTR {
                        continue;
                    }
                    break; // Unrecoverable write failure
                }
                written += res as usize;
            }
        }
    }

    pub fn log(level: &str, color_code: &str, module: &str, msg: &str) {
        // Build the log message manually.
        // We avoid dynamic allocations by writing segments sequentially!
        // Format: \x1b[COLORm[HELIOS LEVEL] (module)\x1b[0m msg\n
        Self::write_raw(color_code);
        Self::write_raw("[HELIOS ");
        Self::write_raw(level);
        Self::write_raw("] (");
        Self::write_raw(module);
        Self::write_raw(") ");
        Self::write_raw("\x1b[0m");
        Self::write_raw(msg);
        Self::write_raw("\n");
    }
}

#[macro_export]
macro_rules! log_info {
    ($module:expr, $msg:expr) => {
        $crate::RawStderrLogger::log("INFO", "\x1b[32m", $module, $msg);
    };
    ($module:expr, $fmt:expr, $($arg:tt)*) => {
        // Fallback for rich formatting outside allocation-sensitive zones
        let formatted = format!($fmt, $($arg)*);
        $crate::RawStderrLogger::log("INFO", "\x1b[32m", $module, &formatted);
    };
}

#[macro_export]
macro_rules! log_warn {
    ($module:expr, $msg:expr) => {
        $crate::RawStderrLogger::log("WARN", "\x1b[33m", $module, $msg);
    };
    ($module:expr, $fmt:expr, $($arg:tt)*) => {
        let formatted = format!($fmt, $($arg)*);
        $crate::RawStderrLogger::log("WARN", "\x1b[33m", $module, &formatted);
    };
}

#[macro_export]
macro_rules! log_error {
    ($module:expr, $msg:expr) => {
        $crate::RawStderrLogger::log("ERROR", "\x1b[31m", $module, $msg);
    };
    ($module:expr, $fmt:expr, $($arg:tt)*) => {
        let formatted = format!($fmt, $($arg)*);
        $crate::RawStderrLogger::log("ERROR", "\x1b[31m", $module, &formatted);
    };
}

#[macro_export]
macro_rules! log_debug {
    ($module:expr, $msg:expr) => {
        #[cfg(debug_assertions)]
        $crate::RawStderrLogger::log("DEBUG", "\x1b[36m", $module, $msg);
    };
    ($module:expr, $fmt:expr, $($arg:tt)*) => {
        #[cfg(debug_assertions)]
        {
            let formatted = format!($fmt, $($arg)*);
            $crate::RawStderrLogger::log("DEBUG", "\x1b[36m", $module, &formatted);
        }
    };
}
