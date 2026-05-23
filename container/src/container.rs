use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use nix::unistd::{fork, ForkResult, Pid};
use nix::sys::wait::waitpid;
use nix::sys::signal::{kill, Signal};
use helios_shared::{log_debug, log_error, log_info, HeliosError, Result};
use std::os::fd::AsRawFd;
use crate::namespaces::{unshare_namespaces, set_container_hostname, configure_loopback_interface};
use crate::container_fs::prepare_isolated_rootfs;
use crate::cgroups::CgroupManager;
use crate::seccomp::install_seccomp_filter;

const STATE_DIR: &str = "/home/rustam/dist/helios/.helios";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerConfig {
    pub hostname: String,
    pub rootfs: String,
    pub command: Vec<String>,
    pub memory_limit_bytes: Option<usize>,
    pub cpu_limit_percentage: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerStatus {
    Creating,
    Running,
    Stopped,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerState {
    pub id: String,
    pub status: ContainerStatus,
    pub pid: Option<i32>,
    pub config: ContainerConfig,
    pub created_at: String,
}

pub struct Container {
    pub id: String,
    pub state_file: PathBuf,
}

impl Container {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            state_file: Path::new(STATE_DIR).join(format!("{}.json", id)),
        }
    }

    /// Primary execution flow: creates and starts the container environment.
    pub unsafe fn run(&self, config: ContainerConfig) -> Result<()> {
        log_info!("container", "Initializing lifecycle for container '{}'", self.id);

        // Ensure state tracking directory exists
        if !Path::new(STATE_DIR).exists() {
            let _ = fs::create_dir_all(STATE_DIR);
        }

        // 1. Establish the parent-child synchronization pipe
        let (sync_read, sync_write) = match nix::unistd::pipe() {
            Ok((r, w)) => (r, w),
            Err(_) => {
                let errno = nix::errno::Errno::last() as i32;
                return Err(HeliosError::SyscallError("pipe(sync)".to_string(), errno));
            }
        };

        // 2. Unshare namespaces (Mount, UTS, IPC, PID, Network)
        unshare_namespaces()?;

        // 3. Spawn child process
        log_debug!("container", "Forking child into isolated namespaces");
        match fork() {
            Ok(ForkResult::Parent { child }) => {
                // Inside parent: supervise setup
                drop(sync_write); // Close unused write end in parent via RAII drop

                log_debug!("container", "Parent: configuring cgroups and constraints");
                
                // Initialize cgroup configuration
                let cg = CgroupManager::new(&self.id);
                if let Err(e) = cg.create() {
                    let _ = kill(child, Signal::SIGKILL);
                    return Err(e);
                }

                if let Some(mem_bytes) = config.memory_limit_bytes {
                    if let Err(e) = cg.set_memory_limit(mem_bytes) {
                        let _ = kill(child, Signal::SIGKILL);
                        return Err(e);
                    }
                }
                if let Some(cpu_pct) = config.cpu_limit_percentage {
                    if let Err(e) = cg.set_cpu_limit(cpu_pct) {
                        let _ = kill(child, Signal::SIGKILL);
                        return Err(e);
                    }
                }

                // Attach child process to cgroups BEFORE unblocking it!
                if let Err(e) = cg.apply_limit(child.as_raw()) {
                    let _ = kill(child, Signal::SIGKILL);
                    return Err(e);
                }

                // Write container state file to workspace tracking cache
                let initial_state = ContainerState {
                    id: self.id.clone(),
                    status: ContainerStatus::Running,
                    pid: Some(child.as_raw()),
                    config: config.clone(),
                    created_at: format!("{:?}", std::time::SystemTime::now()),
                };
                self.save_state(&initial_state)?;

                // OCI Sync: notify child to proceed by writing a start signal byte
                log_debug!("container", "OCI Sync: Parent configured limits. Releasing child.");
                let start_signal = [1u8; 1];
                let _ = nix::unistd::write(&sync_read, &start_signal); // Write to pipe
                drop(sync_read); // Close read end

                // Block and supervise child process execution
                log_info!("container", "Supervising container PID {}...", child);
                match waitpid(child, None) {
                    Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => {
                        log_info!("container", "Container child exited with status code: {}", code);
                    }
                    Ok(status) => {
                        log_info!("container", "Container child changed state with status: {:?}", status);
                    }
                    Err(e) => {
                        log_error!("container", "Error waiting for container child: {}", e);
                    }
                }

                // Transition state to Stopped upon container process exit
                let mut current_state = self.load_state()?;
                current_state.status = ContainerStatus::Stopped;
                self.save_state(&current_state)?;
            }
            Ok(ForkResult::Child) => {
                // Inside child: complete isolation
                drop(sync_write); // Close unused write end in child via RAII drop

                // Block on reading a single byte from the parent sync pipe
                log_debug!("container", "OCI Sync: Child waiting for parent limit allocations...");
                let mut buf = [0u8; 1];
                let _ = nix::unistd::read(sync_read.as_raw_fd(), &mut buf);
                drop(sync_read); // Close read end

                log_debug!("container", "OCI Sync: Child unblocked. Finalizing isolation.");

                // A. Set isolated hostname
                set_container_hostname(&config.hostname)?;

                // B. Bring network loopback online
                configure_loopback_interface()?;

                // C. Confine directory space via pivot_root and configure mounts
                prepare_isolated_rootfs(&config.rootfs)?;

                // D. Apply sandboxed Seccomp packet filters
                install_seccomp_filter()?;

                // E. Execute target payload command
                let cmd_c = std::ffi::CString::new(config.command[0].as_str()).unwrap();
                let args_c: Vec<std::ffi::CString> = config.command.iter()
                    .map(|arg| std::ffi::CString::new(arg.as_str()).unwrap())
                    .collect();

                log_info!("container", "Executing container payload command: {:?}", config.command);
                let err = nix::unistd::execvp(&cmd_c, &args_c).unwrap_err();
                log_error!("container", "Failed to execvp inside container: {}", err);
                std::process::exit(127);
            }
            Err(_) => {
                let errno = nix::errno::Errno::last() as i32;
                return Err(HeliosError::SyscallError("fork".to_string(), errno));
            }
        }

        Ok(())
    }

    /// Stops a running container by sending SIGKILL to its process.
    pub fn stop(&self) -> Result<()> {
        let mut state = self.load_state()?;
        if state.status != ContainerStatus::Running {
            return Err(HeliosError::ContainerError(format!(
                "Cannot stop container '{}' - it is not running (status: {:?})", self.id, state.status
            )));
        }

        if let Some(pid) = state.pid {
            log_info!("container", "Sending SIGKILL to container process PID {}", pid);
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }

        state.status = ContainerStatus::Stopped;
        self.save_state(&state)?;
        log_info!("container", "Container '{}' stopped successfully", self.id);
        Ok(())
    }

    /// Cleans up container cgroups and purges tracking state.
    pub fn destroy(&self) -> Result<()> {
        let state = self.load_state()?;
        if state.status == ContainerStatus::Running {
            return Err(HeliosError::ContainerError(format!(
                "Cannot delete container '{}' - it is currently running. Stop it first.", self.id
            )));
        }

        log_info!("container", "Destroying container '{}' and removing resource limits", self.id);

        // Remove cgroup entries
        let cg = CgroupManager::new(&self.id);
        let _ = cg.destroy();

        // Delete JSON state file
        if self.state_file.exists() {
            let _ = fs::remove_file(&self.state_file);
        }

        log_info!("container", "Container '{}' destroyed cleanly", self.id);
        Ok(())
    }

    /// Returns a list of all active container states on the host.
    pub fn list() -> Result<Vec<ContainerState>> {
        let mut list = Vec::new();
        if !Path::new(STATE_DIR).exists() {
            return Ok(list);
        }

        let entries = fs::read_dir(STATE_DIR)
            .map_err(|e| HeliosError::ContainerError(format!("Failed to read state folder: {}", e)))?;

        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("json") {
                    let content = fs::read_to_string(&path)
                        .map_err(|e| HeliosError::ContainerError(format!("Failed to read file: {}", e)))?;
                    if let Ok(state) = serde_json::from_str::<ContainerState>(&content) {
                        list.push(state);
                    }
                }
            }
        }

        Ok(list)
    }

    pub fn save_state(&self, state: &ContainerState) -> Result<()> {
        let serialized = serde_json::to_string_pretty(state)
            .map_err(|e| HeliosError::ContainerError(format!("Failed to serialize state: {}", e)))?;
        fs::write(&self.state_file, serialized)
            .map_err(|e| HeliosError::ContainerError(format!("Failed to write state file: {}", e)))?;
        Ok(())
    }

    pub fn load_state(&self) -> Result<ContainerState> {
        if !self.state_file.exists() {
            return Err(HeliosError::ContainerError(format!("Container '{}' does not exist", self.id)));
        }
        let content = fs::read_to_string(&self.state_file)
            .map_err(|e| HeliosError::ContainerError(format!("Failed to read state file: {}", e)))?;
        let state: ContainerState = serde_json::from_str(&content)
            .map_err(|e| HeliosError::ContainerError(format!("Failed to parse state file: {}", e)))?;
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_serialization() {
        let config = ContainerConfig {
            hostname: "helios-test".to_string(),
            rootfs: "/rootfs".to_string(),
            command: vec!["/bin/sh".to_string(), "-c".to_string(), "echo".to_string()],
            memory_limit_bytes: Some(1024 * 1024 * 50), // 50MB
            cpu_limit_percentage: Some(50), // 50%
        };

        // Serialize to JSON string
        let json_str = serde_json::to_string(&config).unwrap();
        
        // Deserialize back and assert equality
        let decoded: ContainerConfig = serde_json::from_str(&json_str).unwrap();
        assert_eq!(decoded.hostname, "helios-test");
        assert_eq!(decoded.rootfs, "/rootfs");
        assert_eq!(decoded.command, vec!["/bin/sh", "-c", "echo"]);
        assert_eq!(decoded.memory_limit_bytes, Some(1024 * 1024 * 50));
        assert_eq!(decoded.cpu_limit_percentage, Some(50));
    }

    #[test]
    fn test_container_state_properties() {
        let config = ContainerConfig {
            hostname: "test-uts".to_string(),
            rootfs: "/tmp/rootfs".to_string(),
            command: vec!["ls".to_string()],
            memory_limit_bytes: None,
            cpu_limit_percentage: None,
        };

        let state = ContainerState {
            id: "test-c01".to_string(),
            status: ContainerStatus::Stopped,
            pid: Some(4221),
            config,
            created_at: "2026-05-23T08:50:00Z".to_string(),
        };

        assert_eq!(state.id, "test-c01");
        assert_eq!(state.status, ContainerStatus::Stopped);
        assert_eq!(state.pid, Some(4221));
        assert_eq!(state.created_at, "2026-05-23T08:50:00Z");
    }
}
