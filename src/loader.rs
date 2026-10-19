//! PE image loader: maps images, applies relocations, binds imports to native implementations,
//! sets up static TLS and starts the program.
//!
//! The same binder serves two entry paths: [`load_main`] maps a `.exe` itself, while
//! [`attach_mapped`] adopts an image the Linux kernel already mapped (executables produced by
//! `nle relink`).

use crate::pe::{self, Import, ImportName, Layout, Pe, PeError};
use crate::registry;
use crate::runtime::{self, paths, teb, trap};
use crate::win::*;
use crate::{trace_bind, trace_load};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

const PAGE: usize = 4096;

#[derive(Debug)]
pub enum LoadError {
    Io(String),
    Pe(PeError),
    Unsupported(String),
    AddressSpace(String),
    Unresolved(Vec<String>),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(m) => write!(f, "{m}"),
            LoadError::Pe(e) => write!(f, "{e}"),
            LoadError::Unsupported(m) => write!(f, "unsupported: {m}"),
            LoadError::AddressSpace(m) => write!(f, "cannot map image: {m}"),
            LoadError::Unresolved(v) => write!(f, "{} unimplemented imports (strict mode): {}", v.len(), v.join(", ")),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<PeError> for LoadError {
    fn from(e: PeError) -> Self {
        LoadError::Pe(e)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SectionMap {
    pub start: usize,
    pub len: usize,
    pub prot: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct TlsInfo {
    pub index: usize,
    pub template: usize,
    pub raw_len: usize,
    pub zero_fill: usize,
    pub callbacks: usize,
}

enum Target {
    Addr(usize),
    Forward(String),
}

struct ExportMap {
    by_name: HashMap<String, Target>,
    by_ordinal: HashMap<u32, Target>,
}

pub struct Module {
    pub name: String,
    pub path: PathBuf,
    pub base: usize,
    pub size: usize,
    pub preferred_base: u64,
    pub entry: usize,
    pub is_dll: bool,
    pub is_main: bool,
    pub tls: Option<TlsInfo>,
    /// Absolute address and byte size of the `.pdata` unwind table.
    pub pdata: (usize, usize),
    pub sections: Vec<SectionMap>,
    pub no_thread_calls: AtomicBool,
    exports: OnceLock<ExportMap>,
}

impl Module {
    fn export_map(&self) -> &ExportMap {
        self.exports.get_or_init(|| {
            let mut by_name = HashMap::new();
            let mut by_ordinal = HashMap::new();
            let img = unsafe { std::slice::from_raw_parts(self.base as *const u8, self.size) };
            if let Ok(pe) = Pe::parse(img, Layout::Image) {
                for e in pe.exports() {
                    let target = match &e.forwarder {
                        Some(f) => Target::Forward(f.clone()),
                        None => Target::Addr(self.base + e.rva as usize),
                    };
                    let t2 = match &target {
                        Target::Addr(a) => Target::Addr(*a),
                        Target::Forward(s) => Target::Forward(s.clone()),
                    };
                    by_ordinal.insert(e.ordinal, t2);
                    if let Some(n) = e.name {
                        by_name.insert(n, target);
                    }
                }
            }
            ExportMap { by_name, by_ordinal }
        })
    }

    fn target_addr(t: &Target) -> Option<usize> {
        match t {
            Target::Addr(a) => Some(*a),
            Target::Forward(f) => resolve_forwarder(f),
        }
    }

    pub fn export_by_name(&self, name: &str) -> Option<usize> {
        self.export_map().by_name.get(name).and_then(Self::target_addr)
    }

    pub fn export_by_ordinal(&self, ord: u32) -> Option<usize> {
        self.export_map().by_ordinal.get(&ord).and_then(Self::target_addr)
    }

    pub fn win_path(&self) -> String {
        paths::host_to_win(&self.path)
    }

    pub fn contains(&self, addr: usize) -> bool {
        addr >= self.base && addr < self.base + self.size
    }
}

static MODULES: RwLock<Vec<Arc<Module>>> = RwLock::new(Vec::new());
static NEXT_TLS_INDEX: AtomicUsize = AtomicUsize::new(0);
static FAKES: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());

pub fn all_modules() -> Vec<Arc<Module>> {
    MODULES.read().unwrap().clone()
}

pub fn main_module() -> Option<Arc<Module>> {
    MODULES.read().unwrap().iter().find(|m| m.is_main).cloned()
}

pub fn module_by_base(base: usize) -> Option<Arc<Module>> {
    MODULES.read().unwrap().iter().find(|m| m.base == base).cloned()
}

pub fn module_from_addr(addr: usize) -> Option<Arc<Module>> {
    MODULES.read().unwrap().iter().find(|m| m.contains(addr)).cloned()
}

fn module_by_path(p: &Path) -> Option<Arc<Module>> {
    MODULES.read().unwrap().iter().find(|m| m.path == p).cloned()
}

fn norm_dll_name(name: &str) -> String {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name).to_ascii_lowercase();
    if base.contains('.') {
        base
    } else {
        format!("{base}.dll")
    }
}

pub fn module_by_name(name: &str) -> Option<Arc<Module>> {
    let n = norm_dll_name(name);
    MODULES.read().unwrap().iter().find(|m| m.name.to_ascii_lowercase() == n).cloned()
}

// ---- fake system modules -----------------------------------------------------------------------

/// A handle for a Windows system DLL that we implement natively. It points at a page with a
/// minimal PE header so code that sniffs the header does not crash; exports resolve through the registry.
pub fn fake_module(dll: &str) -> Option<usize> {
    let n = norm_dll_name(dll);
    registry::family_of(&n)?;
    let mut fakes = FAKES.lock().unwrap();
    if let Some((_, a)) = fakes.iter().find(|(d, _)| *d == n) {
        return Some(*a);
    }
    unsafe {
        let p = libc::mmap(std::ptr::null_mut(), PAGE, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANONYMOUS, -1, 0) as *mut u8;
        if p as isize == -1 {
            return None;
        }
        *p = b'M';
        *p.add(1) = b'Z';
        (p.add(0x3c) as *mut u32).write(0x80);
        std::ptr::copy_nonoverlapping(b"PE\0\0".as_ptr(), p.add(0x80), 4);
        (p.add(0x84) as *mut u16).write(0x8664);
        (p.add(0x84 + 16) as *mut u16).write(0xf0);
        (p.add(0x84 + 18) as *mut u16).write(0x2022);
        (p.add(0x98) as *mut u16).write(0x20b);
        libc::mprotect(p as *mut libc::c_void, PAGE, libc::PROT_READ);
        fakes.push((n, p as usize));
        Some(p as usize)
    }
}

pub fn fake_module_name(h: usize) -> Option<String> {
    FAKES.lock().unwrap().iter().find(|(_, a)| *a == h).map(|(d, _)| d.clone())
}

pub fn get_module_handle(name: Option<&str>) -> Option<usize> {
    match name {
        None => main_module().map(|m| m.base),
        Some(n) => module_by_name(n).map(|m| m.base).or_else(|| fake_module(n)),
    }
}

// ---- address-space handling ---------------------------------------------------------------------
fn round_up(v: usize, to: usize) -> usize {
    (v + to - 1) & !(to - 1)
}

fn prot_of(chars: u32) -> i32 {
    let mut p = 0;
    if chars & pe::SCN_MEM_WRITE != 0 {
        p |= libc::PROT_READ | libc::PROT_WRITE;
    }
    if chars & (pe::SCN_MEM_EXECUTE | pe::SCN_CNT_CODE) != 0 {
        p |= libc::PROT_READ | libc::PROT_EXEC;
    }
    if p == 0 {
        p = libc::PROT_READ;
    }
    p
}

unsafe fn reserve(pe: &Pe, size: usize) -> Result<(usize, i64), LoadError> {
    let preferred = pe.image_base as usize;
    let flags = libc::MAP_PRIVATE | libc::MAP_ANONYMOUS;
    let p = libc::mmap(preferred as *mut libc::c_void, size, libc::PROT_NONE, flags | libc::MAP_FIXED_NOREPLACE, -1, 0);
    if p != libc::MAP_FAILED && p as usize == preferred {
        return Ok((preferred, 0));
    }
    if p != libc::MAP_FAILED {
        libc::munmap(p, size);
    }
    if !pe.has_relocs() {
        return Err(LoadError::AddressSpace(format!(
            "preferred base 0x{preferred:x} is taken and the image has no relocation table"
        )));
    }
    let p = libc::mmap(std::ptr::null_mut(), size, libc::PROT_NONE, flags, -1, 0);
    if p == libc::MAP_FAILED {
        return Err(LoadError::AddressSpace(format!("mmap of {size} bytes failed")));
    }
    Ok((p as usize, p as usize as i64 - preferred as i64))
}

unsafe fn copy_image(pe: &Pe, base: usize, bytes: &[u8], size: usize) -> Result<(), LoadError> {
    let hdr = (pe.size_of_headers as usize).min(bytes.len()).min(size);
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), base as *mut u8, hdr);
    for s in &pe.sections {
        let extent = s.extent() as usize;
        if s.vaddr as usize + extent > size {
            return Err(LoadError::Pe(PeError::Truncated("section extends past SizeOfImage")));
        }
        let n = (s.raw_size as usize).min(extent);
        if n == 0 {
            continue;
        }
        let off = s.raw_off as usize;
        let src = bytes.get(off..off + n).ok_or(LoadError::Pe(PeError::Truncated("section raw data")))?;
        std::ptr::copy_nonoverlapping(src.as_ptr(), (base + s.vaddr as usize) as *mut u8, n);
    }
    Ok(())
}

