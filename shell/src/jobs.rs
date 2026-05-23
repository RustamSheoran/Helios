use std::collections::HashMap;
use nix::unistd::{Pid, tcsetpgrp};
use nix::sys::wait::WaitStatus;
use helios_shared::{log_debug, log_info, log_warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Stopped,
    Done,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: usize,
    pub pgid: Pid,
    pub pids: Vec<Pid>,
    pub cmd_line: String,
    pub state: JobState,
}

pub struct JobTable {
    jobs: HashMap<usize, Job>,
    next_id: usize,
    shell_pgid: Pid,
}

impl JobTable {
    pub fn new(shell_pgid: Pid) -> Self {
        Self {
            jobs: HashMap::new(),
            next_id: 1,
            shell_pgid,
        }
    }

    pub fn add_job(&mut self, pgid: Pid, pids: Vec<Pid>, cmd_line: String, state: JobState) -> usize {
        let id = self.next_id;
        self.jobs.insert(
            id,
            Job {
                id,
                pgid,
                pids,
                cmd_line,
                state,
            },
        );
        self.next_id += 1;
        log_debug!("shell", "Job control: added job [{}] with PGID {}", id, pgid);
        id
    }

    pub fn list_jobs(&self) {
        for job in self.jobs.values() {
            let state_str = match job.state {
                JobState::Running => "Running",
                JobState::Stopped => "Stopped",
                JobState::Done => "Done",
            };
            // Format: [1] Running  sleep 100 &
            println!("[{}] {:<10} {}", job.id, state_str, job.cmd_line);
        }
    }

    pub fn get_job(&self, id: usize) -> Option<&Job> {
        self.jobs.get(&id)
    }

    pub fn get_job_mut(&mut self, id: usize) -> Option<&mut Job> {
        self.jobs.get_mut(&id)
    }

    pub fn find_by_pgid(&self, pgid: Pid) -> Option<&Job> {
        self.jobs.values().find(|j| j.pgid == pgid)
    }

    pub fn find_by_pid(&self, pid: Pid) -> Option<&Job> {
        self.jobs.values().find(|j| j.pids.contains(&pid))
    }

    pub fn remove_job(&mut self, id: usize) {
        if self.jobs.remove(&id).is_some() {
            log_debug!("shell", "Job control: removed job [{}]", id);
        }
    }

    pub fn clean_completed_jobs(&mut self) {
        let mut completed_ids = Vec::new();
        for job in self.jobs.values() {
            if job.state == JobState::Done {
                completed_ids.push(job.id);
            }
        }
        for id in completed_ids {
            println!("[{}] Done completed job", id);
            self.remove_job(id);
        }
    }

    /// Shift a job to the foreground, grab terminal control, and wait.
    pub unsafe fn shift_to_foreground(&mut self, id: usize) {
        let (pgid, cmd_line) = {
            let job = match self.get_job_mut(id) {
                Some(j) => j,
                None => {
                    println!("helios: fg: no such job [{}]", id);
                    return;
                }
            };
            job.state = JobState::Running;
            (job.pgid, job.cmd_line.clone())
        };

        println!("{}", cmd_line);

        // 1. Give controlling terminal focus to child process group
        if let Err(e) = tcsetpgrp(std::io::stdin(), pgid) {
            log_warn!("shell", "Failed to transfer terminal control to child PGID: {}", e);
        }

        // 2. Send SIGCONT in case the job was stopped
        let _ = nix::sys::signal::kill(pgid, nix::sys::signal::Signal::SIGCONT);

        // 3. Block and wait for foreground job group to exit or stop
        self.wait_for_job(id);

        // 4. Re-claim controlling terminal focus for the shell
        if let Err(e) = tcsetpgrp(std::io::stdin(), self.shell_pgid) {
            log_warn!("shell", "Failed to reclaim terminal control for shell: {}", e);
        }
    }

