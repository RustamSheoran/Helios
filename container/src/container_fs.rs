use std::fs;
use std::path::Path;
use nix::mount::{mount, umount2, MntFlags, MsFlags};
use helios_shared::{log_debug, log_error, log_info, HeliosError, Result};

/// Isolates the filesystem of the container by configuring mount propagation,
/// executing pivot_root, and mounting essential virtual filesystems.
pub unsafe fn prepare_isolated_rootfs(rootfs: &str) -> Result<()> {
    let new_root = Path::new(rootfs);

    // 1. Set mount propagation on "/" to MS_PRIVATE recursively.
    // This isolates our container mounts so they don't leak back to the host!
    log_debug!("container", "Mounts: setting root mount propagation to private");
    if let Err(_) = mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        None::<&str>,
    ) {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError("mount(/, MS_PRIVATE)".to_string(), errno));
    }

    // 2. pivot_root requires that the new root is a mount point.
    // We make it a mount point by bind mounting the directory onto itself!
    log_debug!("container", "Mounts: bind mounting rootfs directory onto itself");
    if let Err(_) = mount(
        Some(new_root),
        new_root,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    ) {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError("mount(bind new_root)".to_string(), errno));
    }

    // 3. Create the directory for the old root under the new rootfs
    let pivot_dir_name = ".pivot_old";
    let pivot_dir = new_root.join(pivot_dir_name);
    if !pivot_dir.exists() {
        if let Err(e) = fs::create_dir_all(&pivot_dir) {
            return Err(HeliosError::ContainerError(format!(
                "Failed to create pivot_old folder: {}", e
            )));
        }
    }

    // 4. Pivot root!
    log_debug!("container", "Mounts: executing pivot_root syscall");
    let new_root_c = std::ffi::CString::new(new_root.to_str().unwrap()).unwrap();
    let pivot_dir_c = std::ffi::CString::new(pivot_dir.to_str().unwrap()).unwrap();
    
    if libc::syscall(libc::SYS_pivot_root, new_root_c.as_ptr(), pivot_dir_c.as_ptr()) < 0 {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError("pivot_root".to_string(), errno));
    }

    // 5. Change current working directory to the new root
    log_debug!("container", "Mounts: chdir to new '/' root");
    if libc::chdir(b"/\0".as_ptr() as *const libc::c_char) < 0 {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError("chdir(/)".to_string(), errno));
    }

    // 6. Mount virtual kernel filesystems inside the container
    mount_virtual_filesystems()?;

    // 7. Unmount the old root filesystem
    log_debug!("container", "Mounts: unmounting old root filesystem");
    let old_root_path = format!("/{}", pivot_dir_name);
    if let Err(_) = umount2(old_root_path.as_str(), MntFlags::MNT_DETACH) {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError("umount2(old_root)".to_string(), errno));
    }

    // 8. Delete the temporary old root directory
    if let Err(e) = fs::remove_dir(old_root_path.as_str()) {
        log_error!("container", "Failed to remove old root directory mountpoint: {}", e);
    }

    log_info!("container", "Filesystem: successfully jailed container inside rootfs");
    Ok(())
}

/// Helper function to mount procfs, sysfs, and standard devices in the new root.
unsafe fn mount_virtual_filesystems() -> Result<()> {
    // A. Mount /proc
    log_debug!("container", "Mounts: mounting procfs on /proc");
    let proc_path = Path::new("/proc");
    if !proc_path.exists() {
        let _ = fs::create_dir_all(proc_path);
    }
    if let Err(_) = mount(
        Some("proc"),
        proc_path,
        Some("proc"),
        MsFlags::empty(),
        None::<&str>,
    ) {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError("mount(/proc)".to_string(), errno));
    }

    // B. Mount /sys
    log_debug!("container", "Mounts: mounting sysfs on /sys");
    let sys_path = Path::new("/sys");
    if !sys_path.exists() {
        let _ = fs::create_dir_all(sys_path);
    }
    if let Err(_) = mount(
        Some("sysfs"),
        sys_path,
        Some("sysfs"),
        MsFlags::empty(),
        None::<&str>,
    ) {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError("mount(/sys)".to_string(), errno));
    }

    // C. Mount /dev (tmpfs for isolated devices)
    log_debug!("container", "Mounts: mounting tmpfs on /dev");
    let dev_path = Path::new("/dev");
    if !dev_path.exists() {
        let _ = fs::create_dir_all(dev_path);
    }
    if let Err(_) = mount(
        Some("tmpfs"),
        dev_path,
        Some("tmpfs"),
        MsFlags::empty(),
        None::<&str>,
    ) {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError("mount(/dev)".to_string(), errno));
    }

    // D. Bind-mount standard device nodes from the host to ensure basic shell utilities work
    let devices = ["null", "zero", "urandom", "random", "tty"];
    for dev in &devices {
        let target = format!("/dev/{}", dev);
        let host_source = format!("/dev/{}", dev);
        
        let target_path = Path::new(&target);
        if !target_path.exists() {
            // Touch dummy file so it can be bind-mounted
            if let Err(e) = fs::write(target_path, "") {
                log_error!("container", "Failed to prepare dummy dev file '{}': {}", target, e);
                continue;
            }
        }

        if let Err(_) = mount(
            Some(host_source.as_str()),
            target_path,
            None::<&str>,
            MsFlags::MS_BIND,
            None::<&str>,
        ) {
            let errno = nix::errno::Errno::last() as i32;
            log_error!("container", "Failed to bind-mount /dev/{} device (errno={})", dev, errno);
        }
    }

    Ok(())
}