unsafe fn apply_relocs(pe: &Pe, base: usize, size: usize, delta: i64) {
    pe.for_each_reloc(|rva, ty| {
        let rva = rva as usize;
        match ty {
            pe::REL_DIR64 if rva + 8 <= size => {
                let p = (base + rva) as *mut u64;
                p.write_unaligned(p.read_unaligned().wrapping_add(delta as u64));
            }
            pe::REL_HIGHLOW if rva + 4 <= size => {
                let p = (base + rva) as *mut u32;
                p.write_unaligned(p.read_unaligned().wrapping_add(delta as u32));
            }
            pe::REL_HIGH if rva + 2 <= size => {
                let p = (base + rva) as *mut u16;
                p.write_unaligned(p.read_unaligned().wrapping_add((delta >> 16) as u16));
            }
            pe::REL_LOW if rva + 2 <= size => {
                let p = (base + rva) as *mut u16;
                p.write_unaligned(p.read_unaligned().wrapping_add(delta as u16));
            }
            _ => {}
        }
    });
}

fn section_maps(pe: &Pe, base: usize) -> Vec<SectionMap> {
    pe.sections
        .iter()
        .map(|s| SectionMap { start: base + s.vaddr as usize, len: round_up(s.extent() as usize, PAGE), prot: prot_of(s.chars) })
        .collect()
}

