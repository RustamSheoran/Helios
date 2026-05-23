use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use helios_shared::{log_debug, log_error};

/// Initializes the signals for the parent shell.
/// Ignores SIGINT (Ctrl+C) and SIGTSTP (Ctrl+Z) so they don't terminate the shell itself.
pub unsafe fn init_shell_signals() {
    let action = SigAction::new(
        SigHandler::SigIgn,
        SaFlags::empty(),
        SigSet::empty(),
    );

    if let Err(e) = sigaction(Signal::SIGINT, &action) {
        log_error!("shell", "Failed to ignore SIGINT: {}", e);
    }

    if let Err(e) = sigaction(Signal::SIGTSTP, &action) {
        log_error!("shell", "Failed to ignore SIGTSTP: {}", e);
    }

    // Ignore SIGTTOU and SIGTTIN to allow background process group terminal redirection
    // without triggering terminal write/read block interrupts.
    if let Err(e) = sigaction(Signal::SIGTTOU, &action) {
        log_error!("shell", "Failed to ignore SIGTTOU: {}", e);
    }
    if let Err(e) = sigaction(Signal::SIGTTIN, &action) {
        log_error!("shell", "Failed to ignore SIGTTIN: {}", e);
    }
    
    log_debug!("shell", "Shell signals configured successfully");
}

/// Resets signal handlers in child processes back to standard defaults (SIG_DFL).
/// Must be called in the child process immediately after `fork` and before `execvp`.
pub unsafe fn reset_signals_in_child() {
    let default_action = SigAction::new(
        SigHandler::SigDfl,
        SaFlags::empty(),
        SigSet::empty(),
    );

    let _ = sigaction(Signal::SIGINT, &default_action);
    let _ = sigaction(Signal::SIGTSTP, &default_action);
    let _ = sigaction(Signal::SIGTTOU, &default_action);
    let _ = sigaction(Signal::SIGTTIN, &default_action);
}
