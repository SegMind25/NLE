//! Resources of loaded modules: the resource directory tree and decoders for the resource types
//! GUI programs load (string tables, icons, bitmaps).
//!
//! The tree walker works on a byte slice of the mapped image, so it is bounds-checked and testable
//! without a real module. Menus, dialogs and accelerator tables are decoded by their own modules
//! from the bytes found here.

use super::gdi::{dib_info, dib_to_surface};
use super::surface::Surface;
use crate::loader;
use crate::pe::{Layout, Pe};

pub const RT_CURSOR: u16 = 1;
pub const RT_BITMAP: u16 = 2;
pub const RT_ICON: u16 = 3;
pub const RT_MENU: u16 = 4;
pub const RT_DIALOG: u16 = 5;
pub const RT_STRING: u16 = 6;
pub const RT_ACCELERATOR: u16 = 9;
pub const RT_GROUP_CURSOR: u16 = 12;
pub const RT_GROUP_ICON: u16 = 14;

/// A resource type or name: a number (`MAKEINTRESOURCE`) or a string.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Id(u16),
    Name(String),
}

impl Key {
    /// From the `LPCWSTR` / `LPCSTR` argument of the resource APIs: values below 64 KiB are numbers,
    /// and so are names of the form `#123`.
    pub fn from_arg(p: usize, wide: bool) -> Key {
        if p < 0x10000 {
            return Key::Id(p as u16);
        }
        let s = unsafe {
            if wide {
                crate::runtime::strings::w_ptr_to_string(p as *const u16)
            } else {
                crate::runtime::strings::a_to_string(p as *const u8)
            }
        };
        match s.strip_prefix('#').and_then(|n| n.parse::<u16>().ok()) {
            Some(n) => Key::Id(n),
            None => Key::Name(s),
        }
    }
}

/// Where a resource's bytes are, as offsets into the image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    /// Offset of the `IMAGE_RESOURCE_DATA_ENTRY` (what `FindResource` hands out as the `HRSRC`).
    pub entry: usize,
    pub rva: usize,
    pub size: usize,
}

fn u16_at(img: &[u8], off: usize) -> Option<u32> {
    let b = img.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([b[0], b[1]]) as u32)
}

fn u32_at(img: &[u8], off: usize) -> Option<u32> {
    let b = img.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// One level of the tree: `(id or name, offset field)` per entry; `Key::Id` entries sort after named ones.
fn level(img: &[u8], root: usize, dir: usize) -> Vec<(Key, u32)> {
    let mut out = Vec::new();
    let base = root + dir;
    let (Some(named), Some(ids)) = (u16_at(img, base + 12), u16_at(img, base + 14)) else { return out };
    for i in 0..(named + ids).min(4096) as usize {
        let e = base + 16 + i * 8;
        let (Some(name), Some(off)) = (u32_at(img, e), u32_at(img, e + 4)) else { break };
        let key = if name & 0x8000_0000 != 0 {
            let s = root + (name & 0x7fff_ffff) as usize;
            let n = u16_at(img, s).unwrap_or(0) as usize;
            let units: Vec<u16> = (0..n).filter_map(|k| u16_at(img, s + 2 + k * 2)).map(|u| u as u16).collect();
            Key::Name(String::from_utf16_lossy(&units))
        } else {
            Key::Id(name as u16)
        };
        out.push((key, off));
    }
    out
}

fn key_matches(a: &Key, b: &Key) -> bool {
    match (a, b) {
        (Key::Id(x), Key::Id(y)) => x == y,
        (Key::Name(x), Key::Name(y)) => x.eq_ignore_ascii_case(y),
        _ => false,
    }
}

/// Finds a resource in the resource section `root..` of `img`. `lang` 0 means "any": the neutral
/// language, then US English, then whatever comes first.
pub fn find_in(img: &[u8], root: usize, ty: &Key, name: &Key, lang: u16) -> Option<Found> {
    let sub = |off: u32| -> Option<usize> { (off & 0x8000_0000 != 0).then_some((off & 0x7fff_ffff) as usize) };
    let by_type = level(img, root, 0);
    let (_, t) = by_type.iter().find(|(k, _)| key_matches(k, ty))?;
    let by_name = level(img, root, sub(*t)?);
    let (_, n) = by_name.iter().find(|(k, _)| key_matches(k, name))?;
    let langs = level(img, root, sub(*n)?);
    let pick = if lang != 0 {
        langs.iter().find(|(k, _)| *k == Key::Id(lang))
    } else {
        None
    }
    .or_else(|| langs.iter().find(|(k, _)| *k == Key::Id(0)))
    .or_else(|| langs.iter().find(|(k, _)| *k == Key::Id(0x409)))
    .or_else(|| langs.first())?;
    if pick.1 & 0x8000_0000 != 0 {
        return None;
    }
    let entry = root + pick.1 as usize;
    Some(Found { entry, rva: u32_at(img, entry)? as usize, size: u32_at(img, entry + 4)? as usize })
}

/// Every name or id of one resource type (for `EnumResourceNames`).
pub fn names_in(img: &[u8], root: usize, ty: &Key) -> Vec<Key> {
    let by_type = level(img, root, 0);
    let Some((_, t)) = by_type.iter().find(|(k, _)| key_matches(k, ty)) else { return Vec::new() };
    if t & 0x8000_0000 == 0 {
        return Vec::new();
    }
    level(img, root, (t & 0x7fff_ffff) as usize).into_iter().map(|(k, _)| k).collect()
}

/// The mapped image of a module and where its resource section starts.
pub struct ModuleRes {
    pub base: usize,
    pub size: usize,
    root: usize,
}

impl ModuleRes {
    pub fn image(&self) -> &'static [u8] {
        unsafe { std::slice::from_raw_parts(self.base as *const u8, self.size) }
    }

    /// Offset of the resource section's root directory.
    pub fn root(&self) -> usize {
        self.root
    }
}