unsafe fn apply_protections(pe: &Pe, base: usize, sections: &[SectionMap]) {
    let hdr = round_up(pe.size_of_headers as usize, PAGE);
    libc::mprotect(base as *mut libc::c_void, hdr.min(pe.size_of_image as usize), libc::PROT_READ);
    for s in sections {
        if s.len > 0 {
            libc::mprotect(s.start as *mut libc::c_void, s.len, s.prot);
        }
    }
}

/// Makes non-executable sections writable so the IAT can be patched.
unsafe fn unprotect_for_binding(sections: &[SectionMap]) {
    for s in sections {
        if s.len > 0 && s.prot & libc::PROT_EXEC == 0 {
            libc::mprotect(s.start as *mut libc::c_void, s.len, libc::PROT_READ | libc::PROT_WRITE);
        }
    }
}

// ---- import binding ---------------------------------------------------------------------------------

fn resolve_forwarder(f: &str) -> Option<usize> {
    let (dll, func) = f.rsplit_once('.')?;
    let dll_name = format!("{dll}.dll");
    if registry::is_system_dll(&dll_name) {
        return match func.strip_prefix('#') {
            Some(_) => None,
            None => registry::resolve(&dll_name, func),
        };
    }
    let m = module_by_name(&dll_name)?;
    match func.strip_prefix('#') {
        Some(o) => m.export_by_ordinal(o.parse().ok()?),
        None => m.export_by_name(func),
    }
}

