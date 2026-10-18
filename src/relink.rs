//! The relinker: converts a Windows PE executable into a native Linux ELF.
//!
//! The produced file is an `ET_EXEC` ELF whose `PT_LOAD` segments are the PE headers and sections,
//! placed at the image's original base address with the right protections. The Linux kernel maps
//! them directly; there is no PE parsing for the code and data. A small generated segment holds the
//! program headers, the dynamic section and an entry stub which calls `__libc_start_main` with
//! `anyexe_relinked_main` from `libanyexe.so`. That function adopts the already-mapped image, binds its
//! imports to the reimplemented Windows libraries and starts it.

use crate::pe::{self, Layout, Pe, PeError};
use std::fmt;
use std::path::Path;

const PAGE: u64 = 4096;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE: u32 = 4;
const PT_PHDR: u32 = 6;
const PT_GNU_STACK: u32 = 0x6474_e551;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

pub const NOTE_NAME: &[u8] = b"ANYEXE\0";
pub const NOTE_TYPE: u32 = 0x4e4c_4501;

#[derive(Debug)]
pub enum RelinkError {
    Pe(PeError),
    Unsupported(String),
}

impl fmt::Display for RelinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RelinkError::Pe(e) => write!(f, "{e}"),
            RelinkError::Unsupported(m) => write!(f, "cannot relink: {m}"),
        }
    }
}

impl std::error::Error for RelinkError {}

impl From<PeError> for RelinkError {
    fn from(e: PeError) -> Self {
        RelinkError::Pe(e)
    }
}

pub struct Options {
    /// `DT_RUNPATH` entry for finding `libanyexe.so`.
    pub runpath: String,
    /// Name of the original program, recorded so the runtime can present it as `GetModuleFileName(NULL)`.
    pub original_name: String,
}

fn align_up(v: u64, a: u64) -> u64 {
    (v + a - 1) & !(a - 1)
}

fn put16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

