# Helios Shell: Process Group Job Control & Pipeline Systems Architecture

The Helios Shell (`helios-shell`) is an **educational systems command-line interpreter** designed to explore POSIX job control, process group coordination, and low-level pipeline file descriptor plumbing. 

This document details the shell's parser architecture, signal routing semantics, terminal ownership handovers, potential failure modes, and comparison with production-grade Unix shells.

---

## 1. Process Group Semantics and Job Control

In a POSIX system, processes are organized in a hierarchy:
* **Process (PID)**: A single executing thread of execution with its own address space and descriptors.
* **Process Group (PGID)**: A collection of one or more processes. Signals sent to a process group are broadcast to all processes within that group.
* **Session (SID)**: A collection of process groups attached to a single controlling terminal (`/dev/tty`).

```
+-----------------------------------------------------------------------------------+
|                               SESSION (SID)                                       |
|                                                                                   |
|   +------------------------------------+   +----------------------------------+   |
|   |   FOREGROUND PROCESS GROUP (PGID)  |   |   BACKGROUND PROCESS GROUP (PGID)|   |
|   |                                    |   |                                  |   |
|   |  +------------+    +------------+  |   |  +------------+                  |   |
|   |  |   PID A    | -> |   PID B    |  |   |  |   PID C    |                  |   |
|   |  |  (Active)  |    |  (Active)  |  |   |  |  (Stopped) |                  |   |
|   |  +------------+    +------------+  |   |  +------------+                  |   |
|   +-----------------+------------------+   +----------------------------------+   |
|                     |                                                             |
|                     | tcsetpgrp() routes signals (SIGINT, SIGTSTP)                |
|                     v                                                             |
|             [ /dev/tty (Keyboard) ]                                               |
+-----------------------------------------------------------------------------------+
```

### 1.1 The Controlling Terminal and Foreground Handover
The terminal driver (`/dev/tty`) routes keyboard-generated signals (such as `Ctrl+C` for `SIGINT` or `Ctrl+Z` for `SIGTSTP`) exclusively to the process group designated as the **foreground process group**. The shell directs this handover using the `tcsetpgrp(fd, pgid)` system call:
1. **The Shell Session**: Upon initialization, the shell sets itself as the session leader and sets its own PGID as the foreground process group.
2. **Launching Foreground Jobs**: When a pipeline is spawned, the shell assigns all processes in the pipeline to a new process group. The first child’s PID is used as the PGID for the entire pipeline. The shell then calls `tcsetpgrp(libc::STDIN_FILENO, pgid)` to hand controlling terminal ownership to this new group.
3. **Restoring Terminal Control**: When the foreground job exits or is suspended, the shell catches this state transition via `waitpid` and immediately calls `tcsetpgrp(libc::STDIN_FILENO, shell_pgid)` to reclaim control of the keyboard input loop before prompting the user again.

---

## 2. Non-Goals

The following design objectives are explicitly classified as **Non-Goals** for `helios-shell`:
* **Complete POSIX Shell Compliance**: It does not implement complex POSIX shell scripting features (e.g., shell functions, control loops like `if`/`while`, or advanced parameter expansion).
* **Script Parsing and Execution**: It is designed solely as an interactive REPL command interface, not as an engine for executing `.sh` script files.
* **Terminal Emulation / Advanced Line Editing**: It relies on standard terminal line inputs, bypassing built-in terminal escape formatting or autocomplete interfaces.
* **Interactive Tooling Replacement**: It is not intended as a replacement for daily-driver shells (like `bash` or `zsh`).

---

## 3. Potential Failure Modes

### 3.1 Descriptor Leaks and EOF Deadlocks
* **Why it happens**: When plumbing pipes for a pipeline (e.g., `A | B`), the parent shell duplicates the pipe read/write file descriptors into the children before they call `execve`. If the parent shell fails to close its own local references to these pipe descriptors immediately after the fork, the reference count in the kernel remains $> 0$.
* **Systems Implication**: Even if Process A exits, Process B's blocking `read()` will never return `0` (EOF), as the kernel sees the parent shell still holding an open write reference. Process B will block indefinitely, resulting in an EOF deadlock.

### 3.2 Zombie Process Accumulation
* **Why it happens**: If background processes (launched via `&`) terminate while the shell is waiting for a foreground process, they remain in a zombie state until their status is reaped.
* **Systems Implication**: If the shell's `SIGCHLD` signal handler fails or runs with blocking waits, zombies accumulate, exhausting the kernel's PID pool. Helios mitigates this using a non-blocking `waitpid(-1, &status, WNOHANG)` loop, but any unhandled signal block will leak PIDs.

### 3.3 Terminal Control Races (PGID Races)
* **Why it happens**: After forking, a race exists between the parent shell and the child process to call `setpgid`. 
* **Systems Implication**: If the parent shell calls `tcsetpgrp` before the child has successfully executed `setpgid(child_pid, pgid)`, the system call will fail with `EPERM`. This leaves the terminal state ambiguous, and keyboard signals may be routed incorrectly, potentially terminating the shell itself.

### 3.4 Deadlocks on Shared Mutexes inside Signal Handlers
* **Why it happens**: If the shell's asynchronous `SIGCHLD` handler attempts to allocate memory or access shared tables protected by standard Rust mutexes, it can interrupt the main thread while a lock is held.
* **Systems Implication**: Attempting to acquire the same lock inside the handler results in an unrecoverable deadlock. Helios mitigates this by keeping its signal handlers strictly **async-signal-safe**, relying only on raw system calls and pre-allocated static arrays.

---

## 4. Real-World Unix Shells Comparison

`helios-shell` provides a bare-metal exploration of job control, but differs significantly from production command interpreters:

| Feature | Helios Shell | Bash | Zsh | Fish |
| :--- | :--- | :--- | :--- | :--- |
| **Grammar Parser** | Recursive-Descent AST | GNU Bison (LALR Parser) | Custom recursive-descent | Hand-written parser |
| **Pipeline Routing** | Basic Kernel Pipes | Advanced Subshell Forking | Coprocesses & multi-pipe | Thread-safe background pipes |
| **Job Control** | Basic `jobs`/`fg`/`bg` | Full POSIX job specs | Advanced job tables | Auto-background process reaping |
| **Autocompletion** | None | Programmable bash-completion | Rich Zsh completion modules | Interactive syntax suggestions |
| **Scripting Engine** | None | Full POSIX Bourne-Shell | Extended Bourne-Shell | Custom scripting syntax |

### What Helios Imitates:
* Standard **POSIX Process Group Handovers** via `tcsetpgrp` and `setpgid`.
* The **classic Unix pipeline plumbing** model using duplicate file descriptors (`dup2`) and kernel buffers.

### What Production Shells Additionally Implement:
* **Built-in Hashed Path Caching (`hash`)**: Instead of searching directories in `PATH` on every command, they cache path resolutions in a hash map.
* **Terminal Escape Sequence Handling**: Advanced terminal control protocols to manage syntax highlighting, cursor movements, and custom line editing buffers (like `readline`).