/// Where the loader looks for DLLs that ship with the application.
pub fn find_dll(name: &str) -> Option<PathBuf> {
    if name.contains(['\\', '/']) {
        let p = paths::resolve(name);
        return p.is_file().then_some(p);
    }
    let file = if name.contains('.') { name.to_string() } else { format!("{name}.dll") };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = runtime::cfg().exe_host.parent() {
        dirs.push(d.to_path_buf());
    }
    if let Ok(c) = std::env::current_dir() {
        dirs.push(c);
    }
    dirs.push(paths::prefix().join("drive_c/windows/system32"));
    dirs.iter().find_map(|d| crate::analyze::find_ci(d, &file).filter(|p| p.is_file()))
}

struct LoadCtx {
    /// Modules in dependency order, ready for their initializers.
    init: Vec<Arc<Module>>,
    unresolved: Vec<String>,
}

fn resolve_import(imp: &Import, deps: &mut HashMap<String, Option<Arc<Module>>>, ctx: &mut LoadCtx) -> Option<usize> {
    if registry::is_system_dll(&imp.dll) {
        return match &imp.name {
            ImportName::Name { name, .. } => registry::resolve(&imp.dll, name),
            ImportName::Ordinal(_) => None,
        };
    }
    let key = imp.dll.to_ascii_lowercase();
    let dep = deps
        .entry(key)
        .or_insert_with(|| match find_dll(&imp.dll) {
            Some(p) => match load_image_file(&p, false, ctx) {
                Ok(m) => Some(m),
                Err(e) => {
                    trace_load!("dependency {} failed to load: {e}", imp.dll);
                    None
                }
            },
            None => {
                trace_load!("dependency {} not found", imp.dll);
                None
            }
        })
        .clone()?;
    match &imp.name {
        ImportName::Name { name, .. } => dep.export_by_name(name),
        ImportName::Ordinal(o) => dep.export_by_ordinal(*o as u32),
    }
}

unsafe fn bind_imports(pe: &Pe, module: &Module, ctx: &mut LoadCtx) {
    let mut deps: HashMap<String, Option<Arc<Module>>> = HashMap::new();
    let mut pending: Vec<(usize, String)> = Vec::new();
    for imp in pe.imports() {
        let slot_off = imp.iat_rva as usize;
        if slot_off + 8 > module.size {
            continue;
        }
        let slot = (module.base + slot_off) as *mut u64;
        match resolve_import(&imp, &mut deps, ctx) {
            Some(addr) => {
                trace_bind!("{} -> 0x{:x}", imp.display(), addr);
                slot.write_unaligned(addr as u64);
            }
            None if imp.delay => trace_bind!("{} left to the delay-load helper", imp.display()),
            None => {
                trace_bind!("{} UNRESOLVED", imp.display());
                pending.push((slot as usize, imp.display()));
            }
        }
    }
    if !pending.is_empty() {
        let labels: Vec<String> = pending.iter().map(|p| p.1.clone()).collect();
        ctx.unresolved.extend(labels.iter().cloned());
        for ((slot, _), stub) in pending.iter().zip(trap::make_stubs(&labels)) {
            (*slot as *mut u64).write_unaligned(stub as u64);
        }
    }
}

