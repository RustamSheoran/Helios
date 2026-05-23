# Linux Control Groups (Cgroups v2): Unified Resource Management & Kernel Scheduler Constraints

Linux Control Groups (Cgroups) v2 provide a unified hierarchy for managing, limiting, and accounting for system resources (CPU, Memory, I/O, PIDs) consumed by groups of processes.

This document details the Unified Hierarchy specification, the internal mechanics of CPU Completely Fair Scheduler (CFS) throttling, Memory Controller reclaiming limits, potential failure modes, and cgroup coordination.

---

## 1. The Unified Hierarchy (Cgroups v2) Architecture

Cgroups v2 consolidates resource controllers into a single, unified tree structure mapped under `/sys/fs/cgroup/`.
* **The v2 Single-Hierarchy Rule**: In v2, a process can only exist in a single cgroup node. All enabled controllers are applied to that node and its children.
* **The "No Internal Process" Invariant**: A core design rule of cgroups v2 is that **no process can reside in an internal cgroup node that has active child nodes**:
  * If `/sys/fs/cgroup/helios/` has a child node `/sys/fs/cgroup/helios/container-01`, then no process can be registered in `/sys/fs/cgroup/helios/cgroup.procs`.
  * This ensures that resource competition only occurs between leaf nodes (processes) or between sibling cgroup slices, preventing ambiguous scheduling priority calculations.

```
                             [ Root Node: /sys/fs/cgroup ]
                                          |
                         +----------------+----------------+
                         |                                 |
           [ Controller enabling ]              [ Child Node: /helios ]
           - cgroup.subtree_control             - cgroup.procs
           (enable memory, cpu, pids)           - cgroup.threads
                                                           |
                                           +---------------+---------------+
                                           |                               |
                             [ Child Slice: /container-01 ]  [ Child Slice: /container-02 ]
                             - memory.max                    - memory.max
                             - cpu.max                       - cpu.max
```

---

## 2. Non-Goals

The following resource management objectives are explicitly classified as **Non-Goals** for `helios-container`'s cgroup implementation:
* **Cgroups v1 Support**: Helios does not implement fallback paths or compatibility modules for the older, multi-tree cgroups v1 architecture.
* **Systemd Db1/dbus Coordination**: It does not communicate with systemd via D-Bus to register scopes or service slices, relying on direct filesystem writes.
* **Dynamic Resource Auto-Scaling**: It does not dynamically adjust allocations based on real-time container workloads, enforcing only static limits.

---

## 3. Potential Failure Modes

### 3.1 CPU Throttling Starvation
* **Why it happens**: Helios sets CPU limits using the `cpu.max` CFS scheduler interface. If the container process executes intensive concurrent loops, it quickly consumes its allocated quota before the scheduler `period` (typically 100ms) has elapsed.
* **Systems Implication**: The kernel scheduler immediately suspends (throttles) all threads inside the cgroup. The threads remain completely frozen until the next period starts, introducing massive latency spikes.
* **Mitigation in Production**: Production orchestrators use dynamic burst quotas (`cpu.cfs_burst_us` on newer kernels) to allow temporary limit spikes.

### 3.2 Memory Reclaim Stalls
* **Why it happens**: When a container approaches its `memory.max` limit, the kernel suspends allocation calls and enters a synchronous reclaim loop, attempting to purge file-backed page caches and swap out pages.
* **Systems Implication**: The container's execution context experiences severe slowdowns (thrashing) as it blocks waiting for disk I/O operations during the reclaim phase, degrading database and application response times.

### 3.3 Out-Of-Memory (OOM) Group Inconsistencies
* **Why it happens**: By default, if memory reclaim fails, the kernel's OOM killer selects and terminates the single process inside the cgroup with the highest memory footprint.
* **Systems Implication**: If the container runs an init system (PID 1) and multiple background helper processes, the kernel may kill the main payload while leaving the helper processes running as orphans. This results in zombie container states that leak resources.

### 3.4 Systemd Slice Deletion Conflicts
* **Why it happens**: On systemd-based hosts, systemd assumes ownership of the `/sys/fs/cgroup` tree. If a custom runtime writes directories directly without registering them as systemd slices, systemd's periodic sweep routines may identify them as anomalies.
* **Systems Implication**: Systemd can unilaterally migrate processes back to the root slice or delete the container’s cgroup directory, removing all resource limits.

---

## 4. Real-World Orchestration Comparison

In industrial container ecosystems, cgroup management is deeply integrated with host service managers:
* **Systemd Coordination**: Production runtimes (like `runc`) communicate with systemd via D-Bus to instantiate container cgroups as native systemd units (`.scope` or `.slice`). This ensures systemd tracks process lifecycles and prevents directory conflicts.
* **Kubernetes QoS Classes**: Kubernetes maps container limits to specific cgroup structures (Guaranteed, Burstable, BestEffort) to control OOM score adjustments (`/proc/<pid>/oom_score_adj`), ensuring low-priority containers are reclaimed first during host-wide memory pressure. Helios applies simple static directory allocations, lacking dynamic host coordination.