struct Phdr {
    ty: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

fn write_phdr(b: &mut [u8], at: usize, p: &Phdr) {
    put32(b, at, p.ty);
    put32(b, at + 4, p.flags);
    put64(b, at + 8, p.offset);
    put64(b, at + 16, p.vaddr);
    put64(b, at + 24, p.vaddr);
    put64(b, at + 32, p.filesz);
    put64(b, at + 40, p.memsz);
    put64(b, at + 48, p.align);
}

fn section_flags(chars: u32) -> u32 {
    let mut f = PF_R;
    if chars & pe::SCN_MEM_WRITE != 0 {
        f |= PF_W;
    }
    if chars & (pe::SCN_MEM_EXECUTE | pe::SCN_CNT_CODE) != 0 {
        f |= PF_X;
    }
    f
}

/// Checks that an image can be mapped by the kernel at its preferred base.
pub fn check(pe: &Pe) -> Result<(), RelinkError> {
    if pe.machine != pe::MACHINE_AMD64 || !pe.pe32plus {
        return Err(RelinkError::Unsupported(format!("{} executables cannot run in a native x86-64 process", pe.arch_name())));
    }
    if pe.is_clr() {
        return Err(RelinkError::Unsupported(".NET managed executables need a CLR".into()));
    }
    if pe.is_dll() {
        return Err(RelinkError::Unsupported("this is a DLL, not a program".into()));
    }
    if pe.section_align < PAGE as u32 || !pe.section_align.is_power_of_two() {
        return Err(RelinkError::Unsupported("section alignment below the page size".into()));
    }
    if pe.image_base < 0x10000 || pe.image_base % 0x10000 != 0 || pe.image_base + pe.size_of_image as u64 >= 1 << 47 {
        return Err(RelinkError::Unsupported(format!("image base 0x{:x} cannot be mapped by the Linux kernel", pe.image_base)));
    }
    Ok(())
}

/// Builds the ELF file for `pe` (whose raw bytes are `pe_bytes`).
pub fn build(pe: &Pe, pe_bytes: &[u8], interp: &str, opts: &Options) -> Result<Vec<u8>, RelinkError> {
    check(pe)?;
    let base = pe.image_base;
    let image_end = base + align_up(pe.size_of_image as u64, PAGE);
    let meta_va = align_up(image_end, 0x10000);

    // Program headers: PHDR, INTERP, LOADs (headers + sections), meta LOADs, DYNAMIC, NOTE, GNU_STACK.
    let nload_pe = 1 + pe.sections.len();
    let phnum = 2 + nload_pe + 2 + 3;

    // ---- read-only meta segment contents -------------------------------------------------------------
    let phoff = 64usize;
    let mut cur = phoff + phnum * 56;
    let interp_off = cur;
    let interp_bytes = {
        let mut v = interp.as_bytes().to_vec();
        v.push(0);
        v
    };
    cur += interp_bytes.len();

    cur = align_up(cur as u64, 8) as usize;
    let note_off = cur;
    let mut desc = Vec::new();
    desc.extend_from_slice(&base.to_le_bytes());
    desc.extend_from_slice(&(pe.size_of_image as u64).to_le_bytes());
    desc.extend_from_slice(opts.original_name.as_bytes());
    desc.push(0);
    let note_len = 12 + align_up(NOTE_NAME.len() as u64, 4) as usize + align_up(desc.len() as u64, 4) as usize;
    cur += note_len;

    // dynamic string table
    let mut dynstr: Vec<u8> = vec![0];
    let mut add_str = |s: &str| -> u32 {
        let off = dynstr.len() as u32;
        dynstr.extend_from_slice(s.as_bytes());
        dynstr.push(0);
        off
    };
    let s_runtime = add_str("libanyexe.so");
    let s_libc = add_str("libc.so.6");
    let s_runpath = add_str(&opts.runpath);
    let s_main = add_str("anyexe_relinked_main");
    let s_start = add_str("__libc_start_main");

    cur = align_up(cur as u64, 8) as usize;
    let hash_off = cur;
    let nsyms = 3usize;
    let hash_len = 4 * (2 + 1 + nsyms);
    cur += hash_len;
    cur = align_up(cur as u64, 8) as usize;
    let dynsym_off = cur;
    cur += nsyms * 24;
    let dynstr_off = cur;
    cur += dynstr.len();
    cur = align_up(cur as u64, 8) as usize;
    let rela_off = cur;
    cur += 2 * 24;
    cur = align_up(cur as u64, 16) as usize;
    let stub_off = cur;
    let stub_len = 48usize;
    cur += stub_len;
    let ro_len = align_up(cur as u64, PAGE) as usize;

    // ---- writable meta segment: dynamic section and GOT --------------------------------------------------
    let rw_va = meta_va + ro_len as u64;
    let dynamic_off_in_rw = 0usize;
    let ndyn = 14usize;
    let got_off_in_rw = 0x200usize;

    let ro_va = |o: usize| meta_va + o as u64;
    let got_main_va = rw_va + got_off_in_rw as u64;
    let got_start_va = got_main_va + 8;

    let mut meta_ro = vec![0u8; ro_len];
    meta_ro[interp_off..interp_off + interp_bytes.len()].copy_from_slice(&interp_bytes);

    // note
    put32(&mut meta_ro, note_off, NOTE_NAME.len() as u32);
    put32(&mut meta_ro, note_off + 4, desc.len() as u32);
    put32(&mut meta_ro, note_off + 8, NOTE_TYPE);
    let mut p = note_off + 12;
    meta_ro[p..p + NOTE_NAME.len()].copy_from_slice(NOTE_NAME);
    p += align_up(NOTE_NAME.len() as u64, 4) as usize;
    meta_ro[p..p + desc.len()].copy_from_slice(&desc);

    // hash: nbucket=1, nchain=nsyms, bucket[0]=0, chains 0 (no symbols are defined here)
    put32(&mut meta_ro, hash_off, 1);
    put32(&mut meta_ro, hash_off + 4, nsyms as u32);

    // symbols: [0] null, [1] anyexe_relinked_main, [2] __libc_start_main (both undefined functions)
    for (i, name) in [(1usize, s_main), (2, s_start)] {
        let at = dynsym_off + i * 24;
        put32(&mut meta_ro, at, name);
        meta_ro[at + 4] = 0x12;
    }
    meta_ro[dynstr_off..dynstr_off + dynstr.len()].copy_from_slice(&dynstr);

    // relocations: GOT slots bound by the dynamic linker
    for (i, (slot, sym)) in [(got_main_va, 1u64), (got_start_va, 2u64)].iter().enumerate() {
        let at = rela_off + i * 24;
        put64(&mut meta_ro, at, *slot);
        put64(&mut meta_ro, at + 8, (sym << 32) | 6);
    }

    // entry stub (the sequence a C runtime start file uses, with main taken from the GOT)
    let stub: Vec<u8> = {
        let mut c: Vec<u8> = vec![
            0x31, 0xed, // xor ebp, ebp
            0x49, 0x89, 0xd1, // mov r9, rdx
            0x5e, // pop rsi
            0x48, 0x89, 0xe2, // mov rdx, rsp
            0x48, 0x83, 0xe4, 0xf0, // and rsp, -16
            0x50, // push rax
            0x54, // push rsp
            0x45, 0x31, 0xc0, // xor r8d, r8d
            0x31, 0xc9, // xor ecx, ecx
        ];
        let stub_va = ro_va(stub_off);
        let rip_after = |len_so_far: usize, ins_len: usize| stub_va + (len_so_far + ins_len) as u64;
        // mov rdi, [rip+rel32]
        let rel = (got_main_va as i64 - rip_after(c.len(), 7) as i64) as i32;
        c.extend_from_slice(&[0x48, 0x8b, 0x3d]);
        c.extend_from_slice(&rel.to_le_bytes());
        // call [rip+rel32]
        let rel = (got_start_va as i64 - rip_after(c.len(), 6) as i64) as i32;
        c.extend_from_slice(&[0xff, 0x15]);
        c.extend_from_slice(&rel.to_le_bytes());
        c.push(0xf4); // hlt
        c
    };
    meta_ro[stub_off..stub_off + stub.len()].copy_from_slice(&stub);
    debug_assert!(stub.len() <= stub_len);

    let mut meta_rw = vec![0u8; PAGE as usize];
    let dyn_entries: [(u64, u64); 14] = [
        (1, s_runtime as u64),
        (1, s_libc as u64),
        (29, s_runpath as u64),
        (4, ro_va(hash_off)),
        (5, ro_va(dynstr_off)),
        (6, ro_va(dynsym_off)),
        (10, dynstr.len() as u64),
        (11, 24),
        (7, ro_va(rela_off)),
        (8, 2 * 24),
        (9, 24),
        (30, 0x9),
        (21, 0),
        (0, 0),
    ];
    debug_assert_eq!(dyn_entries.len(), ndyn);
    for (i, (tag, val)) in dyn_entries.iter().enumerate() {
        put64(&mut meta_rw, dynamic_off_in_rw + i * 16, *tag);
        put64(&mut meta_rw, dynamic_off_in_rw + i * 16 + 8, *val);
    }
    let dynamic_va = rw_va + dynamic_off_in_rw as u64;
    let dynamic_len = (ndyn * 16) as u64;

    // ---- file layout ----------------------------------------------------------------------------------------
    let mut file: Vec<u8> = Vec::new();
    file.extend_from_slice(&meta_ro);
    let rw_file_off = file.len() as u64;
    file.extend_from_slice(&meta_rw);

    let mut phdrs: Vec<Phdr> = Vec::new();
    phdrs.push(Phdr { ty: PT_PHDR, flags: PF_R, offset: phoff as u64, vaddr: ro_va(phoff), filesz: (phnum * 56) as u64, memsz: (phnum * 56) as u64, align: 8 });
    phdrs.push(Phdr {
        ty: PT_INTERP,
        flags: PF_R,
        offset: interp_off as u64,
        vaddr: ro_va(interp_off),
        filesz: interp_bytes.len() as u64,
        memsz: interp_bytes.len() as u64,
        align: 1,
    });

    // headers segment
    let hdr_len = (pe.size_of_headers as usize).min(pe_bytes.len());
    let hdr_off = file.len() as u64;
    file.extend_from_slice(&pe_bytes[..hdr_len]);
    file.resize(align_up(file.len() as u64, PAGE) as usize, 0);
    phdrs.push(Phdr {
        ty: PT_LOAD,
        flags: PF_R,
        offset: hdr_off,
        vaddr: base,
        filesz: hdr_len as u64,
        memsz: align_up(pe.size_of_headers as u64, PAGE),
        align: PAGE,
    });

    let mut sections: Vec<&pe::Section> = pe.sections.iter().collect();
    sections.sort_by_key(|s| s.vaddr);
    for s in sections {
        let extent = s.extent() as u64;
        let raw = (s.raw_size as u64).min(extent);
        let off = file.len() as u64;
        if raw > 0 {
            let src = pe_bytes
                .get(s.raw_off as usize..s.raw_off as usize + raw as usize)
                .ok_or(RelinkError::Pe(PeError::Truncated("section raw data")))?;
            file.extend_from_slice(src);
        }
        file.resize(align_up(file.len() as u64, PAGE) as usize, 0);
        phdrs.push(Phdr {
            ty: PT_LOAD,
            flags: section_flags(s.chars),
            offset: off,
            vaddr: base + s.vaddr as u64,
            filesz: raw,
            memsz: align_up(extent, PAGE),
            align: PAGE,
        });
    }

    phdrs.push(Phdr { ty: PT_LOAD, flags: PF_R | PF_X, offset: 0, vaddr: meta_va, filesz: ro_len as u64, memsz: ro_len as u64, align: PAGE });
    phdrs.push(Phdr { ty: PT_LOAD, flags: PF_R | PF_W, offset: rw_file_off, vaddr: rw_va, filesz: PAGE, memsz: PAGE, align: PAGE });
    phdrs.push(Phdr { ty: PT_DYNAMIC, flags: PF_R | PF_W, offset: rw_file_off + dynamic_off_in_rw as u64, vaddr: dynamic_va, filesz: dynamic_len, memsz: dynamic_len, align: 8 });
    phdrs.push(Phdr { ty: PT_NOTE, flags: PF_R, offset: note_off as u64, vaddr: ro_va(note_off), filesz: note_len as u64, memsz: note_len as u64, align: 4 });
    phdrs.push(Phdr { ty: PT_GNU_STACK, flags: PF_R | PF_W, offset: 0, vaddr: 0, filesz: 0, memsz: 0, align: 16 });
    debug_assert_eq!(phdrs.len(), phnum);

    // ELF header
    file[0..4].copy_from_slice(b"\x7fELF");
    file[4] = 2;
    file[5] = 1;
    file[6] = 1;
    put16(&mut file, 16, 2);
    put16(&mut file, 18, 62);
    put32(&mut file, 20, 1);
    put64(&mut file, 24, ro_va(stub_off));
    put64(&mut file, 32, phoff as u64);
    put16(&mut file, 52, 64);
    put16(&mut file, 54, 56);
    put16(&mut file, 56, phnum as u16);
    put16(&mut file, 58, 64);
    for (i, p) in phdrs.iter().enumerate() {
        write_phdr(&mut file, phoff + i * 56, p);
    }
    Ok(file)
}

/// Reads the interpreter path (`PT_INTERP`) of an ELF file, e.g. this program's own binary.
pub fn interpreter_of(path: &Path) -> Option<String> {
    let b = std::fs::read(path).ok()?;
    if b.len() < 64 || &b[0..4] != b"\x7fELF" {
        return None;
    }
    let phoff = u64::from_le_bytes(b[32..40].try_into().ok()?) as usize;
    let phentsize = u16::from_le_bytes(b[54..56].try_into().ok()?) as usize;
    let phnum = u16::from_le_bytes(b[56..58].try_into().ok()?) as usize;
    for i in 0..phnum {
        let at = phoff + i * phentsize;
        let ty = u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?);
        if ty == PT_INTERP {
            let off = u64::from_le_bytes(b[at + 8..at + 16].try_into().ok()?) as usize;
            let sz = u64::from_le_bytes(b[at + 32..at + 40].try_into().ok()?) as usize;
            let s = b.get(off..off + sz)?;
            return Some(String::from_utf8_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]).into_owned());
        }
    }
    None
}