pub fn find_in_module(m: &ModuleRes, ty: &Key, name: &Key, lang: u16) -> Option<Found> {
    find_in(m.image(), m.root, ty, name, lang)
}

/// `hmodule` 0 is the main program; the low bits `LoadLibraryEx` may set on data-file handles are ignored.
pub fn module_res(hmodule: usize) -> Option<ModuleRes> {
    let m = if hmodule == 0 { loader::main_module()? } else { loader::module_by_base(hmodule & !3)? };
    let img = unsafe { std::slice::from_raw_parts(m.base as *const u8, m.size) };
    let pe = Pe::parse(img, Layout::Image).ok()?;
    let d = pe.dirs[2];
    if d.rva == 0 || d.size == 0 {
        return None;
    }
    Some(ModuleRes { base: m.base, size: m.size, root: d.rva as usize })
}

pub fn find(hmodule: usize, ty: &Key, name: &Key, lang: u16) -> Option<Found> {
    let m = module_res(hmodule)?;
    find_in(m.image(), m.root, ty, name, lang)
}

pub fn names(hmodule: usize, ty: &Key) -> Vec<Key> {
    module_res(hmodule).map(|m| names_in(m.image(), m.root, ty)).unwrap_or_default()
}

/// The bytes of a found resource.
pub fn bytes(hmodule: usize, f: &Found) -> Option<&'static [u8]> {
    let m = module_res(hmodule)?;
    m.image().get(f.rva..f.rva.checked_add(f.size)?)
}

/// Convenience: find and return the bytes.
pub fn load(hmodule: usize, ty: u16, name: &Key) -> Option<&'static [u8]> {
    let f = find(hmodule, &Key::Id(ty), name, 0)?;
    bytes(hmodule, &f)
}

// ---- string tables --------------------------------------------------------------------------------

/// String `id` of a string table: block `id / 16 + 1` holds 16 length-prefixed UTF-16 strings.
pub fn string_in_block(block: &[u8], id: u32) -> Option<Vec<u16>> {
    let mut off = 0usize;
    for i in 0..16 {
        let n = u16::from_le_bytes(block.get(off..off + 2)?.try_into().ok()?) as usize;
        off += 2;
        if i == id % 16 {
            let raw = block.get(off..off + n * 2)?;
            return Some(raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect());
        }
        off += n * 2;
    }
    None
}

pub fn load_string(hmodule: usize, id: u32) -> Option<Vec<u16>> {
    let block = load(hmodule, RT_STRING, &Key::Id((id / 16 + 1) as u16))?;
    string_in_block(block, id).filter(|s| !s.is_empty())
}

