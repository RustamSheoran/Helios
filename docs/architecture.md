# Helios Subsystem Architecture: Subsystem Coupling, Integration & Systemic Limitations

Helios is an **educational systems monorepo** designed to explore low-level Linux systems programming and kernel interfaces. It bridges low-level kernel abstractions (namespaces, cgroups v2, process credentials, terminal session groups) and classic systems runtime designs (command-line shells, heap memory allocators). 

This document details the architectural topology, boundary coordination, systemic failure modes, and subsystem coupling across the three core modules: `helios-container`, `helios-shell`, and `helios-allocator`.

---

## 1. System Integration Flow and Boundaries

Helios spans three distinct execution boundaries:
1. **Unmanaged Userspace Application Layer (`helios-shell` and process pipelines)**: Responsible for command translation, session job management, and input/output redirection.
2. **Resource-Isolated Userspace Jails (`helios-container`)**: Enforces best-effort operational boundaries, system call restrictions, and OCI-like container handshakes.
3. **Privileged Kernel Space (Linux Subsytems)**: Enforces physical namespace barriers, cgroup v2 controller boundaries, virtual memory maps, and signal routes.

```
+---------------------------------------------------------------------------------------+
|                                    USERSPACE                                          |
|                                                                                       |
|   +-------------------------------------------------------------------------------+   |
|   |                              HELIOS-SHELL (REPL)                              |   |
|   |  - Process Group Leader / Terminal Session coordinator                        |   |
|   |  - Tokenizer / Parser AST Generator                                           |   |
|   |  - Synchronous / Asynchronous Process Pipeline Spawner                         |   |
|   +---------------------------------------+---------------------------------------+   |
|                                           |                                           |
|                                           | fork & exec (helios-container)            |
|                                           v                                           |
|   +-------------------------------------------------------------------------------+   |
|   |                      HELIOS-CONTAINER RUNTIME (PARENT)                        |   |
|   |  - Unshares UTS, Mount, IPC, PID, & Net namespaces                            |   |
|   |  - Provisions Cgroup v2 Controller Nodes (`/sys/fs/cgroup/helios/`)           |   |
|   |  - Synchronizes child execution via raw OCI-like O_CLOEXEC sync pipe          |   |
|   +---------------------------------------+---------------------------------------+   |
|                                           |                                           |
|                                           | Synchronization Barrier Handshake         |
|                                           v                                           |
|   +-------------------------------------------------------------------------------+   |
|   |                       HELIOS-CONTAINER JAIL (CHILD)                           |   |
|   |  - pivot_root() confinement & Mount propagation (MS_PRIVATE)                  |   |
|   |  - Mounts procfs, sysfs, devtmpfs                                             |   |
|   |  - Drops capabilities, installs Seccomp BPF filters via prctl()               |   |
|   |  - execve() target payload (e.g., /bin/sh)                                    |   |
|   +---------------------------------------+---------------------------------------+   |
|                                           |                                           |
|                                           | Memory Allocations (Global Allocator)     |
|                                           v                                           |
|   +-------------------------------------------------------------------------------+   |
|   |                               HELIOS-ALLOCATOR                                |   |
|   |  - Global Heap Broker (GlobalAlloc implementation)                            |   |
|   |  - Slab Cache Engine (O(1) allocation classes: 16B - 1024B)                   |   |
|   |  - Page-backed doubly-linked Best-Fit block manager (mmap / munmap)            |   |
|   +---------------------------------------+---------------------------------------+   |
|                                           |                                           |
+-------------------------------------------|-------------------------------------------+
                                            |
+-------------------------------------------v-------------------------------------------+
|                                   KERNEL SPACE                                        |
|                                                                                       |
|   - Namespaces: clone(), unshare(), setns() isolation mechanics                       |
|   - VFS Subsystem: mount(), umount2(), pivot_root() virtual file boundaries           |
|   - Cgroups v2: memory.max, cpu.max limits mapped through sysfs                       |
|   - Signal Infrastructure: kill(), sigaction() process control                      |
|   - Virtual Memory Manager: mmap(), munmap() page backing operations                  |
+---------------------------------------------------------------------------------------+
```

---

## 2. Non-Goals

