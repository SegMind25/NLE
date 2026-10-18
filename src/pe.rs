//! Bounds-checked PE/COFF parser.
//!
//! The parser works on two views of the same format: the raw file (`Layout::File`,
//! RVAs are translated through the section table) and a mapped image
//! (`Layout::Image`, RVA == offset). Every read is range-checked because the input
//! is untrusted.

use std::fmt;

pub const MACHINE_I386: u16 = 0x014c;
pub const MACHINE_AMD64: u16 = 0x8664;
pub const MACHINE_ARM64: u16 = 0xaa64;

pub const SUBSYSTEM_GUI: u16 = 2;
pub const SUBSYSTEM_CUI: u16 = 3;

pub const FILE_DLL: u16 = 0x2000;
pub const FILE_EXECUTABLE: u16 = 0x0002;

pub const SCN_MEM_EXECUTE: u32 = 0x2000_0000;
pub const SCN_MEM_READ: u32 = 0x4000_0000;
pub const SCN_MEM_WRITE: u32 = 0x8000_0000;
pub const SCN_CNT_CODE: u32 = 0x0000_0020;

pub const DIR_EXPORT: usize = 0;
pub const DIR_IMPORT: usize = 1;
pub const DIR_EXCEPTION: usize = 3;
pub const DIR_BASERELOC: usize = 5;
pub const DIR_TLS: usize = 9;
pub const DIR_DELAY_IMPORT: usize = 13;
pub const DIR_CLR: usize = 14;

pub const REL_ABSOLUTE: u8 = 0;
pub const REL_HIGH: u8 = 1;
pub const REL_LOW: u8 = 2;
pub const REL_HIGHLOW: u8 = 3;
pub const REL_DIR64: u8 = 10;

const MAX_IMPORT_DLLS: usize = 4096;
const MAX_THUNKS_PER_DLL: usize = 1 << 16;
const MAX_EXPORTS: usize = 1 << 17;
const MAX_STRING: usize = 4096;

#[derive(Debug)]
pub enum PeError {
    NotPe(&'static str),
    Truncated(&'static str),
    Unsupported(String),
}

impl fmt::Display for PeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeError::NotPe(m) => write!(f, "not a PE file: {m}"),
            PeError::Truncated(m) => write!(f, "truncated or corrupt PE file: {m}"),
            PeError::Unsupported(m) => write!(f, "unsupported PE file: {m}"),
        }
    }
}

impl std::error::Error for PeError {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layout {
    File,
    Image,
}

#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    pub vsize: u32,
    pub vaddr: u32,
    pub raw_size: u32,
    pub raw_off: u32,
    pub chars: u32,
}

