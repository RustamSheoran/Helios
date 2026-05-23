use std::ffi::CString;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use nix::unistd::{fork, ForkResult, Pid, tcsetpgrp};
use nix::sys::wait::WaitStatus;
use helios_shared::{log_error, log_info};
use crate::parser::{Command, SimpleCommand, RedirectOp};
use crate::jobs::{JobTable, JobState};
use crate::signals::reset_signals_in_child;

/// Struct holding shell session states like current directory, aliases, and environment variables.
pub struct ShellState {
    pub current_dir: std::path::PathBuf,
    pub aliases: std::collections::HashMap<String, String>,
    pub history: Vec<String>,
}

impl ShellState {
    pub fn new() -> Self {
        Self {
            current_dir: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")),
            aliases: std::collections::HashMap::new(),
            history: Vec::new(),
        }
    }
}

/// Executes a shell command AST recursively.
pub unsafe fn execute_command(
    cmd: &Command,
    state: &mut ShellState,
    job_table: &mut JobTable,
    is_foreground: bool,
    cmd_str: &str,
) -> i32 {
    match cmd {
        Command::Simple(simple) => {
            if simple.args.is_empty() {
                return 0;
            }

            // 1. Check and execute Shell Built-in command in parent context
            if is_builtin(&simple.args[0]) {
                return execute_builtin(simple, state, job_table);
            }

            // 2. Otherwise, execute standard external binary
            execute_external_simple(simple, job_table, is_foreground, cmd_str)
        }
        Command::Redirect(inner, op) => {
            // Apply redirection in the parent if it's a built-in (temporary fd swap),
            // or fork and apply in child. To keep it clean, we fork a process to isolate
            // redirects for both simple commands and nested pipelines.
            execute_redirect(inner, op, state, job_table, is_foreground, cmd_str)
        }
        Command::Pipeline(pipeline_cmds) => {
            execute_pipeline(pipeline_cmds, state, job_table, is_foreground, cmd_str)
        }
        Command::Background(inner) => {
            execute_command(inner, state, job_table, false, cmd_str)
        }
        Command::Subshell(inner) => {
            execute_subshell(inner, state, job_table)
        }
        Command::Sequence(cmds) => {
            let mut status = 0;
            for c in cmds {
                status = execute_command(c, state, job_table, is_foreground, cmd_str);
            }
            status
        }
    }
}

fn is_builtin(cmd_name: &str) -> bool {
    matches!(cmd_name, "cd" | "exit" | "jobs" | "fg" | "bg" | "alias" | "unalias" | "export" | "env" | "history")
}

unsafe fn execute_builtin(
    simple: &SimpleCommand,
    state: &mut ShellState,
    job_table: &mut JobTable,
) -> i32 {
    let name = &simple.args[0];
    match name.as_str() {
        "cd" => {
            let target = if simple.args.len() > 1 {
                &simple.args[1]
            } else {
                "/"
            };
            if let Err(e) = std::env::set_current_dir(target) {
                println!("helios: cd: {}: {}", target, e);
                1
            } else {
                state.current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(target));
                0
            }
        }
        "exit" => {
            log_info!("shell", "Helios Shell exiting cleanly");
            std::process::exit(0);
        }
        "jobs" => {
            job_table.list_jobs();
            0
        }
        "fg" => {
            if simple.args.len() < 2 {
                println!("helios: fg: expected job ID");
                return 1;
            }
            if let Ok(id) = simple.args[1].parse::<usize>() {
                job_table.shift_to_foreground(id);
            } else {
                println!("helios: fg: invalid job ID");
            }
            0
        }
        "bg" => {
            if simple.args.len() < 2 {
                println!("helios: bg: expected job ID");
                return 1;
            }
            if let Ok(id) = simple.args[1].parse::<usize>() {
                job_table.shift_to_background(id);
            } else {
                println!("helios: bg: invalid job ID");
            }
            0
        }
        "alias" => {
            if simple.args.len() < 2 {
                for (k, v) in &state.aliases {
                    println!("alias {}='{}'", k, v);
                }
                return 0;
            }
            let binding = simple.args[1..].join(" ");
            if let Some(pos) = binding.find('=') {
                let key = binding[..pos].trim().to_string();
                let val = binding[pos + 1..].trim().trim_matches('\'').trim_matches('"').to_string();
                state.aliases.insert(key, val);
                0
            } else {
                println!("alias: usage: alias name=value");
                1
            }
        }
        "unalias" => {
            if simple.args.len() < 2 {
                println!("unalias: expected alias name");
                return 1;
            }
            state.aliases.remove(&simple.args[1]);
            0
        }
        "export" => {
            if simple.args.len() < 2 {
                for (k, v) in std::env::vars() {
                    println!("export {}={}", k, v);
                }
                return 0;
            }
            let binding = &simple.args[1];
            if let Some(pos) = binding.find('=') {
                let key = &binding[..pos];
                let val = &binding[pos + 1..];
                std::env::set_var(key, val);
                0
            } else {
                println!("export: usage: export NAME=VALUE");
                1
            }
        }
        "env" => {
            for (k, v) in std::env::vars() {
                println!("{}={}", k, v);
            }
            0
        }
        "history" => {
            for (idx, line) in state.history.iter().enumerate() {
                println!("  {:>3}  {}", idx + 1, line);
            }
            0
        }
        _ => 1,
    }
}