// ---- icons ----------------------------------------------------------------------------------------

/// An icon or cursor image: colour plus a separate alpha channel.
#[derive(Clone, Debug)]
pub struct IconImage {
    pub w: i32,
    pub h: i32,
    pub rgb: Vec<u32>,
    pub alpha: Vec<u8>,
    pub hotspot: (i32, i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IconEntry {
    pub w: i32,
    pub h: i32,
    pub bits: u16,
    /// `RT_ICON` id in an executable's group, or the data offset in an `.ico` file.
    pub id: u32,
    pub bytes: u32,
}

/// Entries of an `RT_GROUP_ICON` resource (`GRPICONDIR`).
pub fn parse_group(data: &[u8]) -> Vec<IconEntry> {
    let mut out = Vec::new();
    let count = u16_at(data, 4).unwrap_or(0) as usize;
    for i in 0..count.min(256) {
        let e = 6 + i * 14;
        let Some(b) = data.get(e..e + 14) else { break };
        out.push(IconEntry {
            w: if b[0] == 0 { 256 } else { b[0] as i32 },
            h: if b[1] == 0 { 256 } else { b[1] as i32 },
            bits: u16::from_le_bytes([b[6], b[7]]),
            bytes: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            id: u16::from_le_bytes([b[12], b[13]]) as u32,
        });
    }
    out
}

/// Entries of an `.ico` / `.cur` file; `id` is the offset of the image data.
pub fn parse_ico_file(data: &[u8]) -> Vec<IconEntry> {
    let mut out = Vec::new();
    if u16_at(data, 0) != Some(0) || !matches!(u16_at(data, 2), Some(1 | 2)) {
        return out;
    }
    let count = u16_at(data, 4).unwrap_or(0) as usize;
    for i in 0..count.min(256) {
        let e = 6 + i * 16;
        let Some(b) = data.get(e..e + 16) else { break };
        out.push(IconEntry {
            w: if b[0] == 0 { 256 } else { b[0] as i32 },
            h: if b[1] == 0 { 256 } else { b[1] as i32 },
            bits: u16::from_le_bytes([b[6], b[7]]),
            bytes: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            id: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
        });
    }
    out
}

/// The entry closest to the wanted size: an exact size first, then the smallest larger one, then the largest;
/// more colours break ties.
pub fn best_entry(entries: &[IconEntry], want: i32) -> Option<IconEntry> {
    entries.iter().copied().min_by_key(|e| {
        let size_rank = if e.w == want { 0 } else if e.w > want { 1 + (e.w - want) } else { 10_000 + (want - e.w) };
        (size_rank, -(e.bits as i32))
    })
}

/// Decodes the bytes of one icon image: a DIB whose height covers colour and mask, with the 1-bit AND mask after the colours.
/// PNG-compressed images are not decoded.
pub fn decode_icon(data: &[u8]) -> Option<IconImage> {
    if data.len() < 40 || data.starts_with(b"\x89PNG") {
        return None;
    }
    let info = unsafe { dib_info(data.as_ptr())? };
    let (w, h) = (info.width, info.height / 2);
    if w <= 0 || h <= 0 || info.header < 40 {
        return None;
    }
    let palette_len = info.colors * 4 + if info.compression == 3 { 12 } else { 0 };
    let color_off = info.header + palette_len;
    let stride = (w as usize * info.bpp as usize).div_ceil(32) * 4;
    let mask_off = color_off + stride * h as usize;
    let mask_stride = (w as usize).div_ceil(32) * 4;
    if data.len() < color_off + stride * h as usize {
        return None;
    }
    // decode the colour half with the shared DIB code: a copy of the header with the height halved
    let mut header = data[..info.header + palette_len].to_vec();
    header[8..12].copy_from_slice(&(h * if info.bottom_up { 1 } else { -1 }).to_le_bytes());
    let surf: Surface = unsafe { dib_to_surface(header.as_ptr(), data.as_ptr().add(color_off))? };
    let mut rgb = Vec::with_capacity((w * h) as usize);
    let mut alpha = Vec::with_capacity((w * h) as usize);
    let has_alpha = info.bpp == 32 && (0..h).any(|y| (0..w).any(|x| data[color_off + (if info.bottom_up { h - 1 - y } else { y }) as usize * stride + x as usize * 4 + 3] != 0));
    for y in 0..h {
        let src_y = if info.bottom_up { h - 1 - y } else { y };
        for x in 0..w {
            rgb.push(surf.get(x, y));
            let a = if has_alpha {
                data[color_off + src_y as usize * stride + x as usize * 4 + 3]
            } else {
                let byte = data.get(mask_off + src_y as usize * mask_stride + x as usize / 8).copied().unwrap_or(0);
                if byte >> (7 - x % 8) & 1 == 1 { 0 } else { 255 }
            };
            alpha.push(a);
        }
    }
    Some(IconImage { w, h, rgb, alpha, hotspot: (0, 0) })
}

/// `RT_GROUP_ICON` named `name` in `hmodule`, resolved to the image nearest `want` pixels.
pub fn load_icon_image(hmodule: usize, name: &Key, want: i32) -> Option<IconImage> {
    let group = load(hmodule, RT_GROUP_ICON, name)?;
    let e = best_entry(&parse_group(group), want)?;
    decode_icon(load(hmodule, RT_ICON, &Key::Id(e.id as u16))?)
}

/// An `.ico` file's image nearest `want` pixels.
pub fn icon_from_file(data: &[u8], want: i32) -> Option<IconImage> {
    let e = best_entry(&parse_ico_file(data), want)?;
    decode_icon(data.get(e.id as usize..(e.id as usize).checked_add(e.bytes as usize)?)?)
}

// ---- bitmaps --------------------------------------------------------------------------------------

/// Byte offset of the pixels in a packed DIB (header, optional bit masks, palette).
pub fn dib_pixels_offset(dib: &[u8]) -> Option<usize> {
    if dib.len() < 12 {
        return None;
    }
    let info = unsafe { dib_info(dib.as_ptr())? };
    let entry = if info.header == 12 { 3 } else { 4 };
    let masks = if info.compression == 3 && info.header == 40 { 12 } else { 0 };
    Some(info.header + masks + info.colors * entry)
}

/// A packed DIB (what `RT_BITMAP` holds, or a `.bmp` file after its 14-byte header) as a surface.
pub fn bitmap_from_dib(dib: &[u8]) -> Option<Surface> {
    let off = dib_pixels_offset(dib)?;
    let info = unsafe { dib_info(dib.as_ptr())? };
    let stride = (info.width.max(0) as usize * info.bpp as usize).div_ceil(32) * 4;
    if dib.len() < off + stride * info.height.max(0) as usize {
        return None;
    }
    unsafe { dib_to_surface(dib.as_ptr(), dib.as_ptr().add(off)) }
}

/// A `.bmp` file's contents as a surface.
pub fn bitmap_from_file(data: &[u8]) -> Option<Surface> {
    if data.len() < 14 || &data[0..2] != b"BM" {
        return None;
    }
    bitmap_from_dib(&data[14..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn le16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn le32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_le_bytes());
    }

    /// A resource section with one STRING block (id 7: strings 96..111) and one named DIALOG, in 0-based offsets.
    fn sample_tree() -> Vec<u8> {
        let mut v = Vec::new();
        // root: one id entry (type 6) and one named entry ("DIALOG")
        let dir = |v: &mut Vec<u8>, named: u16, ids: u16| {
            le32(v, 0);
            le32(v, 0);
            le16(v, 0);
            le16(v, 0);
            le16(v, named);
            le16(v, ids);
        };
        dir(&mut v, 1, 1); // 0..16
        // entries are patched below; named first
        let entries_at = v.len();
        v.extend_from_slice(&[0; 16]);
        // name string at 32: "DIALOG"
        let name_off = v.len();
        le16(&mut v, 6);
        for c in "DIALOG".encode_utf16() {
            le16(&mut v, c);
        }
        // type dirs
        let t_string = v.len();
        dir(&mut v, 0, 1);
        le32(&mut v, 7);
        le32(&mut v, 0); // patched below
        let l_string = v.len();
        dir(&mut v, 0, 1);
        le32(&mut v, 0x409);
        le32(&mut v, 0); // patched below
        let d_string = v.len();
        le32(&mut v, 0x1000); // rva
        le32(&mut v, 8); // size
        le32(&mut v, 0);
        le32(&mut v, 0);
        let t_dialog = v.len();
        dir(&mut v, 0, 1);
        le32(&mut v, 1);
        le32(&mut v, 0);
        let l_dialog = v.len();
        dir(&mut v, 0, 1);
        le32(&mut v, 0);
        le32(&mut v, 0);
        le32(&mut v, 0x2000);
        le32(&mut v, 4);
        le32(&mut v, 0);
        le32(&mut v, 0);
        // fix up the root entries and the sub-directory pointers written as "next offset"
        let patch = |v: &mut Vec<u8>, at: usize, x: u32| v[at..at + 4].copy_from_slice(&x.to_le_bytes());
        patch(&mut v, entries_at, 0x8000_0000 | name_off as u32);
        patch(&mut v, entries_at + 4, 0x8000_0000 | t_dialog as u32);
        patch(&mut v, entries_at + 8, 6);
        patch(&mut v, entries_at + 12, 0x8000_0000 | t_string as u32);
        patch(&mut v, t_string + 16 + 4, 0x8000_0000 | l_string as u32);
        patch(&mut v, l_string + 16 + 4, d_string as u32);
        patch(&mut v, t_dialog + 16 + 4, 0x8000_0000 | l_dialog as u32);
        patch(&mut v, l_dialog + 16 + 4, (l_dialog + 24) as u32);
        v
    }

    #[test]
    fn walks_the_resource_tree() {
        let img = sample_tree();
        let f = find_in(&img, 0, &Key::Id(RT_STRING), &Key::Id(7), 0).unwrap();
        assert_eq!((f.rva, f.size), (0x1000, 8));
        let d = find_in(&img, 0, &Key::Name("dialog".into()), &Key::Id(1), 0x409).unwrap();
        assert_eq!((d.rva, d.size), (0x2000, 4));
        assert!(find_in(&img, 0, &Key::Id(RT_ICON), &Key::Id(1), 0).is_none());
        assert!(find_in(&img, 0, &Key::Id(RT_STRING), &Key::Id(8), 0).is_none());
        assert_eq!(names_in(&img, 0, &Key::Id(RT_STRING)), vec![Key::Id(7)]);
    }

    #[test]
    fn a_damaged_tree_is_rejected_not_followed() {
        let mut img = sample_tree();
        assert!(find_in(&img[..20], 0, &Key::Id(RT_STRING), &Key::Id(7), 0).is_none());
        for b in img.iter_mut().skip(16) {
            *b = 0xff;
        }
        assert!(find_in(&img, 0, &Key::Id(RT_STRING), &Key::Id(7), 0).is_none());
    }

    #[test]
    fn reads_strings_from_a_block() {
        let mut block = Vec::new();
        for i in 0..16u16 {
            let s: Vec<u16> = if i == 3 { "Hello".encode_utf16().collect() } else { Vec::new() };
            le16(&mut block, s.len() as u16);
            for c in s {
                le16(&mut block, c);
            }
        }
        assert_eq!(String::from_utf16(&string_in_block(&block, 16 * 5 + 3).unwrap()).unwrap(), "Hello");
        assert!(string_in_block(&block, 4).unwrap().is_empty());
        assert!(string_in_block(&block[..10], 3).is_none());
    }

    /// A 2x2 32-bit icon image: the alpha comes from the colour bytes, the AND mask is ignored.
    fn icon_2x2_32bit() -> Vec<u8> {
        let mut d = Vec::new();
        le32(&mut d, 40);
        le32(&mut d, 2);
        le32(&mut d, 4); // height covers colour + mask
        le16(&mut d, 1);
        le16(&mut d, 32);
        for _ in 0..6 {
            le32(&mut d, 0);
        }
        // bottom row first: B G R A
        d.extend_from_slice(&[0, 0, 255, 255, 0, 255, 0, 255]); // red, green (opaque)
        d.extend_from_slice(&[255, 0, 0, 128, 255, 255, 255, 0]); // blue half, white transparent
        d.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]); // mask rows (4 bytes each)
        d
    }

