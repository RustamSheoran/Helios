# Helios: Systems Monorepo (Container Runtime, Unix Shell & Memory Allocator)

Helios is an **educational systems engineering monorepo** implemented in low-level Rust. It provides a platform for exploring OS resource virtualization, interactive process job control streams, and custom heap memory layouts by directly interfacing with Linux kernel system primitives.

The project is structured as a single Cargo workspace comprised of three integrated subsystems:
1. **`helios-container`**: An experimental container manager managing UTS, Mount, IPC, PID, and Network namespace isolation, private mount propagation, `pivot_root` virtual filesystems, cgroups v2 resource limits, and raw Seccomp BPF filters.
2. **`helios-shell`**: An interactive command-line interpreter with Lexer-Parser AST pipelines, file redirection, background job control tables, process group separation (`setpgid`), and controlling terminal handovers (`tcsetpgrp`).
3. **`helios-allocator`**: A custom global heap allocator (`GlobalAlloc`) managing page-backed allocations via intrusive doubly-linked Best-Fit free lists with physical coalescing, combined with fixed-slot Slab caches for small object footprints.

---

## 1. Global Subsystem Dependency Topology

The following graph maps the flow of execution and data dependencies between userspace subsystems, the custom allocator boundary, and the Linux kernel:

```
+---------------------------------------------------------------------------------------+
|                                    HELIOS USERSPACE                                   |
|                                                                                       |
|   +-------------------------------------------------------------------------------+   |
|   |                             HELIOS-SHELL (REPL)                               |   |
|   |  - Parses AST                                                                 |   |
|   |  - Handles background job control & process groups                            |   |
|   +---------------------------------------+---------------------------------------+   |
|                                           |                                           |
|                                           | Spawns container commands via fork-exec   |
|                                           v                                           |
|   +-------------------------------------------------------------------------------+   |
|   |                       HELIOS-CONTAINER (SUPERVISOR)                           |   |
|   |  - Unshares namespaces                                                        |   |
|   |  - Applies cgroup v2 limits                                                   |   |
|   |  - Handshakes child execution via synchronizing O_CLOEXEC pipes               |   |
|   +---------------------------------------+---------------------------------------+   |
|                                           |                                           |
|                                           | Intercepts dynamic memory requests        |
|                                           v                                           |
|   +-------------------------------------------------------------------------------+   |
|   |                       HELIOS-ALLOCATOR (GLOBAL BROKER)                        |   |
|   |  - Manages heap allocations via O(1) Slabs & Best-Fit Block lists             |   |
|   |  - Communicates directly with Kernel VMM using anonymous mmap/munmap          |   |
|   +---------------------------------------+---------------------------------------+   |
|                                           |                                           |
+-------------------------------------------|-------------------------------------------+
                                            |
+-------------------------------------------v-------------------------------------------+
|                                  LINUX KERNEL SPACE                                   |
|                                                                                       |
|   - Namespaces: clone(), unshare(), setns() barriers                                  |
|   - Virtual Filesystem: mount(), umount2(), pivot_root() isolation                    |
|   - Scheduler: CPU CFS limits & Memory controller reclaim loops                       |
|   - Virtual Memory: mmap() / munmap() anonymous pages                                 |
+---------------------------------------------------------------------------------------+
```

---

## 2. Non-Goals

To maintain a clear and rigorous project scope, the following objectives are explicitly classified as **Non-Goals** for the Helios monorepo:
* **Hardened Production Isolation**: Helios is not designed to secure hostile workloads. It shares the host kernel directly and lacks virtualization virtualization boundaries (like microVMs).
* **OCI Specification Compliance**: It does not parse full OCI config JSON specifications or integrate with production orchestrators (like Kubernetes or containerd).
* **Thread-Safe lock-Free Memory Throughput**: Thread-safety inside the global allocator is enforced using coarse global locks, prioritizing layout correctness over concurrent scaling.
* **Production Shell Replacement**: It does not implement POSIX scripting grammar (`if`/`while`/functions), designed solely as an interactive process control shell.

---

## 3. Compatibility Assumptions & Environment Invariants

Helios relies on specific host configurations. Variations in these configurations can result in runtime failures:
* **Unified Cgroups v2 Hierarchy**: Helios assumes the host runs cgroups v2 mapped at `/sys/fs/cgroup/`. If run on older distributions using cgroups v1, directory paths will fail to resolve, blocking container creation.
* **Distro-Specific Security Modules (LSM)**: Host configurations utilizing SELinux or AppArmor may block recursive private mount operations or system call filters. Running under strict SELinux policies without custom security profiles will result in `EPERM` errors during `mount` or `pivot_root` execution.
* **Architecture Constraints**: The codebase is hardcoded for `x86_64` Linux register architectures and system call numbers.

---

## 4. Threat Model & Project Scope

### 4.1 Threat Profile
Helios provides **best-effort virtualization** under standard Linux namespaces and cgroups v2 semantics, not absolute security containment:
* **Shared Kernel Vulnerabilities**: Processes running inside a Helios container share the host kernel directly. If an attacker executes a kernel exploit (LPE), they can escape the namespace boundaries and compromise the host system.
* **Metadata Vulnerabilities**: `helios-allocator` utilizes intrusive metadata headers. A buffer overflow in a userspace payload can overwrite adjacent block metadata, resulting in allocator crashes or arbitrary memory swaps.
* **Descriptor Leakage**: While Helios attempts to close extra file descriptors before execution, any host descriptor leaked during the child fork remains accessible inside the jail.

---

## 5. Compilation & Verification Instructions

### 5.1 Building the Monorepo
To compile the entire workspace in release mode:
```bash
cargo build --release
```

### 5.2 Running Unit and Integration Tests
To run the full suite of automated tests verifying the memory allocator, parsing engine, and isolation limits:
```bash
cargo test
```

### 5.3 Preparing the Rootfs
To boot container environments, extract a minimal Alpine root filesystem:
```bash
./scripts/setup_rootfs.sh
```

### 5.4 Booting an Isolated Container
To launch a shell inside the isolated jail:
```bash
sudo ./target/release/helios-container run container-01 --rootfs ./rootfs --command "/bin/sh"
```
