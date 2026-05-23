# Helios Systems Observability & Container Debugging Handbook

This handbook outlines the diagnostics workflows, kernel inspection points, tracing configurations, and observability practices needed to debug low-level systems programs (such as memory allocators, interactive shells, and custom container runtimes).

---

## 1. Non-Goals

The following diagnostics objectives are explicitly classified as **Non-Goals** for Helios' debugging handbook:
* **Production Observability Dashboard**: Helios does not implement structured metrics exporters (like Prometheus endpoints) or centralized logging pipelines.
* **Kernel-Level Core Dump Analysis**: It does not cover deep kernel crash dump analysis (`kdump`/`crash`), focusing purely on userspace runtime observability.
* **Cross-Platform Profiling**: All workflows are hardcoded for standard Linux interfaces, bypassing macOS or Windows systems diagnostics.

---

## 2. Potential Debugging Failure Modes and Caveats

### 2.1 Ptrace Virtualization Constraints
* **The Caveat**: Tracing container processes using `strace` or `gdb` (which rely on the `ptrace` system call) from inside the container requires the process to have `CAP_SYS_PTRACE` capabilities.
* **Systems Implication**: If the container runtime dropped capabilities to secure the jail, attempting to run `strace` inside the jail will fail with `EPERM`. Diagnostics must be executed from the host namespace using `nsenter` or host-level `strace -p <pid>`.

### 2.2 Seccomp SIGSYS Confusion
* **The Caveat**: When a process triggers a Seccomp BPF filter violation, the kernel immediately kills the thread with a `SIGSYS` signal.
* **Systems Implication**: If a debugger is attached, it may intercept the `SIGSYS` signal but report it as a generic crash. System engineers must check `dmesg` or `/proc/sys/kernel/core_pattern` logs to distinguish between standard memory segfaults and Seccomp BPF violations.

### 2.3 Namespace Observability Blindspots
* **The Caveat**: When executing commands inside an isolated Mount namespace, standard host commands (like `df` or `mount`) show only the host's mount tables.
* **Systems Implication**: To inspect the container's private VFS layer, engineers must either enter the namespace using `nsenter` or query the specific process mount file under procfs:
  ```bash
  cat /proc/<container_pid>/mounts
  ```

---

## 3. Namespace Inspection and Entry Workflows

When a container process is isolated within namespaces, default observability commands (like `ps`, `ip link`, or `mount`) run inside the host context, making them blind to container boundaries.

### 3.1 Entering Container Namespaces with `nsenter`
To run diagnostic tools directly inside the container's isolated context from the host:
1. **Locate Child PID**: Find the container's PID in the host namespace:
   ```bash
   ps aux | grep helios-container
   ```
2. **Execute Entry**: Enter the UTS, Mount, PID, IPC, and Network namespaces:
   ```bash
   sudo nsenter -t <container_pid> -m -u -i -n -p /bin/sh
   ```
   This shifts the host terminal session's namespaces to match the container's file system, network routing, and process tree, allowing in-jail diagnostics.

### 3.2 Inspecting `/proc/<pid>/ns/`
The Linux kernel exposes namespace handles as virtual symlinks in `/proc`:
```bash
ls -la /proc/<container_pid>/ns/
```
Each file (e.g., `mnt`, `pid`, `net`) contains a unique inode signature:
```text
mnt -> 'mnt:[4026531840]'
net -> 'net:[4026531905]'
```
If two processes have matching inode signatures, they reside in the same physical namespace. If the inodes differ, they are separated by kernel barriers.