// ---- loading -----------------------------------------------------------------------------------------

fn tls_info(pe: &Pe, delta: i64) -> Option<TlsInfo> {
    let t = pe.tls()?;
    if t.end_va < t.start_va || t.index_va == 0 {
        return None;
    }
    let index = NEXT_TLS_INDEX.fetch_add(1, Ordering::SeqCst);
    if index >= teb::TLS_ARRAY_LEN {
        return None;
    }
    let adj = |v: u64| (v as i64).wrapping_add(delta) as usize;
    let info = TlsInfo {
        index,
        template: adj(t.start_va),
        raw_len: (t.end_va - t.start_va) as usize,
        zero_fill: t.zero_fill as usize,
        callbacks: if t.callbacks_va != 0 { adj(t.callbacks_va) } else { 0 },
    };
    unsafe { (adj(t.index_va) as *mut u32).write_unaligned(index as u32) };
    Some(info)
}

fn validate(pe: &Pe) -> Result<(), LoadError> {
    if pe.machine != pe::MACHINE_AMD64 || !pe.pe32plus {
        return Err(LoadError::Unsupported(format!("{} executables cannot run in a native x86-64 process", pe.arch_name())));
    }
    if pe.is_clr() {
        return Err(LoadError::Unsupported(".NET managed executables need a CLR (Mono or dotnet)".into()));
    }
    if pe.section_align < PAGE as u32 || !pe.section_align.is_power_of_two() {
        return Err(LoadError::Unsupported("section alignment below the page size".into()));
    }
    if pe.size_of_image == 0 || pe.size_of_image > 0x4000_0000 {
        return Err(LoadError::Pe(PeError::Truncated("implausible SizeOfImage")));
    }
    Ok(())
}

fn build_module(pe: &Pe, path: &Path, base: usize, size: usize, delta: i64, is_main: bool, tls: Option<TlsInfo>) -> Module {
    let pdata = {
        let d = pe.dirs[pe::DIR_EXCEPTION];
        if d.rva != 0 && d.size != 0 {
            (base + d.rva as usize, d.size as usize)
        } else {
            (0, 0)
        }
    };
    Module {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: path.to_path_buf(),
        base,
        size,
        preferred_base: (base as i64 - delta) as u64,
        entry: if pe.entry_rva != 0 { base + pe.entry_rva as usize } else { 0 },
        is_dll: pe.is_dll(),
        is_main,
        tls,
        pdata,
        sections: section_maps(pe, base),
        no_thread_calls: AtomicBool::new(false),
        exports: OnceLock::new(),
    }
}

fn load_image_file(path: &Path, is_main: bool, ctx: &mut LoadCtx) -> Result<Arc<Module>, LoadError> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(m) = module_by_path(&canon) {
        return Ok(m);
    }
    let bytes = std::fs::read(&canon).map_err(|e| LoadError::Io(format!("{}: {e}", canon.display())))?;
    let pe = Pe::parse(&bytes, Layout::File)?;
    validate(&pe)?;
    let size = round_up(pe.size_of_image as usize, PAGE);
    unsafe {
        let (base, delta) = reserve(&pe, size)?;
        trace_load!("{} mapped at 0x{:x} (preferred 0x{:x})", canon.display(), base, pe.image_base);
        libc::mprotect(base as *mut libc::c_void, size, libc::PROT_READ | libc::PROT_WRITE);
        copy_image(&pe, base, &bytes, size)?;
        if delta != 0 {
            apply_relocs(&pe, base, size, delta);
        }
        let tls = tls_info(&pe, delta);
        let module = Arc::new(build_module(&pe, &canon, base, size, delta, is_main, tls));
        MODULES.write().unwrap().push(module.clone());
        bind_imports(&pe, &module, ctx);
        apply_protections(&pe, base, &module.sections);
        ctx.init.push(module.clone());
        Ok(module)
    }
}