impl Section {
    pub fn executable(&self) -> bool {
        self.chars & (SCN_MEM_EXECUTE | SCN_CNT_CODE) != 0
    }
    pub fn writable(&self) -> bool {
        self.chars & SCN_MEM_WRITE != 0
    }
    pub fn readable(&self) -> bool {
        self.chars & SCN_MEM_READ != 0
    }
    /// Bytes of address space the section occupies in the loaded image.
    pub fn extent(&self) -> u32 {
        self.vsize.max(self.raw_size)
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct DataDir {
    pub rva: u32,
    pub size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportName {
    Name { name: String, hint: u16 },
    Ordinal(u16),
}

#[derive(Clone, Debug)]
pub struct Import {
    pub dll: String,
    pub name: ImportName,
    pub delay: bool,
    /// RVA of the IAT slot the loader must fill.
    pub iat_rva: u32,
}

impl Import {
    pub fn display(&self) -> String {
        match &self.name {
            ImportName::Name { name, .. } => format!("{}!{}", self.dll, name),
            ImportName::Ordinal(o) => format!("{}!#{}", self.dll, o),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Export {
    pub name: Option<String>,
    pub ordinal: u32,
    pub rva: u32,
    pub forwarder: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TlsDir {
    pub start_va: u64,
    pub end_va: u64,
    pub index_va: u64,
    pub callbacks_va: u64,
    pub zero_fill: u32,
    pub chars: u32,
}

pub struct Pe<'a> {
    data: &'a [u8],
    layout: Layout,
    pub machine: u16,
    pub pe32plus: bool,
    pub file_chars: u16,
    pub subsystem: u16,
    pub dll_chars: u16,
    pub image_base: u64,
    pub entry_rva: u32,
    pub section_align: u32,
    pub file_align: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub stack_reserve: u64,
    pub sections: Vec<Section>,
    pub dirs: [DataDir; 16],
}

fn rd_u16(d: &[u8], off: usize) -> Option<u16> {
    let b = d.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

fn rd_u32(d: &[u8], off: usize) -> Option<u32> {
    let b = d.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn rd_u64(d: &[u8], off: usize) -> Option<u64> {
    let b = d.get(off..off.checked_add(8)?)?;
    Some(u64::from_le_bytes(b.try_into().ok()?))
}

impl<'a> Pe<'a> {
    pub fn parse(data: &'a [u8], layout: Layout) -> Result<Pe<'a>, PeError> {
        if data.len() < 0x40 || &data[0..2] != b"MZ" {
            return Err(PeError::NotPe("missing MZ header"));
        }
        let e_lfanew = rd_u32(data, 0x3c).ok_or(PeError::Truncated("e_lfanew"))? as usize;
        let sig = data
            .get(e_lfanew..e_lfanew.checked_add(4).ok_or(PeError::Truncated("pe offset"))?)
            .ok_or(PeError::Truncated("PE signature"))?;
        if sig != b"PE\0\0" {
            return Err(PeError::NotPe("missing PE signature"));
        }
        let coff = e_lfanew + 4;
        let machine = rd_u16(data, coff).ok_or(PeError::Truncated("COFF header"))?;
        let nsections = rd_u16(data, coff + 2).ok_or(PeError::Truncated("COFF header"))? as usize;
        let opt_size = rd_u16(data, coff + 16).ok_or(PeError::Truncated("COFF header"))? as usize;
        let file_chars = rd_u16(data, coff + 18).ok_or(PeError::Truncated("COFF header"))?;
        let opt = coff + 20;
        if opt + opt_size > data.len() || opt_size < 96 {
            return Err(PeError::Truncated("optional header"));
        }
        let magic = rd_u16(data, opt).ok_or(PeError::Truncated("optional header"))?;
        let pe32plus = match magic {
            0x20b => true,
            0x10b => false,
            _ => return Err(PeError::NotPe("unknown optional header magic")),
        };
        let u32o = |o: usize| rd_u32(data, opt + o).ok_or(PeError::Truncated("optional header"));
        let u16o = |o: usize| rd_u16(data, opt + o).ok_or(PeError::Truncated("optional header"));
        let entry_rva = u32o(16)?;
        let (image_base, stack_reserve, ndirs_off, dirs_off) = if pe32plus {
            (
                rd_u64(data, opt + 24).ok_or(PeError::Truncated("optional header"))?,
                rd_u64(data, opt + 72).ok_or(PeError::Truncated("optional header"))?,
                108usize,
                112usize,
            )
        } else {
            (u32o(28)? as u64, u32o(72)? as u64, 92usize, 96usize)
        };
        let section_align = u32o(32)?;
        let file_align = u32o(36)?;
        let size_of_image = u32o(56)?;
        let size_of_headers = u32o(60)?;
        let subsystem = u16o(68)?;
        let dll_chars = u16o(70)?;
        let ndirs = (u32o(ndirs_off)? as usize).min(16);
        let mut dirs = [DataDir::default(); 16];
        for (i, d) in dirs.iter_mut().enumerate().take(ndirs) {
            let o = dirs_off + i * 8;
            if o + 8 > opt_size {
                break;
            }
            d.rva = u32o(o)?;
            d.size = u32o(o + 4)?;
        }

        let sec_tab = opt + opt_size;
        let mut sections = Vec::with_capacity(nsections.min(96));
        for i in 0..nsections.min(96) {
            let o = sec_tab + i * 40;
            let raw = data.get(o..o + 40).ok_or(PeError::Truncated("section table"))?;
            let end = raw[..8].iter().position(|&b| b == 0).unwrap_or(8);
            sections.push(Section {
                name: String::from_utf8_lossy(&raw[..end]).into_owned(),
                vsize: rd_u32(raw, 8).unwrap(),
                vaddr: rd_u32(raw, 12).unwrap(),
                raw_size: rd_u32(raw, 16).unwrap(),
                raw_off: rd_u32(raw, 20).unwrap(),
                chars: rd_u32(raw, 36).unwrap(),
            });
        }

        Ok(Pe {
            data,
            layout,
            machine,
            pe32plus,
            file_chars,
            subsystem,
            dll_chars,
            image_base,
            entry_rva,
            section_align,
            file_align,
            size_of_image,
            size_of_headers,
            stack_reserve,
            sections,
            dirs,
        })
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn is_dll(&self) -> bool {
        self.file_chars & FILE_DLL != 0
    }

    pub fn is_clr(&self) -> bool {
        self.dirs[DIR_CLR].rva != 0
    }

    pub fn has_relocs(&self) -> bool {
        self.dirs[DIR_BASERELOC].rva != 0 && self.dirs[DIR_BASERELOC].size != 0
    }

    pub fn arch_name(&self) -> &'static str {
        match self.machine {
            MACHINE_AMD64 => "x86-64",
            MACHINE_I386 => "x86 (32-bit)",
            MACHINE_ARM64 => "ARM64",
            _ => "unknown",
        }
    }

    pub fn subsystem_name(&self) -> &'static str {
        match self.subsystem {
            1 => "native",
            2 => "windows GUI",
            3 => "windows console",
            5 => "OS/2 console",
            7 => "POSIX console",
            9 => "windows CE",
            10 => "EFI application",
            14 => "xbox",
            _ => "unknown",
        }
    }

    pub fn rva_to_offset(&self, rva: u32) -> Option<usize> {
        match self.layout {
            Layout::Image => Some(rva as usize),
            Layout::File => {
                if rva < self.size_of_headers.min(self.sections.first().map_or(u32::MAX, |s| s.vaddr)) {
                    return Some(rva as usize);
                }
                for s in &self.sections {
                    let delta = rva.checked_sub(s.vaddr)?;
                    if rva >= s.vaddr && delta < s.raw_size {
                        return (s.raw_off as usize).checked_add(delta as usize);
                    }
                }
                None
            }
        }
    }

    pub fn slice(&self, rva: u32, len: usize) -> Option<&'a [u8]> {
        let off = self.rva_to_offset(rva)?;
        self.data.get(off..off.checked_add(len)?)
    }

    pub fn u16_rva(&self, rva: u32) -> Option<u16> {
        rd_u16(self.data, self.rva_to_offset(rva)?)
    }

    pub fn u32_rva(&self, rva: u32) -> Option<u32> {
        rd_u32(self.data, self.rva_to_offset(rva)?)
    }

    pub fn u64_rva(&self, rva: u32) -> Option<u64> {
        rd_u64(self.data, self.rva_to_offset(rva)?)
    }

    pub fn cstr(&self, rva: u32) -> Option<String> {
        let off = self.rva_to_offset(rva)?;
        let tail = self.data.get(off..)?;
        let tail = &tail[..tail.len().min(MAX_STRING)];
        let end = tail.iter().position(|&b| b == 0)?;
        Some(String::from_utf8_lossy(&tail[..end]).into_owned())
    }

    fn va_to_rva(&self, va: u64) -> Option<u32> {
        va.checked_sub(self.image_base).and_then(|v| u32::try_from(v).ok())
    }

    fn thunk_size(&self) -> u32 {
        if self.pe32plus {
            8
        } else {
            4
        }
    }

    fn read_thunk(&self, rva: u32) -> Option<u64> {
        if self.pe32plus {
            self.u64_rva(rva)
        } else {
            self.u32_rva(rva).map(u64::from)
        }
    }

    fn collect_thunks(&self, dll: &str, int_rva: u32, iat_rva: u32, delay: bool, out: &mut Vec<Import>) {
        let tsz = self.thunk_size();
        let ord_flag: u64 = if self.pe32plus { 1 << 63 } else { 1 << 31 };
        for i in 0..MAX_THUNKS_PER_DLL as u32 {
            let off = i * tsz;
            let Some(t) = self.read_thunk(int_rva.wrapping_add(off)) else { break };
            if t == 0 {
                break;
            }
            let name = if t & ord_flag != 0 {
                ImportName::Ordinal((t & 0xffff) as u16)
            } else {
                let nrva = (t & 0x7fff_ffff) as u32;
                let hint = self.u16_rva(nrva).unwrap_or(0);
                match self.cstr(nrva.wrapping_add(2)) {
                    Some(name) => ImportName::Name { name, hint },
                    None => break,
                }
            };
            out.push(Import { dll: dll.to_string(), name, delay, iat_rva: iat_rva.wrapping_add(off) });
        }
    }

    /// Regular and delay-load imports, in table order.
    pub fn imports(&self) -> Vec<Import> {
        let mut out = Vec::new();
        let dir = self.dirs[DIR_IMPORT];
        if dir.rva != 0 {
            for i in 0..MAX_IMPORT_DLLS as u32 {
                let d = dir.rva.wrapping_add(i * 20);
                let (Some(oft), Some(name), Some(ft)) = (self.u32_rva(d), self.u32_rva(d + 12), self.u32_rva(d + 16)) else {
                    break;
                };
                if oft == 0 && name == 0 && ft == 0 {
                    break;
                }
                let Some(dll) = self.cstr(name) else { break };
                let int_rva = if oft != 0 { oft } else { ft };
                self.collect_thunks(&dll, int_rva, ft, false, &mut out);
            }
        }
        let dd = self.dirs[DIR_DELAY_IMPORT];
        if dd.rva != 0 {
            for i in 0..MAX_IMPORT_DLLS as u32 {
                let d = dd.rva.wrapping_add(i * 32);
                let (Some(attrs), Some(name), Some(iat), Some(int)) =
                    (self.u32_rva(d), self.u32_rva(d + 4), self.u32_rva(d + 12), self.u32_rva(d + 16))
                else {
                    break;
                };
                if attrs == 0 && name == 0 && iat == 0 && int == 0 {
                    break;
                }
                let conv = |v: u32| -> Option<u32> {
                    if attrs & 1 != 0 {
                        Some(v)
                    } else {
                        self.va_to_rva(v as u64)
                    }
                };
                let (Some(name), Some(iat), Some(int)) = (conv(name), conv(iat), conv(int)) else { break };
                let Some(dll) = self.cstr(name) else { break };
                self.collect_thunks(&dll, int, iat, true, &mut out);
            }
        }
        out
    }

    pub fn exports(&self) -> Vec<Export> {
        let mut out = Vec::new();
        let dir = self.dirs[DIR_EXPORT];
        if dir.rva == 0 || dir.size < 40 {
            return out;
        }
        let rd = |o: u32| self.u32_rva(dir.rva.wrapping_add(o));
        let (Some(base), Some(nfuncs), Some(nnames), Some(funcs), Some(names), Some(ords)) =
            (rd(16), rd(20), rd(24), rd(28), rd(32), rd(36))
        else {
            return out;
        };
        let nfuncs = (nfuncs as usize).min(MAX_EXPORTS);
        let nnames = (nnames as usize).min(MAX_EXPORTS);
        let mut name_of: Vec<Option<String>> = vec![None; nfuncs];
        for i in 0..nnames as u32 {
            let (Some(nrva), Some(idx)) = (self.u32_rva(names.wrapping_add(i * 4)), self.u16_rva(ords.wrapping_add(i * 2)))
            else {
                break;
            };
            if let (Some(slot), Some(n)) = (name_of.get_mut(idx as usize), self.cstr(nrva)) {
                *slot = Some(n);
            }
        }
        let dir_end = dir.rva.saturating_add(dir.size);
        for (i, name) in name_of.into_iter().enumerate() {
            let Some(rva) = self.u32_rva(funcs.wrapping_add(i as u32 * 4)) else { break };
            if rva == 0 {
                continue;
            }
            let forwarder = if rva >= dir.rva && rva < dir_end { self.cstr(rva) } else { None };
            out.push(Export { name, ordinal: base.wrapping_add(i as u32), rva, forwarder });
        }
        out
    }

    pub fn tls(&self) -> Option<TlsDir> {
        let dir = self.dirs[DIR_TLS];
        if dir.rva == 0 {
            return None;
        }
        let r = dir.rva;
        if self.pe32plus {
            Some(TlsDir {
                start_va: self.u64_rva(r)?,
                end_va: self.u64_rva(r + 8)?,
                index_va: self.u64_rva(r + 16)?,
                callbacks_va: self.u64_rva(r + 24)?,
                zero_fill: self.u32_rva(r + 32)?,
                chars: self.u32_rva(r + 36)?,
            })
        } else {
            Some(TlsDir {
                start_va: self.u32_rva(r)? as u64,
                end_va: self.u32_rva(r + 4)? as u64,
                index_va: self.u32_rva(r + 8)? as u64,
                callbacks_va: self.u32_rva(r + 12)? as u64,
                zero_fill: self.u32_rva(r + 16)?,
                chars: self.u32_rva(r + 20)?,
            })
        }
    }

    /// Calls `f(rva, type)` for every base relocation entry (absolute/padding entries skipped).
    pub fn for_each_reloc(&self, mut f: impl FnMut(u32, u8)) {
        let dir = self.dirs[DIR_BASERELOC];
        if dir.rva == 0 {
            return;
        }
        let mut off = 0u32;
        while off.saturating_add(8) <= dir.size {
            let (Some(page), Some(bsize)) = (self.u32_rva(dir.rva + off), self.u32_rva(dir.rva + off + 4)) else { break };
            if bsize < 8 || off.saturating_add(bsize) > dir.size {
                break;
            }
            let n = (bsize - 8) / 2;
            for i in 0..n {
                let Some(e) = self.u16_rva(dir.rva + off + 8 + i * 2) else { break };
                let ty = (e >> 12) as u8;
                if ty != REL_ABSOLUTE {
                    f(page.wrapping_add((e & 0xfff) as u32), ty);
                }
            }
            off += bsize;
        }
    }

    pub fn section_for_rva(&self, rva: u32) -> Option<&Section> {
        self.sections.iter().find(|s| rva >= s.vaddr && rva - s.vaddr < s.extent())
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builds a tiny but valid PE32+ image in memory for unit tests.

    pub fn put16(v: &mut [u8], off: usize, x: u16) {
        v[off..off + 2].copy_from_slice(&x.to_le_bytes());
    }
    pub fn put32(v: &mut [u8], off: usize, x: u32) {
        v[off..off + 4].copy_from_slice(&x.to_le_bytes());
    }
    pub fn put64(v: &mut [u8], off: usize, x: u64) {
        v[off..off + 8].copy_from_slice(&x.to_le_bytes());
    }

    /// One `.text` section at RVA 0x1000, one `.idata` section at RVA 0x2000 importing
    /// `kernel32.dll!ExitProcess` and `msvcrt.dll!puts`.
    pub fn sample_pe() -> Vec<u8> {
        let mut v = vec![0u8; 0x600];
        v[0..2].copy_from_slice(b"MZ");
        put32(&mut v, 0x3c, 0x80);
        v[0x80..0x84].copy_from_slice(b"PE\0\0");
        let c = 0x84;
        put16(&mut v, c, 0x8664);
        put16(&mut v, c + 2, 2);
        put16(&mut v, c + 16, 0xf0);
        put16(&mut v, c + 18, 0x0022);
        let o = c + 20;
        put16(&mut v, o, 0x20b);
        put32(&mut v, o + 16, 0x1000);
        put64(&mut v, o + 24, 0x1_4000_0000);
        put32(&mut v, o + 32, 0x1000);
        put32(&mut v, o + 36, 0x200);
        put32(&mut v, o + 56, 0x3000);
        put32(&mut v, o + 60, 0x200);
        put16(&mut v, o + 68, 3);
        put32(&mut v, o + 108, 16);
        // import directory: RVA 0x2000, size 0x28
        put32(&mut v, o + 112 + 8, 0x2000);
        put32(&mut v, o + 112 + 12, 0x28);
        let s = o + 0xf0;
        v[s..s + 5].copy_from_slice(b".text");
        put32(&mut v, s + 8, 0x10);
        put32(&mut v, s + 12, 0x1000);
        put32(&mut v, s + 16, 0x200);
        put32(&mut v, s + 20, 0x200);
        put32(&mut v, s + 36, 0x6000_0020);
        let s2 = s + 40;
        v[s2..s2 + 6].copy_from_slice(b".idata");
        put32(&mut v, s2 + 8, 0x200);
        put32(&mut v, s2 + 12, 0x2000);
        put32(&mut v, s2 + 16, 0x200);
        put32(&mut v, s2 + 20, 0x400);
        put32(&mut v, s2 + 36, 0xc000_0040);
        // .idata raw at 0x400
        let b = 0x400;
        // descriptors at +0 and +20, terminator at +40
        put32(&mut v, b, 0x2100); // OFT kernel32
        put32(&mut v, b + 12, 0x2180); // name
        put32(&mut v, b + 16, 0x2100); // FT
        put32(&mut v, b + 20, 0x2120); // OFT msvcrt
        put32(&mut v, b + 20 + 12, 0x2190);
        put32(&mut v, b + 20 + 16, 0x2120);
        // thunks
        put64(&mut v, b + 0x100, 0x21a0);
        put64(&mut v, b + 0x120, 0x21c0);
        v[b + 0x180..b + 0x180 + 13].copy_from_slice(b"kernel32.dll\0");
        v[b + 0x190..b + 0x190 + 11].copy_from_slice(b"msvcrt.dll\0");
        put16(&mut v, b + 0x1a0, 7);
        v[b + 0x1a2..b + 0x1a2 + 12].copy_from_slice(b"ExitProcess\0");
        put16(&mut v, b + 0x1c0, 9);
        v[b + 0x1c2..b + 0x1c2 + 5].copy_from_slice(b"puts\0");
        v
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn parses_headers_and_imports() {
        let buf = sample_pe();
        let pe = Pe::parse(&buf, Layout::File).unwrap();
        assert_eq!(pe.machine, MACHINE_AMD64);
        assert!(pe.pe32plus);
        assert_eq!(pe.image_base, 0x1_4000_0000);
        assert_eq!(pe.sections.len(), 2);
        assert_eq!(pe.sections[0].name, ".text");
        let imps = pe.imports();
        assert_eq!(imps.len(), 2);
        assert_eq!(imps[0].display(), "kernel32.dll!ExitProcess");
        assert_eq!(imps[1].display(), "msvcrt.dll!puts");
        assert_eq!(imps[0].iat_rva, 0x2100);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Pe::parse(b"not a pe file at all, just some text.....................", Layout::File).is_err());
        assert!(Pe::parse(&[], Layout::File).is_err());
        let mut buf = sample_pe();
        buf.truncate(0x100);
        assert!(Pe::parse(&buf, Layout::File).is_err());
    }

    #[test]
    fn truncated_import_table_does_not_panic() {
        let mut buf = sample_pe();
        buf.truncate(0x420);
        if let Ok(pe) = Pe::parse(&buf, Layout::File) {
            let _ = pe.imports();
        }
    }

    #[test]
    fn random_bytes_never_panic() {
        let base = sample_pe();
        let mut seed = 0x1234_5678u32;
        for _ in 0..2000 {
            let mut buf = base.clone();
            for _ in 0..8 {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let i = (seed >> 8) as usize % buf.len();
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                buf[i] = (seed >> 16) as u8;
            }
            if let Ok(pe) = Pe::parse(&buf, Layout::File) {
                let _ = pe.imports();
                let _ = pe.exports();
                let _ = pe.tls();
                pe.for_each_reloc(|_, _| {});
            }
        }
    }
}
