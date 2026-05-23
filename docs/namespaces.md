# Linux Namespaces: Kernel Virtualization, Lifecycle Mechanics & Container Isolation

Linux Namespaces are the foundational resource-virtualization primitive of the Linux kernel. They allow a process (or group of processes) to obtain an isolated view of specific global system resources.

This document details the mechanics of UTS, Mount, IPC, PID, and Network namespace isolation, explains mount propagation theory, details the unique fork-exec constraints of PID namespaces, and analyzes potential failure modes and comparison with production OCI container runtimes.

---

## 1. Namespace Types and Virtualization Boundaries

```
+-----------------------------------------------------------------------------------------------+
|                                      HOST KERNEL (Linux)                                      |
|                                                                                               |
|   +---------------------------------------------------------------------------------------+   |
|   |                              HELIOS CONTAINER SANDBOX                                 |   |
|   |                                                                                       |   |
|   |  - UTS: Isolated Hostname (sethostname() bypasses host)                               |   |
|   |  - PID: Isolated Process Tree (Jail child is PID 1)                                   |   |
|   |  - Mount: Private mount points (MS_PRIVATE | pivot_root)                             |   |
|   |  - IPC: Isolated POSIX Message Queues & System V IPC                                  |   |
|   |  - Network: Virtual loopback device & veth interface routing                          |   |
|   +-------------------------------------------+-------------------------------------------+   |
|                                               |                                               |
|                                               | unshare() or clone() system calls             |
|                                               v                                               |
|   +---------------------------------------------------------------------------------------+   |
|   |                                   HOST NAMESPACES                                     |   |
|   |                                                                                       |   |
|   |  - Shared host routing table, active physical devices, root process tree, host disks  |   |
|   +---------------------------------------------------------------------------------------+   |
+-----------------------------------------------------------------------------------------------+
```

### 1.1 PID Namespace (`CLONE_NEWPID`) and the Fork Invariant
When a process calls `unshare(CLONE_NEWPID)`, only its *children* enter the new PID namespace. The calling process remains in the parent namespace. The first child spawned inside the namespace becomes **PID 1 (init)**.
* **PID 1 Semantics**:
  * If PID 1 terminates, the kernel immediately sends a `SIGKILL` to all other processes inside that PID namespace, tearing down the container.
  * PID 1 is responsible for reaping orphans. If a background process forks and its direct parent exits, the background child is re-parented to PID 1. PID 1 must run an active event loop to reap these processes via `waitpid`.

---

## 2. Non-Goals

The following isolation and virtualization objectives are explicitly classified as **Non-Goals** for `helios-container`'s namespace implementation:
* **Hardened Security Boundaries**: Namespaces are best-effort resource views, not security sandboxes. They share the host kernel directly. Helios does not protect against kernel vulnerabilities (e.g., local privilege escalations).
* **Rootless User namespace UID Mapping**: Helios does not implement complex sub-UID/sub-GID host mapping loop configurations for rootless isolation without privileged host setup, requiring sudo to initialize namespaces.
* **Comprehensive Network Bridging**: It does not implement complex overlay networks, dynamic DNS, or multi-host routing protocols.

---

## 3. Potential Failure Modes

### 3.1 Incomplete Isolation via Shared /proc
* **Why it happens**: If the container runtime mounts the host's `/proc` filesystem inside the container instead of mounting a fresh `proc` filesystem, the namespaces boundary is bypassed.
* **Systems Implication**: Processes inside the container can view every process running on the host system, inspect host environment variables, and potentially send signals across the boundary, completely violating PID namespace isolation.

### 3.2 Capability Leakage
* **Why it happens**: If the runtime drops privileges (`setresuid`) but fails to clear the process's **Inheritable Capability Set** or fails to drop transition capabilities (like `CAP_SYS_ADMIN`), the containerized process can regain root privileges.
* **Systems Implication**: An attacker executing code inside the container can perform raw mount operations, bypass filesystem checks, or access host devices.

### 3.3 Mount Propagation Leaks
* **Why it happens**: If the root mount `/` propagation is not explicitly changed to `MS_PRIVATE` before mounting filesystems, mounts are inherited as `MS_SHARED`.
* **Systems Implication**: Mounting a filesystem (like `/proc` or a bind mount) inside the container propagates directly back to the host, polluting the host's mount table and potentially unmounting host systems when the container terminates.

### 3.4 Host Network Namespace Inheritance
* **Why it happens**: If network namespace configuration fails during container startup, the container falls back to inheriting the host's network namespace (`CLONE_NEWNET` fails or is skipped).
* **Systems Implication**: The containerized process can bind to host ports, intercept host network packets, and potentially access private local network interfaces.

---

## 4. Real-World Runtimes Comparison

`helios-container` uses standard Linux primitives to isolate processes, but differs significantly from production systems:

| Feature | Helios | runc | crun | gVisor |
| :--- | :--- | :--- | :--- | :--- |
| **Language** | Rust | Go | C | Go |
| **Kernel Boundary** | Shared Host Kernel | Shared Host Kernel | Shared Host Kernel | Isolated Userspace Kernel (Sentry) |
| **Namespace Configuration** | Direct syscalls | Go `libcontainer` | Direct C syscall wrappers | Virtualized Syscall Interception |
| **Security Profiles** | Basic Seccomp BPF | AppArmor, SELinux, Seccomp | AppArmor, SELinux, Seccomp | Full Kernel Sandbox |
| **Rootless Execution** | None (Requires sudo) | Full Sub-UID Mapping | Full Sub-UID Mapping | Unprivileged User Mapping |

### What Helios Imitates:
* The core OCI lifecycle concept of **Namespace Unsharing** and **Pivot Root** filesystem confinement.
* Enforcing Seccomp BPF system call restrictions at the kernel-userspace boundary.

### What Production Runtimes Additionally Implement:
* **Advanced LSM Integrations**: Automatic generation of AppArmor profiles and SELinux multi-category security (MCS) tags to restrict file access even if a process breaks out of the mount namespace.
* **Rootless Namespace Mapping**: Complex user namespace configurations that map a range of unprivileged host UIDs to container UIDs, allowing non-root users to execute containers safely.
