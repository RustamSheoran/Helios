# Helios Custom Memory Allocator: High-Performance Intrusive Block & Slab Architecture

The Helios Memory Allocator (`helios-allocator`) is an **educational custom memory allocation engine** designed to explore heap structures and low-level memory mapping. It provides an alternative to standard system allocators by implementing Rust's `GlobalAlloc` trait. 

This document details the allocator's physical layout, metadata management, fragmentation mitigation algorithms, potential failure modes, and comparison with production-grade runtime alternatives.

---

## 1. Core Architectural Layout

The allocator operates on a hybrid design:
1. **The Page-Backed Block Allocator**: A dynamic heap manager that requests virtual memory pages from the kernel via anonymous `mmap` mappings, utilizing a doubly-linked intrusive list with a Best-Fit allocation strategy and physical coalescing on deallocation.
2. **The Slab Cache**: A constant-time, fixed-size slot allocator handling small, frequent allocations (16B, 32B, 64B, 128B, 256B, 512B, and 1024B classes). It intercepts requests falling within these bounds to bypass free-list traversals.

```
                         [ Global allocation request ]
                                      |
                     Is size <= 1024 bytes (Slab Class)?
                                      |
                   +------------------+------------------+
                   | YES                                 | NO
                   v                                     v
         [ Slab Cache Match ]                  [ Block Allocator ]
                   |                                     |
         Find target Slab page                 Traverse intrusive free list
                   |                                (Best-Fit search)
      Slot free?   |                                     |
    +--------------+-------------+                   Block found?
    | YES                        | NO             +------+------+
    |                            v                | YES         | NO
    v                    [ mmap new Slab ]        v             v
 [ Return slot ]                            [ Split block ]  [ mmap new page ]
                                                  |             |
                                            [ Return block]  [ Write header ]
```

---

## 2. Non-Goals

The following design objectives are explicitly classified as **Non-Goals** for `helios-allocator`:
* **NUMA-Aware Design**: It does not allocate memory from specific NUMA nodes or coordinate thread pools across hardware nodes.
* **Lock-Free Concurrency**: It does not implement complex atomic bucket arrays. Thread-safety is achieved using a coarse global lock, prioritizing structural simplicity.
* **Hardened Metadata Boundaries**: It does not isolate metadata in protected tables, prioritizing high performance and zero external allocations over exploit mitigation.
* **Production Replacement**: It is not intended as a drop-in replacement for production-grade allocators in industrial environments.

---

## 3. Potential Failure Modes

### 3.1 Metadata Corruption via Buffer Overflow
* **Why it happens**: Helios uses intrusive `BlockHeader` structures stored directly adjacent to the active payload. If a userspace program writes past the allocated boundary of its buffer, it will overwrite the adjacent block’s metadata.
* **Helios Behavior**: During subsequent splits or physical merges, the allocator will dereference the corrupted pointer fields (`next`, `prev`), resulting in immediate segfaults or arbitrary memory writes.
* **Mitigation in Production**: Production allocators (like `mimalloc`) isolate metadata in segregated, read-only memory pools distinct from user payloads.

### 3.2 Stale Pointer Reuse (Use-After-Free)
* **Why it happens**: Helios does not execute memory sanitization or automatic slot poisoning on deallocation. If a process retains a reference to a freed address, it can modify its payload.
* **Helios Behavior**: Since freed memory remains active in the intrusive list, writing to a stale pointer corrupts the block's free-list links or overwrites data returned to other threads, resulting in silent data corruption.

### 3.3 Severe Lock Contention Bottlenecks
* **Why it happens**: A coarse global lock serializes all allocation and deallocation requests.
* **Helios Behavior**: Under heavy thread concurrency (e.g., a multi-threaded shell pipeline), thread performance degrades as threads block waiting for the global lock, neutralizing CPU cores.

### 3.4 Fragmentation Amplification
* **Why it happens**: If the block allocator frequently splits large blocks and coalescing fails (due to active interleaved blocks), the free list becomes highly fragmented with tiny unusable blocks.
* **Helios Behavior**: Traversing long, fragmented lists degrades Best-Fit search times to a slow $O(N)$ linear traversal.

### 3.5 Allocator Recursion Hazards
* **Why it happens**: If the allocator’s internal diagnostic logging routines attempt to allocate memory dynamically (e.g., using standard formatting macros), the allocator will re-enter its own allocation path.
* **Helios Behavior**: Re-entering the path while the global lock is held results in an immediate, unrecoverable deadlock. Helios mitigates this by writing diagnostic messages using raw system calls (`libc::write`).

---

## 4. Real-World Systems Comparison

`helios-allocator` is modeled after foundational segregated-fit heap engines but differs significantly from modern production-grade allocators:

| Feature | Helios | jemalloc | tcmalloc | mimalloc |
| :--- | :--- | :--- | :--- | :--- |
| **Metadata Isolation** | None (Intrusive) | Guard pages & segregated chunks | Central Page Map | Segregated Page Tables |
| **Concurrency Model** | Global Coarse Lock | Segregated Arenas (Proportional to CPU cores) | Thread-Local Cache (tcache) | Lock-free Thread-Local Pages |
| **Small Object Allocation** | Fixed-slot Slab pages | Segregated bins | Thread-local free lists | Lock-free atomic slot maps |
| **Fragmentation Defense** | Immediate Coalescing | Active cache-line bin bins | Thread-local recycling | Temporal Page Reclaiming |
| **Observability** | Standard Error Logging | Advanced malloc stats dumps | Built-in heap profiling | None (silent execution) |

### What Helios Imitates:
* The core concept of **Slab Allocation** for constant-time small object requests, similar to `tcmalloc`’s thread-local bins.
* Doubly-linked free lists with **Best-Fit** splits and physical contiguous block coalescing.

### What Production Allocators Additionally Implement:
* **Thread-Local Storage (TLS)**: Modern allocators bypass locks for over $95\%$ of allocations by retrieving memory from thread-local pools (`tcache`), only acquiring global locks when these pools are exhausted.
* **NUMA-Aware Chunks**: Mapped memory is bound to the physical NUMA node matching the thread's execution core to maximize L1/L2 cache locality.