/// Maps the main executable and every application DLL it needs.
pub fn load_main(path: &Path) -> Result<(Arc<Module>, Vec<Arc<Module>>), LoadError> {
    let mut ctx = LoadCtx { init: Vec::new(), unresolved: Vec::new() };
    let m = load_image_file(path, true, &mut ctx)?;
    if runtime::cfg().strict && !ctx.unresolved.is_empty() {
        return Err(LoadError::Unresolved(ctx.unresolved));
    }
    maybe_start_kuser(&ctx.init);
    Ok((m, ctx.init))
}

/// Adopts an image that the kernel's ELF loader already mapped at its preferred base.
pub unsafe fn attach_mapped(base: usize, path: &Path) -> Result<(Arc<Module>, Vec<Arc<Module>>), LoadError> {
    let probe = std::slice::from_raw_parts(base as *const u8, 0x1000);
    let size = {
        let pe = Pe::parse(probe, Layout::Image)?;
        pe.size_of_image as usize
    };
    let img = std::slice::from_raw_parts(base as *const u8, size);
    let pe = Pe::parse(img, Layout::Image)?;
    validate(&pe)?;
    let sections = section_maps(&pe, base);
    unprotect_for_binding(&sections);
    let mut ctx = LoadCtx { init: Vec::new(), unresolved: Vec::new() };
    let tls = tls_info(&pe, 0);
    let module = Arc::new(build_module(&pe, path, base, round_up(size, PAGE), 0, true, tls));
    MODULES.write().unwrap().push(module.clone());
    bind_imports(&pe, &module, &mut ctx);
    apply_protections(&pe, base, &module.sections);
    ctx.init.push(module.clone());
    if runtime::cfg().strict && !ctx.unresolved.is_empty() {
        return Err(LoadError::Unresolved(ctx.unresolved));
    }
    maybe_start_kuser(&ctx.init);
    Ok((module, ctx.init))
}

/// `LoadLibrary` for a DLL shipped with the application. Runs its initializers.
pub fn load_library(path: &Path) -> Result<Arc<Module>, LoadError> {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(m) = module_by_path(&canon) {
        return Ok(m);
    }
    let mut ctx = LoadCtx { init: Vec::new(), unresolved: Vec::new() };
    let m = load_image_file(path, false, &mut ctx)?;
    maybe_start_kuser(&ctx.init);
    unsafe {
        alloc_tls_blocks(&ctx.init, teb::tls_array());
        for dep in &ctx.init {
            run_attach(dep);
        }
    }
    Ok(m)
}

pub fn free_library(m: &Arc<Module>) {
    if m.is_main {
        return;
    }
    unsafe {
        call_dll_main(m, DLL_PROCESS_DETACH);
        libc::munmap(m.base as *mut libc::c_void, m.size);
    }
    MODULES.write().unwrap().retain(|x| !Arc::ptr_eq(x, m));
}

// ---- KUSER_SHARED_DATA heuristic ----------------------------------------------------------------------
fn maybe_start_kuser(mods: &[Arc<Module>]) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.load(Ordering::SeqCst) {
        return;
    }
    let forced = std::env::var("NLE_KUSER").ok();
    let wanted = match forced.as_deref() {
        Some("0") | Some("never") => false,
        Some("1") | Some("always") => true,
        _ => mods.iter().any(|m| scan_for_kuser(m)),
    };
    if wanted && !STARTED.swap(true, Ordering::SeqCst) {
        unsafe {
            if teb::init_kuser() {
                teb::start_kuser_updater();
            }
        }
    }
}

/// Looks for an absolute reference into the 0x7ffe0000 page in executable sections.
fn scan_for_kuser(m: &Module) -> bool {
    m.sections.iter().filter(|s| s.prot & libc::PROT_EXEC != 0).any(|s| {
        let bytes = unsafe { std::slice::from_raw_parts(s.start as *const u8, s.len) };
        bytes.windows(4).any(|w| w[2] == 0xfe && w[3] == 0x7f && w[1] < 0x10)
    })
}

