use nix::sched::{unshare, CloneFlags};
use helios_shared::{log_debug, log_info, HeliosError, Result};

/// Detaches the process's namespaces from the host.
/// Isolates Mount (NEWNS), UTS (hostname), IPC, PID, and Network.
pub fn unshare_namespaces() -> Result<()> {
    log_debug!("container", "Namespaces: disassociating from host namespaces");

    // Unified flag set for full isolation.
    let flags = CloneFlags::CLONE_NEWNS  | // Private Mount Namespace
                CloneFlags::CLONE_NEWUTS | // Private Hostname Namespace
                CloneFlags::CLONE_NEWIPC | // Private Inter-Process Communication
                CloneFlags::CLONE_NEWPID | // Next spawned child is PID 1
                CloneFlags::CLONE_NEWNET;  // Private Network Stack

    if let Err(_) = unshare(flags) {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError(format!("unshare({:?})", flags), errno));
    }

    log_debug!("container", "Namespaces: UTS, IPC, Mount, Network, and PID configured");
    Ok(())
}

/// Sets the container private hostname inside the UTS namespace.
pub fn set_container_hostname(hostname: &str) -> Result<()> {
    log_debug!("container", "Namespaces: setting UTS hostname");
    if let Err(_) = nix::unistd::sethostname(hostname) {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError("sethostname".to_string(), errno));
    }
    log_info!("container", "Namespaces: hostname locked to '{}'", hostname);
    Ok(())
}

/// Configures the loopback network interface 'lo' inside the isolated NET namespace.
pub unsafe fn configure_loopback_interface() -> Result<()> {
    log_debug!("container", "Network: bringing loopback interface online");

    // Create a raw socket to issue interface command ioctl
    let socket_fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
    if socket_fd < 0 {
        let errno = nix::errno::Errno::last() as i32;
        return Err(HeliosError::SyscallError("socket(AF_INET)".to_string(), errno));
    }

    // Initialize interface request structure (ifreq) for 'lo'
    let mut ifr: libc::ifreq = std::mem::zeroed();
    let if_name = b"lo\0";
    std::ptr::copy_nonoverlapping(
        if_name.as_ptr(),
        ifr.ifr_name.as_mut_ptr() as *mut u8,
        if_name.len(),
    );

    // 1. Get current interface flags
    if libc::ioctl(socket_fd, libc::SIOCGIFFLAGS, &mut ifr) < 0 {
        let errno = nix::errno::Errno::last() as i32;
        libc::close(socket_fd);
        return Err(HeliosError::SyscallError("ioctl(SIOCGIFFLAGS)".to_string(), errno));
    }

    // 2. Set UP and RUNNING flags
    ifr.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as i16;

    if libc::ioctl(socket_fd, libc::SIOCSIFFLAGS, &ifr) < 0 {
        let errno = nix::errno::Errno::last() as i32;
        libc::close(socket_fd);
        return Err(HeliosError::SyscallError("ioctl(SIOCSIFFLAGS)".to_string(), errno));
    }

    libc::close(socket_fd);
    log_info!("container", "Network: loopback interface 'lo' is now active");
    Ok(())
}