unsafe fn execute_external_simple(
    simple: &SimpleCommand,
    job_table: &mut JobTable,
    is_foreground: bool,
    cmd_str: &str,
) -> i32 {
    match fork() {
        Ok(ForkResult::Parent { child }) => {
            // Parent context
            // Synchronously establish the child process group ID in both parent and child
            // to eliminate scheduling race conditions before tcsetpgrp terminal switches!
            let _ = nix::unistd::setpgid(child, child);

            if is_foreground {
                // Add to job table as running
                let job_id = job_table.add_job(child, vec![child], cmd_str.to_string(), JobState::Running);
                
                // Set foreground terminal process group
                let _ = tcsetpgrp(std::io::stdin(), child);
                
                // Wait for foreground job group to exit or stop
                job_table.wait_for_job(job_id);
                
                // Reclaim terminal focus
                let _ = tcsetpgrp(std::io::stdin(), nix::unistd::getpgrp());
                0
            } else {
                // Background job execution
                let job_id = job_table.add_job(child, vec![child], cmd_str.to_string(), JobState::Running);
                println!("[{}] {}", job_id, child);
                0
            }
        }
        Ok(ForkResult::Child) => {
            // Child context
            let _ = nix::unistd::setpgid(Pid::from_raw(0), Pid::from_raw(0));
            reset_signals_in_child();

            let cmd_c = CString::new(simple.args[0].as_str()).unwrap();
            let args_c: Vec<CString> = simple
                .args
                .iter()
                .map(|arg| CString::new(arg.as_str()).unwrap())
                .collect();

            let err = nix::unistd::execvp(&cmd_c, &args_c).unwrap_err();
            eprintln!("helios: {}: {}", simple.args[0], err);
            std::process::exit(127);
        }
        Err(e) => {
            log_error!("shell", "Failed to fork child process: {}", e);
            1
        }
    }
}

unsafe fn execute_redirect(
    inner: &Command,
    op: &RedirectOp,
    state: &mut ShellState,
    job_table: &mut JobTable,
    is_foreground: bool,
    cmd_str: &str,
) -> i32 {
    match fork() {
        Ok(ForkResult::Parent { child }) => {
            let _ = nix::unistd::setpgid(child, child);
            if is_foreground {
                let job_id = job_table.add_job(child, vec![child], cmd_str.to_string(), JobState::Running);
                let _ = tcsetpgrp(std::io::stdin(), child);
                job_table.wait_for_job(job_id);
                let _ = tcsetpgrp(std::io::stdin(), nix::unistd::getpgrp());
                0
            } else {
                let job_id = job_table.add_job(child, vec![child], cmd_str.to_string(), JobState::Running);
                println!("[{}] {}", job_id, child);
                0
            }
        }
        Ok(ForkResult::Child) => {
            let _ = nix::unistd::setpgid(Pid::from_raw(0), Pid::from_raw(0));
            reset_signals_in_child();

            // Apply file redirection
            match op {
                RedirectOp::Input(file) => {
                    if let Ok(f) = File::open(file) {
                        let fd = f.as_raw_fd();
                        let _ = nix::unistd::dup2(fd, libc::STDIN_FILENO);
                    } else {
                        eprintln!("helios: {}: No such file or directory", file);
                        std::process::exit(1);
                    }
                }
                RedirectOp::Output(file) => {
                    if let Ok(f) = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(file)
                    {
                        let fd = f.as_raw_fd();
                        let _ = nix::unistd::dup2(fd, libc::STDOUT_FILENO);
                    } else {
                        eprintln!("helios: {}: Failed to create file", file);
                        std::process::exit(1);
                    }
                }
                RedirectOp::Append(file) => {
                    if let Ok(f) = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .append(true)
                        .open(file)
                    {
                        let fd = f.as_raw_fd();
                        let _ = nix::unistd::dup2(fd, libc::STDOUT_FILENO);
                    } else {
                        eprintln!("helios: {}: Failed to append file", file);
                        std::process::exit(1);
                    }
                }
                RedirectOp::Error(file) => {
                    if let Ok(f) = std::fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(file)
                    {
                        let fd = f.as_raw_fd();
                        let _ = nix::unistd::dup2(fd, libc::STDERR_FILENO);
                    } else {
                        eprintln!("helios: {}: Failed to open error log", file);
                        std::process::exit(1);
                    }
                }
            }

            // Execute inner Command in child context directly
            // We set foreground to true because the child parent is waiting,
            // and we set the inner command to simple execution.
            execute_command(inner, state, job_table, true, cmd_str);
            std::process::exit(0);
        }
        Err(e) => {
            log_error!("shell", "Redirect fork failed: {}", e);
            1
        }
    }
}