To maintain a clear and rigorous project scope, the following objectives are explicitly classified as **Non-Goals** for the Helios architecture:
* **Hardened Multi-Tenant Isolation**: Helios is not designed to secure hostile workloads. It lacks protection against kernel local privilege escalation exploits (LPE) or hardware side-channel attacks (like Meltdown or Spectre).
* **OCI-Spec Compliance**: Helios does not attempt to parse full OCI runtime specification JSON structures or interact with production orchestrators (like Kubernetes or Docker container engines).
* **NUMA Awareness & CPU Affinity**: The custom allocator and process scheduler do not attempt to map allocations to specific NUMA nodes or coordinate thread pools across CPU sockets.
* **Lock-Free Allocator Design**: Thread-safety is achieved using a coarse-grained global lock, prioritizing structural correctness over raw concurrent throughput.
* **Production Sandbox Security**: Unlike microVM-based sandboxes (e.g., Firecracker) or userspace kernels (e.g., gVisor), Helios shares the host Linux kernel directly, offering best-effort virtualization rather than absolute virtualization boundaries.

---

## 3. Subsystem Coupling and Interfaces

### 3.1 The Global Allocator Boundary
`helios-allocator` acts as the `#[global_allocator]` for the entire monorepo. This coupling introduces strict runtime constraints:
* **Deadlock Hazards in Signal Handlers**: Both the shell and the container rely on kernel signals (`SIGCHLD`, `SIGTSTP`, `SIGTTOU`, `SIGTTIN`). Because signal handlers interrupt thread execution arbitrarily, they must not perform operations that require dynamic memory allocation. If a signal handler interrupts the global allocator while a mutex lock is held, attempting to allocate inside the handler causes an immediate and unrecoverable deadlock. Helios mitigates this by enforcing **Zero-Allocation Signal Handlers** relying on pre-allocated static tables and direct raw system calls (`libc::write`).
* **Telemetry Allocation Traps**: Standard I/O print macros (like `print!`) perform dynamic heap allocations internally. To prevent recursive allocator crashes during diagnostic outputs, `helios-allocator` writes diagnostic events directly via standard error file descriptors (`libc::STDERR_FILENO`) utilizing a custom raw log writer that performs no dynamic heap allocations.

### 3.2 Shell-Container Interlock
The shell spawns the container by invoking `helios-container` as a standard child process. 
* **State Handover**: The shell manages process groups using `setpgid` and delegates terminal control via `tcsetpgrp`. When invoking the container, the container parent process is registered in the shell’s job table.
* **Privilege Boundaries**: The shell runs as an unprivileged user session. However, namespace isolation (`CLONE_NEWNS`, `CLONE_NEWPID`) and cgroup directory manipulation require privileged access. Therefore, the container runtime operates as a `sudo` process. The shell yields controlling terminal operations cleanly, ensuring signal routing (`SIGINT`, `SIGTSTP`) is directed entirely to the root supervisor parent of the container, rather than leaking back into the shell session.

---

## 4. Potential Failure Modes

### 4.1 Subsystem Coupling Cascades
* **Failure Mode**: If `helios-allocator` runs out of virtual memory pages (e.g., `mmap` returns `MAP_FAILED` under memory exhaustion), any subsequent allocation inside `helios-shell` or `helios-container` will trigger an immediate Rust panic. 
* **Systems Implication**: Unlike robust production environments where runtimes catch allocation failures gracefully, Helios handles memory exhaustion by aborting the execution context. This can leave container processes partially configured or leave orphaned directories inside the `/sys/fs/cgroup` virtual filesystem.

### 4.2 Terminal Desynchronization
* **Failure Mode**: If the container runtime crashes before returning terminal control to the parent shell, the controlling terminal remains bound to the container's PGID.
* **Systems Implication**: The parent shell will be blocked from reading keyboard inputs, resulting in a hung terminal session that requires a manual `reset` command or a kill signal from an external terminal.

---

## 5. Compatibility Assumptions & Environment Invariants

The Helios architecture relies on specific host configurations. Variations in these configurations can result in runtime failures:
* **Unified Cgroups v2 Hierarchy**: Helios assumes the host runs cgroups v2 mapped at `/sys/fs/cgroup/`. If run on older distributions using cgroups v1, directory paths (like `/sys/fs/cgroup/unified`) will fail to resolve, blocking container creation.
* **Distro-Specific Security Modules (LSM)**: Host configurations utilizing SELinux or AppArmor may block recursive private mount operations or system call filters. Running under strict SELinux policies without custom security profiles will result in `EPERM` errors during `mount` or `pivot_root` execution.
* **Library Linkage (glibc vs. musl)**: Under glibc, certain system wrappers perform dynamic operations during child forks. Helios relies on direct raw register calls via `libc::syscall` to bypass userspace library variations, but variations in child execution environments can occur if the host lacks a statically compiled target structure.
* **Systemd Ownership**: On systemd-based hosts, systemd acts as the single writer for `/sys/fs/cgroup`. If Helios creates directories directly, systemd's periodic sweeps may remove them, resulting in race conditions.
