use std::fs;
use std::path::{Path, PathBuf};
use helios_shared::{log_debug, log_error, log_info, HeliosError, Result};

const CGROUP_ROOT: &str = "/sys/fs/cgroup/helios";

pub struct CgroupManager {
    id: String,
    path: PathBuf,
}

impl CgroupManager {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            path: Path::new(CGROUP_ROOT).join(id),
        }
    }

    /// Creates the cgroup directory structure for this container.
    pub fn create(&self) -> Result<()> {
        if !Path::new(CGROUP_ROOT).exists() {
            // Attempt to create root helios group. Requires root.
            if let Err(e) = fs::create_dir_all(CGROUP_ROOT) {
                return Err(HeliosError::ContainerError(format!(
                    "Failed to create helios cgroup root directory '{}': {}. Make sure you are running as root/sudo.",
                    CGROUP_ROOT, e
                )));
            }
        }

        if !self.path.exists() {
            if let Err(e) = fs::create_dir(&self.path) {
                return Err(HeliosError::ContainerError(format!(
                    "Failed to create cgroup sub-directory '{}': {}",
                    self.path.display(), e
                )));
            }
            log_debug!("container", "Cgroups: created cgroup v2 group: {}", self.path.display());
        }

        Ok(())
    }

    /// Binds a PID to this cgroup group.
    pub fn apply_limit(&self, pid: i32) -> Result<()> {
        let procs_file = self.path.join("cgroup.procs");
        if let Err(e) = fs::write(&procs_file, pid.to_string()) {
            return Err(HeliosError::ContainerError(format!(
                "Failed to attach process PID {} to cgroup.procs: {}",
                pid, e
            )));
        }
        log_debug!("container", "Cgroups: assigned process PID {} to cgroup", pid);
        Ok(())
    }

    /// Sets memory limit in bytes.
    pub fn set_memory_limit(&self, bytes: usize) -> Result<()> {
        let mem_file = self.path.join("memory.max");
        if let Err(e) = fs::write(&mem_file, bytes.to_string()) {
            return Err(HeliosError::ContainerError(format!(
                "Failed to set memory.max limit: {}", e
            )));
        }
        log_info!("container", "Cgroups: restricted memory capacity to {} bytes", bytes);
        Ok(())
    }

    /// Sets CPU limits in terms of percentage (e.g. 50 represents 50% of 1 core).
    pub fn set_cpu_limit(&self, percentage: usize) -> Result<()> {
        let cpu_file = self.path.join("cpu.max");
        // Format: <quota> <period>
        // Set standard period to 100,000 microseconds (100ms)
        let period = 100000;
        let quota = (period * percentage) / 100;
        
        let limit_str = format!("{} {}", quota, period);
        if let Err(e) = fs::write(&cpu_file, limit_str) {
            return Err(HeliosError::ContainerError(format!(
                "Failed to set cpu.max quota: {}", e
            )));
        }
        log_info!("container", "Cgroups: throttled CPU quota to {}% ({}us period)", percentage, period);
        Ok(())
    }

    /// Removes cgroup resources directory.
    pub fn destroy(&self) -> Result<()> {
        if self.path.exists() {
            if let Err(e) = fs::remove_dir(&self.path) {
                return Err(HeliosError::ContainerError(format!(
                    "Failed to delete cgroup directory '{}': {}",
                    self.path.display(), e
                )));
            }
            log_debug!("container", "Cgroups: destroyed cgroup group cleanly");
        }
        Ok(())
    }
}