// ---- TLS and initializers --------------------------------------------------------------------------------

pub unsafe fn alloc_tls_blocks(mods: &[Arc<Module>], array: *mut usize) {
    for m in mods {
        if let Some(t) = &m.tls {
            let len = (t.raw_len + t.zero_fill).max(8);
            let blk = libc::calloc(1, len) as *mut u8;
            if t.raw_len > 0 {
                std::ptr::copy_nonoverlapping(t.template as *const u8, blk, t.raw_len);
            }
            *array.add(t.index) = blk as usize;
        }
    }
}

type TlsCallback = unsafe extern "win64" fn(usize, u32, usize);
type DllMainFn = unsafe extern "win64" fn(usize, u32, usize) -> i32;

unsafe fn call_tls_callbacks(m: &Module, reason: u32) {
    let Some(t) = &m.tls else { return };
    if t.callbacks == 0 {
        return;
    }
    let mut p = t.callbacks as *const usize;
    while *p != 0 {
        let cb: TlsCallback = std::mem::transmute(*p);
        cb(m.base, reason, 0);
        p = p.add(1);
    }
}

unsafe fn call_dll_main(m: &Module, reason: u32) -> bool {
    if !m.is_dll || m.entry == 0 {
        return true;
    }
    let f: DllMainFn = std::mem::transmute(m.entry);
    f(m.base, reason, 0) != 0
}

unsafe fn run_attach(m: &Module) {
    call_tls_callbacks(m, DLL_PROCESS_ATTACH);
    if !call_dll_main(m, DLL_PROCESS_ATTACH) {
        trace_load!("DllMain of {} returned FALSE", m.name);
    }
}

/// Notifies every module that a new thread started (TLS callbacks and `DllMain`).
pub unsafe fn thread_attach() {
    let mods = all_modules();
    alloc_tls_blocks(&mods, teb::tls_array());
    for m in &mods {
        call_tls_callbacks(m, DLL_THREAD_ATTACH);
        if !m.no_thread_calls.load(Ordering::Relaxed) {
            call_dll_main(m, DLL_THREAD_ATTACH);
        }
    }
}

pub unsafe fn thread_detach() {
    for m in all_modules().iter().rev() {
        if !m.no_thread_calls.load(Ordering::Relaxed) {
            call_dll_main(m, DLL_THREAD_DETACH);
        }
        call_tls_callbacks(m, DLL_THREAD_DETACH);
    }
}

pub unsafe fn exit_process(code: u32) -> ! {
    for m in all_modules().iter().rev() {
        if m.is_dll {
            call_dll_main(m, DLL_PROCESS_DETACH);
        }
    }
    libc::fflush(std::ptr::null_mut());
    libc::_exit((code & 0xff) as i32)
}

/// Sets up the process-wide state, runs initializers and calls the program's entry point.
pub unsafe fn run_main(main: Arc<Module>, init: Vec<Arc<Module>>) -> ! {
    if main.entry == 0 {
        eprintln!("nle: {} has no entry point", main.name);
        libc::_exit(1);
    }
    teb::init_peb(main.base);
    teb::setup_thread();
    alloc_tls_blocks(&init, teb::tls_array());
    for m in &init {
        run_attach(m);
    }
    let code = teb::call_guest_top(main.entry, teb::peb());
    exit_process(code)
}

// ---- GetProcAddress ------------------------------------------------------------------------------------------

pub fn get_proc_address(hmod: usize, name: &str) -> Option<usize> {
    if let Some(m) = module_by_base(hmod) {
        return m.export_by_name(name);
    }
    let dll = fake_module_name(hmod)?;
    registry::resolve(&dll, name)
}

pub fn get_proc_address_ordinal(hmod: usize, ord: u32) -> Option<usize> {
    module_by_base(hmod)?.export_by_ordinal(ord)
}