unsafe fn execute_pipeline(
    cmds: &[Command],
    state: &mut ShellState,
    job_table: &mut JobTable,
    is_foreground: bool,
    cmd_str: &str,
) -> i32 {
    let num_cmds = cmds.len();
    let mut pipes = Vec::new();

    // Create N-1 pipes
    for _ in 0..num_cmds - 1 {
        match nix::unistd::pipe() {
            Ok((r, w)) => pipes.push((r, w)),
            Err(e) => {
                log_error!("shell", "Failed to create OS pipe: {}", e);
                return 1;
            }
        }
    }

    let mut children_pids = Vec::new();
    let mut pgid: Option<Pid> = None;

    for (i, cmd) in cmds.iter().enumerate() {
        match fork() {
            Ok(ForkResult::Parent { child }) => {
                // Initialize process group leader based on the first process in pipeline
                if pgid.is_none() {
                    pgid = Some(child);
                }
                let _ = nix::unistd::setpgid(child, pgid.unwrap());
                children_pids.push(child);
            }
            Ok(ForkResult::Child) => {
                // Child process
                let child_pid = nix::unistd::getpid();
                let leader_pgid = pgid.unwrap_or(child_pid);
                let _ = nix::unistd::setpgid(Pid::from_raw(0), leader_pgid);
                reset_signals_in_child();

                // Wire up input pipe if not the first command
                if i > 0 {
                    let _ = nix::unistd::dup2(pipes[i - 1].0.as_raw_fd(), libc::STDIN_FILENO);
                }

                // Wire up output pipe if not the last command
                if i < num_cmds - 1 {
                    let _ = nix::unistd::dup2(pipes[i].1.as_raw_fd(), libc::STDOUT_FILENO);
                }

                // Crucial: close all pipe ends in child via safe RAII drop!
                drop(pipes);

                // Run inner command
                // Since this runs in a subshell-like isolated child, we execute simple command inside
                execute_command(cmd, state, job_table, true, cmd_str);
                std::process::exit(0);
            }
            Err(e) => {
                log_error!("shell", "Pipeline fork failed: {}", e);
                return 1;
            }
        }
    }

    // Crucial: close all pipe ends in the shell parent via safe RAII drop!
    drop(pipes);

    let pipeline_leader = children_pids[0];

    if is_foreground {
        // Track entire pipeline PIDs in job table
        let job_id = job_table.add_job(pipeline_leader, children_pids, cmd_str.to_string(), JobState::Running);
        
        // Hand terminal control to pipeline process group
        let _ = tcsetpgrp(std::io::stdin(), pipeline_leader);
        
        // Wait for pipeline processes to stop/exit
        job_table.wait_for_job(job_id);
        
        // Reclaim terminal
        let _ = tcsetpgrp(std::io::stdin(), nix::unistd::getpgrp());
        0
    } else {
        // Background pipeline execution
        let job_id = job_table.add_job(pipeline_leader, children_pids, cmd_str.to_string(), JobState::Running);
        println!("[{}] {}", job_id, pipeline_leader);
        0
    }
}

unsafe fn execute_subshell(
    inner: &Command,
    state: &mut ShellState,
    job_table: &mut JobTable,
) -> i32 {
    match fork() {
        Ok(ForkResult::Parent { child }) => {
            let _ = nix::unistd::setpgid(child, child);
            let mut status = 0;
            // Wait for the subshell process group to finish
            match nix::sys::wait::waitpid(child, None) {
                Ok(WaitStatus::Exited(_, code)) => status = code,
                Ok(WaitStatus::Signaled(_, sig, _)) => status = 128 + sig as i32,
                _ => {}
            }
            status
        }
        Ok(ForkResult::Child) => {
            let _ = nix::unistd::setpgid(Pid::from_raw(0), Pid::from_raw(0));
            reset_signals_in_child();
            
            // Execute inner sequence command in this isolated child process
            let status = execute_command(inner, state, job_table, true, "");
            std::process::exit(status);
        }
        Err(e) => {
            log_error!("shell", "Subshell fork failed: {}", e);
            1
        }
    }
}