    /// Shift a stopped job to run in the background.
    pub unsafe fn shift_to_background(&mut self, id: usize) {
        let pgid = {
            let job = match self.get_job_mut(id) {
                Some(j) => j,
                None => {
                    println!("helios: bg: no such job [{}]", id);
                    return;
                }
            };
            job.state = JobState::Running;
            job.pgid
        };

        // Send SIGCONT to trigger background running
        let _ = nix::sys::signal::kill(pgid, nix::sys::signal::Signal::SIGCONT);
        println!("[{}] running in background &", id);
    }

    /// Monitor and block wait for a foreground job.
    pub unsafe fn wait_for_job(&mut self, id: usize) {
        let pids = {
            let job = match self.get_job(id) {
                Some(j) => j,
                None => return,
            };
            job.pids.clone()
        };

        let mut active_count = pids.len();

        while active_count > 0 {
            // WUNTRACED retrieves stopped child states, WCONTINUED retrieves continued ones
            let flags = nix::sys::wait::WaitPidFlag::WUNTRACED | nix::sys::wait::WaitPidFlag::WCONTINUED;
            
            // Wait on any child in our process session
            match nix::sys::wait::waitpid(Pid::from_raw(-1), Some(flags)) {
                Ok(status) => {
                    match status {
                        WaitStatus::Exited(pid, code) => {
                            log_debug!("shell", "Child PID {} exited with code {}", pid, code);
                            if pids.contains(&pid) {
                                active_count -= 1;
                            }
                        }
                        WaitStatus::Signaled(pid, sig, core_dump) => {
                            log_debug!("shell", "Child PID {} killed by signal {:?}, core_dump={}", pid, sig, core_dump);
                            if pids.contains(&pid) {
                                active_count -= 1;
                            }
                        }
                        WaitStatus::Stopped(pid, sig) => {
                            log_info!("shell", "Job [{}] (PID {}) stopped by signal {:?}", id, pid, sig);
                            if let Some(job) = self.get_job_mut(id) {
                                job.state = JobState::Stopped;
                            }
                            return; // Foreground job stopped, return shell control
                        }
                        WaitStatus::Continued(pid) => {
                            log_debug!("shell", "Child PID {} continued", pid);
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    // ECHILD means no more children left in this group
                    if e == nix::errno::Errno::ECHILD {
                        break;
                    }
                    log_warn!("shell", "waitpid returned error: {}", e);
                    break;
                }
            }
        }

        // Foreground job fully terminated
        self.remove_job(id);
    }

    /// Asynchronous reaper to check on background processes (called during REPL turns or via SIGCHLD).
    pub unsafe fn check_background_jobs(&mut self) {
        let flags = nix::sys::wait::WaitPidFlag::WNOHANG | nix::sys::wait::WaitPidFlag::WUNTRACED;
        
        loop {
            match nix::sys::wait::waitpid(Pid::from_raw(-1), Some(flags)) {
                Ok(WaitStatus::Exited(pid, code)) => {
                    if let Some(job) = self.find_by_pid(pid) {
                        let job_id = job.id;
                        log_info!("shell", "Background job [{}] (PID {}) finished with exit code {}", job_id, pid, code);
                        if let Some(j) = self.get_job_mut(job_id) {
                            j.state = JobState::Done;
                        }
                    }
                }
                Ok(WaitStatus::Signaled(pid, sig, _)) => {
                    if let Some(job) = self.find_by_pid(pid) {
                        let job_id = job.id;
                        log_info!("shell", "Background job [{}] (PID {}) killed by signal {:?}", job_id, pid, sig);
                        if let Some(j) = self.get_job_mut(job_id) {
                            j.state = JobState::Done;
                        }
                    }
                }
                Ok(WaitStatus::Stopped(pid, sig)) => {
                    if let Some(job) = self.find_by_pid(pid) {
                        let job_id = job.id;
                        log_info!("shell", "Background job [{}] (PID {}) stopped by signal {:?}", job_id, pid, sig);
                        if let Some(j) = self.get_job_mut(job_id) {
                            j.state = JobState::Stopped;
                        }
                    }
                }
                Ok(WaitStatus::StillAlive) | Ok(_) => break, // WNOHANG returned no status, or unrelated event
                Err(e) => {
                    if e != nix::errno::Errno::ECHILD {
                        // Suppress log noise if no children
                    }
                    break;
                }
            }
        }
    }
}