    #[test]
    fn decodes_a_32_bit_icon_with_alpha() {
        let icon = decode_icon(&icon_2x2_32bit()).unwrap();
        assert_eq!((icon.w, icon.h), (2, 2));
        // top row is the second stored row (bottom-up)
        assert_eq!(icon.rgb[0], 0x0000ff);
        assert_eq!(icon.alpha[0], 128);
        assert_eq!(icon.alpha[1], 0);
        assert_eq!(icon.rgb[2], 0xff0000);
        assert_eq!(icon.rgb[3], 0x00ff00);
        assert_eq!(icon.alpha[2], 255);
    }

    #[test]
    fn a_24_bit_icon_takes_its_transparency_from_the_mask() {
        let mut d = Vec::new();
        le32(&mut d, 40);
        le32(&mut d, 2);
        le32(&mut d, 4);
        le16(&mut d, 1);
        le16(&mut d, 24);
        for _ in 0..6 {
            le32(&mut d, 0);
        }
        // rows are padded to 8 bytes; bottom row first
        d.extend_from_slice(&[0, 0, 255, 0, 0, 255, 0, 0]);
        d.extend_from_slice(&[255, 0, 0, 255, 0, 0, 0, 0]);
        // mask: bottom row nothing masked, top row: left pixel masked (bit 7)
        d.extend_from_slice(&[0, 0, 0, 0]);
        d.extend_from_slice(&[0x80, 0, 0, 0]);
        let icon = decode_icon(&d).unwrap();
        assert_eq!(icon.alpha, vec![0, 255, 255, 255]);
        assert_eq!(icon.rgb[2], 0xff0000);
    }

