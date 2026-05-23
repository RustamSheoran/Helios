# Helios Design Tradeoffs: Systems Decisions & Architecture Rationale

Systems programming requires balancing resource efficiency, performance guarantees, safety boundaries, and implementation complexity. In an educational systems project like Helios, these tradeoffs are deliberately chosen to explore kernel interfaces.

This document details the architectural decisions, structural compromises, and technical tradeoffs chosen across the Helios allocator, shell, and container subsystems, contrasting them with standard production-grade runtime alternatives.

---

## 1. Memory Management Tradeoffs

### 1.1 Best-Fit vs. First-Fit Allocations
* **Helios Decision**: Doubly-linked intrusive free list using a Best-Fit search strategy.
* **Tradeoff Analysis**:
  * **First-Fit**: Selects the first free block that is $\ge$ requested size. It has low allocation latency ($O(1)$ best-case, stopping as soon as a block is found) but suffers from high fragmentation over long-running workloads, as large blocks are sliced prematurely.
  * **Best-Fit**: Traverses the *entire* free list to find the block that minimizes residual space. This significantly reduces external fragmentation at the cost of $O(N)$ linear search complexity.
* **Performance Implication**: For a large, fragmented heap, Best-Fit search times degrade linearly with list length. Helios mitigates this for small objects using the Slab cache, but large-block allocations remain bound to linear list traversals.
* **Production Alternative**: Production allocators (like `jemalloc`) utilize segregated free lists indexed by Radix Trees or balanced binary trees, achieving $O(\log N)$ search times while maintaining optimal space utilization.

### 1.2 Intrusive Metadata vs. Separate Metadata Tables
* **Helios Decision**: Prefixes all large blocks with a `BlockHeader` stored immediately before the payload inside the heap.
* **Tradeoff Analysis**:
  * **Intrusive Layout**: Eliminates the need to allocate external tables to track free blocks—preventing recursive allocator traps.
  * **Safety Hazard**: If a userspace payload executes a buffer overflow, it will overwrite the metadata header of the physically adjacent block. This corrupts pointer offsets (`next`, `prev`), resulting in immediate allocator crashes or arbitrary memory writes during subsequent merges/splits.
* **Production Alternative**: Hardened allocators isolate block metadata in dedicated, read-only or randomized table segments, protecting pointer structures from linear buffer overflow exploits.

### 1.3 Slab Allocators for Small Objects
* **Helios Decision**: Fixed-size slot caches mapped via page-aligned Slabs.
* **Tradeoff Analysis**:
  * **Advantage**: Achieves $O(1)$ constant-time allocation and deallocation for frequent small sizes ($16\text{B}-1024\text{B}$) while eliminating internal fragmentation for those classes. Addresses are resolved in zero-overhead time using CPU intrinsics and bitmask lookups, combined with $O(1)$ pointer round-downs (`ptr & !(4095)`) to locate parent slab pages.
  * **Complexity Cost**: Requires dedicating entire $4\text{KB}$ pages to a single size class. If a thread allocates only a single 16-byte object, the remaining 4080 bytes on that page are locked and unusable by other size classes, amplifying internal fragmentation under sparse workloads.

---

## 2. Shell & Job Control Tradeoffs

### 2.1 Process Groups and Controlling Terminals
* **Helios Decision**: Process Group isolation (`setpgid`) and Controlling Terminal handovers (`tcsetpgrp`).
* **Tradeoff Analysis**:
  * **Rationale**: Without process groups, sending an interrupt signal (like `SIGINT` via `Ctrl+C`) would trigger signal delivery to every process sharing the shell's active session, killing the shell itself. Setting up PGIDs allows the terminal driver to route keyboard interrupts exclusively to the foreground pipeline group.
  * **Complexity Cost**: Requires maintaining and auditing child process group states, handling background suspensions (`SIGTTIN`/`SIGTTOU`), and resolving race conditions by calling `setpgid` in *both* parent and child contexts.

### 2.2 Unidirectional Pipes vs. Shared Memory for Pipelines
* **Helios Decision**: Low-level kernel pipe structures to wire process I/O.
* **Tradeoff Analysis**:
  * **Advantage**: Leverage standard kernel buffering and stream abstractions. When a writer terminates, the kernel automatically generates EOF signals, unblocking subsequent processes in the pipeline.
  * **Performance Cost**: Data must be copied twice across the kernel-userspace boundary (from Child A userspace to Kernel pipe buffer, then from Kernel buffer to Child B userspace). For high-throughput data processing, this adds significant context-switching and copy overhead.
  * **Production Alternative**: High-performance systems use shared memory ring buffers (e.g., vring or custom UNIX domain socket configurations) to bypass memory copy boundaries.

---

## 3. Container Isolation & Virtualization Tradeoffs

### 3.1 Unshare + Fork vs. Raw Clone System Calls
* **Helios Decision**: Calling `unshare` followed by a secondary `fork` to spawn the container jail process.
* **Tradeoff Analysis**:
  * **Rationale**: In the Linux kernel, the `CLONE_NEWPID` flag dictates that only the *subsequent* children spawned by a process will enter the new PID namespace. The calling process remains in the parent namespace. Therefore, to make the container jail PID 1 inside the namespace, the supervisor parent must perform a secondary fork.
  * **Complexity Cost**: Adds process hierarchy overhead, requiring the parent to remain active as a supervisor loop to monitor the container, handle signal forwarding, and manage directory cleanup.

### 3.2 Synchronization Pipes vs. Blind Execve
* **Helios Decision**: A blocking, unidirectional pipe-based handshake between parent and child.
* **Tradeoff Analysis**:
  * **Rationale**: Prevents a critical race condition where the child executes `execve` before the parent has registered the child's PID in `/sys/fs/cgroup/` and written resource limits.
  * **Tradeoff**: Adds file descriptor tracking overhead and introduces potential deadlock vectors if the parent crashes or closes descriptors out of order.

### 3.3 Private Mount Propagation
* **Helios Decision**: Changing the mount namespace root to `MS_PRIVATE` before invoking `pivot_root`.
* **Tradeoff Analysis**:
  * **Rationale**: If mounts are left as `MS_SHARED` (the default on systemd-based distributions), mounting or unmounting VFS targets inside the container would immediately propagate back to the host, polluting the host's mount table and potentially unmounting critical host volumes.
  * **Security Cost**: Completely isolates the container mount tree from the host. However, it blocks the container from dynamically receiving storage updates or shared host volumes without explicit recursive bind mounts.

### 3.4 Seccomp BPF Filters vs. Ptrace Interception
* **Helios Decision**: Static BPF filter loading via `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, ...)`.
* **Tradeoff Analysis**:
  * **Seccomp BPF**: Evaluates system calls statically inside the kernel path. It adds zero-overhead context switches, ensuring high performance. However, filters cannot dynamically inspect system call arguments stored in memory buffers (e.g., checking file paths in `openat`), as Seccomp only verifies raw register values.
  * **Ptrace Interception**: Allows dynamic, userspace inspection of all register and buffer contents. However, it requires context-switching out of the kernel to the tracer process on *every* system call entry and exit, degrading execution speeds by orders of magnitude.
  * **Production Alternative**: Advanced sandboxes (like `gVisor`) run a complete userspace kernel that intercepts syscalls via custom virtualization extensions (like KVM or modified ptrace paths) to enforce deep semantic security constraints.
