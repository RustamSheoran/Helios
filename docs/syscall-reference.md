# Linux System Call Reference: Low-Level Primitives, Invariants & Failure Semantics

This document provides a low-level systems engineering reference for the core Linux system calls utilized by the Helios runtime ecosystem. 

Each system call is analyzed by its system signature, privilege requirements, namespace interactions, kernel invariants, potential failure modes, and safety implications.

---

## 1. UTS, Mount & PID Namespace Virtualization System Calls

### 1.1 `unshare(int flags)`
* **Description**: Detaches the calling process's context from its current shared global resources, establishing new namespace boundaries.
* **Core Flags**:
  * `CLONE_NEWNS` (Mount): Virtualizes the VFS mount table.
  * `CLONE_NEWPID` (PID): Virtualizes the process ID namespace for subsequent children.
  * `CLONE_NEWUTS` (UTS): Virtualizes the hostname/NIS domain.
  * `CLONE_NEWIPC` (IPC): Virtualizes POSIX message queues and System V IPC structures.
  * `CLONE_NEWNET` (Network): Virtualizes physical device interfaces and routing tables.
* **Privilege Requirement**: `CAP_SYS_ADMIN` in the active user namespace.
* **Kernel Invariants**:
  * Calling `unshare(CLONE_NEWPID)` does *not* place the calling process into the new PID namespace; only its subsequently spawned child processes will enter the namespace and PID 1 mapping.
* **Common Failure Modes**:
  * `EPERM`: The calling process lacks `CAP_SYS_ADMIN` credentials.
  * `ENOMEM`: Kernel failed to allocate namespace tracking structures.
  * `EINVAL`: An invalid combination of flags was specified.

### 1.2 `pivot_root(const char *new_root, const char *put_old)`
* **Description**: Moves the root mount point of the calling process's mount namespace to `new_root` and places the old root mount point at `put_old`.
* **Privilege Requirement**: `CAP_SYS_ADMIN` inside the active mount namespace.
* **Kernel Invariants & Constraints**:
  * `new_root` and `put_old` must be directories.
  * `new_root` must not be on the same mount point as the current root `/`. To satisfy this, runtimes perform a self-bind mount of `new_root` before calling `pivot_root`.
  * `put_old` must reside under `new_root`.
  * The current root `/` and the target `new_root` must be marked as private propagation mounts.
* **Why Direct Syscall is Preferred**: Standard libc implementations (like `glibc`) do not expose a public system wrapper function for `pivot_root`. Helios invokes it directly via register setup and the kernel system interface:
  ```rust
  libc::syscall(libc::SYS_pivot_root, new_root.as_ptr(), put_old.as_ptr())
  ```
* **Common Failure Modes**:
  * `EINVAL`: `new_root` is not a mount point, or mount propagation is not private.
  * `EBUSY`: `new_root` or `put_old` is currently active in another mount context.
  * `ENOTDIR`: A path parameter does not point to a valid directory.

---

## 2. Non-Goals

The following objectives are explicitly classified as **Non-Goals** for Helios' system call implementations:
* **Multi-Architecture Wrapper Support**: Helios is hardcoded for `x86_64` Linux register mappings and syscall numbers. It does not support alternate architectures (like ARM64 or RISC-V) without code modification.
* **Abstract Libc Bypassing**: It does not bypass libc completely, utilizing standard `libc::syscall` entry points rather than inline assembly blocks.
* **System Call Emulation**: It does not intercept or emulate syscalls, sending them directly to the host kernel.

---

## 3. Subsystem System Call Failure Semantics

When invoking raw system calls, error handling is critical:
* **The `errno` Interface**: Standard libc wrappers set the thread-local `errno` variable on failure.
* **Failure Modes**:
  * If `mount` returns `-1`, the runtime must read `errno` immediately to map the failure. If the parent performs other operations first, `errno` may be overwritten, masking the true error.
  * If `setpgid` fails with `EPERM` during child initialization, the child must immediately abort to prevent running the payload in an unprivileged or incorrectly routed terminal group.