    #[test]
    fn picks_the_nearest_icon_size() {
        let e = |w: i32, bits: u16, id: u32| IconEntry { w, h: w, bits, id, bytes: 0 };
        let list = [e(16, 32, 1), e(32, 8, 2), e(32, 32, 3), e(48, 32, 4)];
        assert_eq!(best_entry(&list, 32).unwrap().id, 3);
        assert_eq!(best_entry(&list, 20).unwrap().id, 3, "the smallest larger size");
        assert_eq!(best_entry(&list, 64).unwrap().id, 4, "the largest when nothing is big enough");
        assert_eq!(best_entry(&list, 16).unwrap().id, 1);
    }

    #[test]
    fn parses_icon_groups_and_ico_files() {
        let mut g = Vec::new();
        le16(&mut g, 0);
        le16(&mut g, 1);
        le16(&mut g, 1);
        g.extend_from_slice(&[0, 0, 0, 0, 1, 0, 32, 0]);
        le32(&mut g, 1234);
        le16(&mut g, 7);
        let e = parse_group(&g);
        assert_eq!(e, vec![IconEntry { w: 256, h: 256, bits: 32, id: 7, bytes: 1234 }]);
        let mut f = Vec::new();
        le16(&mut f, 0);
        le16(&mut f, 1);
        le16(&mut f, 1);
        f.extend_from_slice(&[16, 16, 0, 0, 1, 0, 8, 0]);
        le32(&mut f, 99);
        le32(&mut f, 22);
        assert_eq!(parse_ico_file(&f), vec![IconEntry { w: 16, h: 16, bits: 8, id: 22, bytes: 99 }]);
        assert!(parse_ico_file(b"not an icon").is_empty());
    }

    #[test]
    fn decodes_a_bitmap_resource() {
        let mut d = Vec::new();
        le32(&mut d, 40);
        le32(&mut d, 2);
        le32(&mut d, 1);
        le16(&mut d, 1);
        le16(&mut d, 24);
        for _ in 0..6 {
            le32(&mut d, 0);
        }
        d.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]); // red, green, pad
        let s = bitmap_from_dib(&d).unwrap();
        assert_eq!((s.w, s.h), (2, 1));
        assert_eq!(s.get(0, 0), 0xff0000);
        assert_eq!(s.get(1, 0), 0x00ff00);
        assert!(bitmap_from_dib(&d[..45]).is_none());
        let mut file = b"BM".to_vec();
        file.extend_from_slice(&[0; 12]);
        file.extend_from_slice(&d);
        assert!(bitmap_from_file(&file).is_some());
    }
}
