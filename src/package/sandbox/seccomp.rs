#[cfg(target_os = "linux")]
mod imp {
    use std::sync::Arc;

    const BPF_LD: u16 = 0x00;
    const BPF_JMP: u16 = 0x05;
    const BPF_RET: u16 = 0x06;
    const BPF_W: u16 = 0x00;
    const BPF_ABS: u16 = 0x20;
    const BPF_JEQ: u16 = 0x10;
    const BPF_JGE: u16 = 0x30;
    const BPF_JGT: u16 = 0x20;
    const BPF_K: u16 = 0x00;

    const SECCOMP_RET_KILL_PROCESS: u32 = 0x80000000;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff0000;

    #[cfg(target_arch = "x86_64")]
    const AUDIT_ARCH: u32 = 0xc000003e;
    #[cfg(target_arch = "aarch64")]
    const AUDIT_ARCH: u32 = 0xc00000b7;
    #[cfg(target_arch = "arm")]
    const AUDIT_ARCH: u32 = 0x40000028;
    #[cfg(target_arch = "x86")]
    const AUDIT_ARCH: u32 = 0x40000006;
    #[cfg(target_arch = "riscv64")]
    const AUDIT_ARCH: u32 = 0xc00000f3;

    #[repr(C)]
    struct sock_filter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }

    #[repr(C)]
    struct sock_fprog {
        len: u16,
        filter: *const sock_filter,
    }

    fn build_bpf_program() -> Vec<sock_filter> {
        let ranges: &[(u32, u32)] = &[
            (0, 17),
            (22, 25),
            (35, 43),
            (47, 48),
            (56, 63),
            (72, 293),
        ];

        let n = ranges.len();
        let mut prog = Vec::new();

        // Architecture check (3 instructions)
        prog.push(sock_filter {
            code: BPF_LD | BPF_W | BPF_ABS,
            jt: 0,
            jf: 0,
            k: 4,
        });
        prog.push(sock_filter {
            code: BPF_JMP | BPF_JEQ | BPF_K,
            jt: 1,
            jf: 0,
            k: AUDIT_ARCH,
        });
        prog.push(sock_filter {
            code: BPF_RET | BPF_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_KILL_PROCESS,
        });

        // Load syscall number
        prog.push(sock_filter {
            code: BPF_LD | BPF_W | BPF_ABS,
            jt: 0,
            jf: 0,
            k: 0,
        });

        // Each range produces 3 instructions: JGE, JGT, RET ALLOW
        // pc layout for range i (0-indexed):
        //   pc = 4 + 3*i     : JGE #lo, 0, KILL_OFFSET
        //   pc = 5 + 3*i     : JGT #hi, NEXT_OFFSET, 0
        //   pc = 6 + 3*i     : RET ALLOW
        // KILL is at pc = 4 + 3*n
        let kill_pc: u16 = (4 + 3 * n) as u16;

        for (i, &(lo, hi)) in ranges.iter().enumerate() {
            let jge_pc: u16 = (4 + 3 * i) as u16;

            // JGE: if A >= lo, go to JGT (next instruction); else go to KILL
            let jge_jt: u8 = 0;
            let jge_jf: u8 = (kill_pc - jge_pc - 1) as u8;

            prog.push(sock_filter {
                code: BPF_JMP | BPF_JGE | BPF_K,
                jt: jge_jt,
                jf: jge_jf,
                k: lo,
            });

            // JGT: if A > hi, go to next range or KILL; else go to ALLOW
            let jgt_jt: u8 = 1;
            let jgt_jf: u8 = 0;

            prog.push(sock_filter {
                code: BPF_JMP | BPF_JGT | BPF_K,
                jt: jgt_jt,
                jf: jgt_jf,
                k: hi,
            });

            // ALLOW
            prog.push(sock_filter {
                code: BPF_RET | BPF_K,
                jt: 0,
                jf: 0,
                k: SECCOMP_RET_ALLOW,
            });
        }

        // Final KILL
        prog.push(sock_filter {
            code: BPF_RET | BPF_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_KILL_PROCESS,
        });

        prog
    }

    pub struct SeccompProfile {
        filter: Arc<Vec<sock_filter>>,
    }

    impl SeccompProfile {
        pub fn default_allowlist() -> Self {
            let prog = build_bpf_program();
            Self {
                filter: Arc::new(prog),
            }
        }

        pub fn apply(&self) -> Result<(), std::io::Error> {
            let fprog = sock_fprog {
                len: self.filter.len() as u16,
                filter: self.filter.as_ptr(),
            };

            let ret = unsafe {
                libc::prctl(
                    libc::PR_SET_SECCOMP,
                    libc::SECCOMP_MODE_FILTER,
                    &fprog as *const sock_fprog,
                )
            };
            if ret != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Clone for SeccompProfile {
        fn clone(&self) -> Self {
            Self {
                filter: Arc::clone(&self.filter),
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    pub struct SeccompProfile;

    impl SeccompProfile {
        pub fn default_allowlist() -> Self {
            Self
        }

        pub fn apply(&self) -> Result<(), std::io::Error> {
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "seccomp not supported on this platform",
            ))
        }
    }

    impl Clone for SeccompProfile {
        fn clone(&self) -> Self {
            Self
        }
    }
}

pub use imp::SeccompProfile;