pub fn parse_for_relink(bytes: &[u8]) -> Result<Pe<'_>, RelinkError> {
    Ok(Pe::parse(bytes, Layout::File)?)
}

/// Finds the descriptor note in the running ELF's program headers (used by the runtime).
/// Returns `(image_base, image_size, original_name)`.
pub unsafe fn find_note() -> Option<(u64, u64, String)> {
    let phdr = libc::getauxval(libc::AT_PHDR) as *const u8;
    let phnum = libc::getauxval(libc::AT_PHNUM) as usize;
    if phdr.is_null() {
        return None;
    }
    for i in 0..phnum {
        let p = phdr.add(i * 56);
        if (p as *const u32).read_unaligned() != PT_NOTE {
            continue;
        }
        let vaddr = (p.add(16) as *const u64).read_unaligned() as *const u8;
        let namesz = (vaddr as *const u32).read_unaligned() as usize;
        let descsz = (vaddr.add(4) as *const u32).read_unaligned() as usize;
        let ty = (vaddr.add(8) as *const u32).read_unaligned();
        if ty != NOTE_TYPE {
            continue;
        }
        let desc = vaddr.add(12 + align_up(namesz as u64, 4) as usize);
        if descsz < 17 {
            return None;
        }
        let base = (desc as *const u64).read_unaligned();
        let size = (desc.add(8) as *const u64).read_unaligned();
        let name_bytes = std::slice::from_raw_parts(desc.add(16), descsz - 16);
        let name = String::from_utf8_lossy(&name_bytes[..name_bytes.iter().position(|&c| c == 0).unwrap_or(name_bytes.len())]).into_owned();
        return Some((base, size, name));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::testutil::sample_pe;

    #[test]
    fn builds_a_well_formed_elf() {
        let bytes = sample_pe();
        let pe = Pe::parse(&bytes, Layout::File).unwrap();
        let elf = build(&pe, &bytes, "/lib64/ld-linux-x86-64.so.2", &Options { runpath: "$ORIGIN".into(), original_name: "sample.exe".into() }).unwrap();
        assert_eq!(&elf[0..4], b"\x7fELF");
        assert_eq!(u16::from_le_bytes([elf[16], elf[17]]), 2);
        assert_eq!(u16::from_le_bytes([elf[18], elf[19]]), 62);
        let phnum = u16::from_le_bytes([elf[56], elf[57]]) as usize;
        let phoff = u64::from_le_bytes(elf[32..40].try_into().unwrap()) as usize;
        let mut loads = Vec::new();
        for i in 0..phnum {
            let at = phoff + i * 56;
            let ty = u32::from_le_bytes(elf[at..at + 4].try_into().unwrap());
            if ty == PT_LOAD {
                let off = u64::from_le_bytes(elf[at + 8..at + 16].try_into().unwrap());
                let va = u64::from_le_bytes(elf[at + 16..at + 24].try_into().unwrap());
                assert_eq!(off % PAGE, va % PAGE, "file offset must match the address modulo the page size");
                loads.push(va);
            }
        }
        let mut sorted = loads.clone();
        sorted.sort();
        assert_eq!(loads, sorted, "PT_LOAD segments must be in ascending address order");
        assert!(loads.contains(&0x1_4000_0000));
        assert!(loads.contains(&0x1_4000_1000));
        assert!(loads.contains(&0x1_4000_2000));
    }

    #[test]
    fn rejects_32bit_and_dlls() {
        let mut bytes = sample_pe();
        crate::pe::testutil::put16(&mut bytes, 0x84, 0x14c);
        let pe = Pe::parse(&bytes, Layout::File).unwrap();
        assert!(check(&pe).is_err());
    }
}
