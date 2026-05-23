use helios_shared::{log_debug, log_error, HeliosError, Result};

// Low-level BPF structure declarations
#[repr(C)]
struct SockFilter {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

#[repr(C)]
struct SockFprog {
    pub len: u16,
    pub filter: *const SockFilter,
}

// BPF Instruction opcodes and constants
const BPF_LD: u16 = 0x00;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JMP: u16 = 0x05;
const BPF_RET: u16 = 0x06;
const BPF_K: u16 = 0x00;
const BPF_JEQ: u16 = 0x15;

const SECCOMP_RET_ALLOW: u32 = 0x7fff0000;
const SECCOMP_RET_ERRNO: u32 = 0x00050000; // Returns errno
const EPERM: u32 = 1; // Error code for Operation Not Permitted

const AUDIT_ARCH_X86_64: u32 = 0xc000003e;

// Syscall numbers on x86_64
const SYS_REBOOT: u32 = 169;
const SYS_KEXEC_LOAD: u32 = 246;
const SYS_SYSLOG: u32 = 103;

/// Installs custom Seccomp system call filters inside the container process.
pub unsafe fn install_seccomp_filter() -> Result<()> {
    // Construct Berkeley Packet Filter instructions:
    let filter_instructions = [
        // 1. Load the architecture field from Seccomp data offset 4
        SockFilter {
            code: BPF_LD | BPF_W | BPF_ABS,
            jt: 0,
            jf: 0,
            k: 4, // Offset of arch in seccomp_data
        },
        // 2. Validate it is AUDIT_ARCH_X86_64. If yes, jump 1 step down, otherwise return EPERM
        SockFilter {
            code: BPF_JMP | BPF_JEQ | BPF_K,
            jt: 1,
            jf: 0,
            k: AUDIT_ARCH_X86_64,
        },
        SockFilter {
            code: BPF_RET | BPF_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ERRNO | EPERM,
        },
        // 3. Load the syscall number field from Seccomp data offset 0
        SockFilter {
            code: BPF_LD | BPF_W | BPF_ABS,
            jt: 0,
            jf: 0,
            k: 0, // Offset of nr in seccomp_data
        },
        // 4. If reboot syscall, jump to return EPERM block
        SockFilter {
            code: BPF_JMP | BPF_JEQ | BPF_K,
            jt: 3, // Jumps to the return EPERM instruction
            jf: 0,
            k: SYS_REBOOT,
        },
        // 5. If kexec_load syscall, jump to return EPERM block
        SockFilter {
            code: BPF_JMP | BPF_JEQ | BPF_K,
            jt: 2,
            jf: 0,
            k: SYS_KEXEC_LOAD,
        },
        // 6. If syslog (kernel logs) syscall, jump to return EPERM block
        SockFilter {
            code: BPF_JMP | BPF_JEQ | BPF_K,
            jt: 1,
            jf: 0,
            k: SYS_SYSLOG,
        },
        // 7. Success case: allow all other syscalls
        SockFilter {
            code: BPF_RET | BPF_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ALLOW,
        },
        // 8. Blocked case: return EPERM
        SockFilter {
            code: BPF_RET | BPF_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ERRNO | EPERM,
        },
    ];

    let program = SockFprog {
        len: filter_instructions.len() as u16,
        filter: filter_instructions.as_ptr(),
    };

    // 1. Set PR_SET_NO_NEW_PRIVS to allow non-root to apply seccomp (essential security step!)
    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0 {
        let errno = *libc::__errno_location();
        return Err(HeliosError::SyscallError("prctl(PR_SET_NO_NEW_PRIVS)".to_string(), errno));
    }

    // 2. Load the custom Seccomp filter program
    if libc::prctl(
        libc::PR_SET_SECCOMP,
        libc::SECCOMP_MODE_FILTER,
        &program as *const SockFprog as usize,
        0,
        0,
    ) < 0 {
        let errno = *libc::__errno_location();
        return Err(HeliosError::SyscallError("prctl(PR_SET_SECCOMP)".to_string(), errno));
    }

    log_debug!("container", "Seccomp: successfully loaded BPF filter (reboot, kexec_load, syslog blocked)");
    Ok(())
}
