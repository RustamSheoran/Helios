# Helios Container Process Model: Fork/Exec Mechanics, Synchronization Barriers & Descriptor Hygiene

The process model of `helios-container` governs the lifecycle, resource constraints, and signal propagation of processes executing across namespace boundaries.

This document details the parent-child supervisor process model, explains the mechanics of the pipe-based synchronization barrier, analyzes descriptor inheritance hazards, and explores potential failure modes and process constraints.

---

## 1. Parent-Child Process Model and Supervisors

To isolate processes in a new PID namespace, `helios-container` implements a dual-stage fork-exec architecture:
1. **The Host Supervisor (Parent)**: Operates in the host PID namespace. It acts as an out-of-band monitor responsible for namespace unsharing, provisioning cgroup limits, synchronizing child startup, and cleaning up system resources upon container termination.
2. **The Container Jail Process (Child/Init)**: Created by the secondary fork immediately after the namespace unsharing. It resides in the new namespaces, configures the local mount table (`pivot_root`), drops privileges, installs Seccomp system call filters, and executes the user's targeted payload via `execve`.

```
                    [ Shell Process (Host Context) ]
                                   |
                         fork & exec (Sudo Context)
                                   v
             [ helios-container Parent (Host Supervisor) ]
                                   |
                  - unshare() new namespaces
                  - pipe2() synchronization pipe
                                   |
                             secondary fork()
                                   |
                  +----------------+----------------+
                  |                                 |
        [ Parent Supervisor ]              [ Child Jail Process ]
        - Mappings: Host PID               - Mappings: Container PID 1
        - Configures cgroups v2            - Blocked on read(sync_pipe)
        - Writes child PID to cgroup       - Executes pivot_root()
        - Writes Go byte to pipe           - Installs Seccomp BPF filter
        - waitpid() monitoring loop        - execve("/bin/sh") payload
```

---

## 2. Non-Goals

The following design objectives are explicitly classified as **Non-Goals** for `helios-container`'s process model:
* **Daemon Process Management**: Helios does not act as a process daemon monitor (like `systemd` or `supervisord`). It is not designed to restart failed internal container services.
* **Clustered Process Orchestration**: It does not implement multi-host process scheduling, networking synchronization, or cluster state tracking (like Kubernetes).
* **Multi-Threaded Jail Setup**: The setup phase inside the container jail is strictly single-threaded, avoiding concurrent race hazards before the `execve` boundary.

---

## 3. Potential Failure Modes

### 3.1 Descriptor Inheritance Leakage
* **Why it happens**: When the parent process forks the child, the child inherits duplicate references to *all* open file descriptors of the parent by default.
* **Systems Implication**: If the parent process had open connections to host databases, log files, or raw socket descriptors and fails to set the `O_CLOEXEC` flag or sweep-close descriptors before calling `execve`, the jailed container process can read or write to these host resources, completely bypassing filesystem isolation.
* **Helios Mitigation**: Helios implements an aggressive file descriptor sweep (`close(fd)`) for descriptors above `2` before `execve`, but any descriptor missed during the sweep remains accessible.

### 3.2 Incorrect Cgroup Migration Timing
* **Why it happens**: If the synchronization pipe fails or if the parent writes the child PID to `cgroup.procs` *after* the child has executed `execve`.
* **Systems Implication**: The child starts executing the payload unconstrained. If the payload immediately spawns memory-intensive loops, it can trigger host-wide memory exhaustion before the parent’s cgroup write completes.

### 3.3 Mount Propagation Leakage
* **Why it happens**: If Mount namespace unsharing executes but the root mount `/` propagation is left as `MS_SHARED` instead of being explicitly marked `MS_PRIVATE`.
* **Systems Implication**: Mounts executed inside the jail will propagate back to the host, polluting the host VFS table and potentially unmounting host systems during container cleanup.

### 3.4 Namespace Lifecycle Races
* **Why it happens**: If the parent process supervisor terminates abruptly (e.g., due to a host-level crash) while the child is still blocked on the synchronization pipe.
* **Systems Implication**: The child's blocking `read()` returns `0` (EOF). If the child does not handle this exit signal, it can execute the payload in an unconstrained host state. Helios is designed to abort if `read` returns `0`, but any uncaught error path will leak processes.

### 3.5 PID 1 Signal Immunity Hazards
* **Why it happens**: The kernel blocks default signals (`SIGINT`, `SIGTERM`) sent to PID 1 inside the namespace.
* **Systems Implication**: If the user payload runs as PID 1 and does not register signal handlers, attempting to stop the container from inside the namespace using standard commands will fail. The container can only be terminated via signals sent from the host namespace.
