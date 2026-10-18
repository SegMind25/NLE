//! Stubs for imports that have no native implementation.
//!
//! An unresolved import slot points at a tiny generated stub. Calling it stops the program
//! with the name of the missing API: unsupported behaviour fails loudly rather than returning
//! a made-up success value.

use std::arch::naked_asm;
use std::sync::Mutex;

const STUB_SIZE: usize = 32;

static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[unsafe(naked)]
unsafe extern "C" fn trap_common() -> ! {
    naked_asm!(
        "mov edi, eax",
        "mov rsi, [rsp]",
        "and rsp, -16",
        "call {report}",
        report = sym trap_report,
    );
}

extern "C" fn trap_report(idx: u32, ret_addr: usize) -> ! {
    let name = NAMES.lock().map(|n| n.get(idx as usize).cloned().unwrap_or_default()).unwrap_or_default();
    let place = match crate::loader::module_from_addr(ret_addr) {
        Some(m) => format!("{}+0x{:x}", m.name, ret_addr - m.base),
        None => format!("0x{ret_addr:x}"),
    };
    unsafe { libc::fflush(std::ptr::null_mut()) };
    eprintln!("anyexe: fatal: the program called a Windows API that is not implemented: {name}");
    eprintln!("        called from {place}");
    eprintln!("        `anyexe inspect <file>` lists everything missing; `--backend wine` runs it under Wine instead");
    unsafe { libc::_exit(70) }
}

/// Generates one stub per label and returns their addresses.
pub fn make_stubs(labels: &[String]) -> Vec<usize> {
    if labels.is_empty() {
        return Vec::new();
    }
    let first_idx = {
        let mut n = NAMES.lock().unwrap();
        let first = n.len();
        n.extend(labels.iter().cloned());
        first
    };
    let size = (labels.len() * STUB_SIZE + 4095) & !4095;
    unsafe {
        let p = libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0);
        if p == libc::MAP_FAILED {
            eprintln!("anyexe: cannot allocate import stubs");
            libc::_exit(70);
        }
        let target = trap_common as *const () as usize as u64;
        let mut out = Vec::with_capacity(labels.len());
        for i in 0..labels.len() {
            let s = (p as *mut u8).add(i * STUB_SIZE);
            let idx = (first_idx + i) as u32;
            // mov eax, imm32 ; movabs r10, target ; jmp r10
            *s = 0xb8;
            std::ptr::copy_nonoverlapping(idx.to_le_bytes().as_ptr(), s.add(1), 4);
            *s.add(5) = 0x49;
            *s.add(6) = 0xba;
            std::ptr::copy_nonoverlapping(target.to_le_bytes().as_ptr(), s.add(7), 8);
            *s.add(15) = 0x41;
            *s.add(16) = 0xff;
            *s.add(17) = 0xe2;
            out.push(s as usize);
        }
        libc::mprotect(p, size, libc::PROT_READ | libc::PROT_EXEC);
        out
    }
}
