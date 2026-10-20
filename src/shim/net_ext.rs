//! Network configuration for programs that list interfaces (`GetAdaptersAddresses`, `if_nametoindex`, ...), built from
//! `getifaddrs`, and the wide-character name lookup of Winsock.

use crate::runtime::strings::*;
use crate::win::*;
use std::collections::BTreeMap;

const AF_INET: u16 = 2;
const AF_INET6: u16 = 23;
const NO_ERROR: u32 = 0;
const ERROR_BUFFER_OVERFLOW: u32 = 111;
const ERROR_NO_DATA: u32 = 232;
const GAA_FLAG_SKIP_UNICAST: u32 = 0x1;

const ADAPTER_SIZE: usize = 448;
const UNICAST_SIZE: usize = 64;
const SOCKADDR_IN_SIZE: usize = 16;
const SOCKADDR_IN6_SIZE: usize = 28;

struct Iface {
    name: String,
    index: u32,
    flags: u32,
    mtu: u32,
    mac: [u8; 6],
    loopback: bool,
    /// (family, address bytes, prefix length)
    addrs: Vec<(u16, Vec<u8>, u8)>,
}

fn prefix_len(mask: &[u8]) -> u8 {
    mask.iter().map(|b| b.count_ones() as u8).sum()
}

unsafe fn interfaces() -> Vec<Iface> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    if libc::getifaddrs(&mut head) != 0 {
        return Vec::new();
    }
    let mut by_name: BTreeMap<String, Iface> = BTreeMap::new();
    let mut p = head;
    while !p.is_null() {
        let ifa = &*p;
        p = ifa.ifa_next;
        let name = std::ffi::CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
        let entry = by_name.entry(name.clone()).or_insert_with(|| {
            let c = std::ffi::CString::new(name.clone()).unwrap();
            Iface { index: libc::if_nametoindex(c.as_ptr()), name, flags: ifa.ifa_flags, mtu: 1500, mac: [0; 6], loopback: ifa.ifa_flags & libc::IFF_LOOPBACK as u32 != 0, addrs: Vec::new() }
        });
        if ifa.ifa_addr.is_null() {
            continue;
        }
        match (*ifa.ifa_addr).sa_family as i32 {
            libc::AF_INET => {
                let a = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let mask = if ifa.ifa_netmask.is_null() { [255u8; 4] } else { (*(ifa.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr.to_ne_bytes() };
                entry.addrs.push((AF_INET, a.sin_addr.s_addr.to_ne_bytes().to_vec(), prefix_len(&mask)));
            }
            libc::AF_INET6 => {
                let a = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                let mask = if ifa.ifa_netmask.is_null() { [255u8; 16] } else { (*(ifa.ifa_netmask as *const libc::sockaddr_in6)).sin6_addr.s6_addr };
                entry.addrs.push((AF_INET6, a.sin6_addr.s6_addr.to_vec(), prefix_len(&mask)));
            }
            libc::AF_PACKET => {
                let ll = &*(ifa.ifa_addr as *const libc::sockaddr_ll);
                entry.mac.copy_from_slice(&ll.sll_addr[..6]);
            }
            _ => {}
        }
    }
    libc::freeifaddrs(head);
    let mut list: Vec<Iface> = by_name.into_values().filter(|i| i.index != 0).collect();
    list.sort_by_key(|i| i.index);
    list
}

/// Writes `n` bytes of zeroes at the end of `out`, returning the offset; everything the call hands out lives in one block.
fn alloc(out: &mut Vec<u8>, n: usize) -> usize {
    while out.len() % 8 != 0 {
        out.push(0);
    }
    let at = out.len();
    out.resize(at + n, 0);
    at
}

fn put<const N: usize>(out: &mut [u8], at: usize, v: [u8; N]) {
    out[at..at + N].copy_from_slice(&v);
}

/// Builds the list as one block whose internal pointers are offsets from 0, then rebases them onto `base`.
unsafe fn build(list: &[Iface], flags: u32, family: u32, base: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut ptr_fixups: Vec<usize> = Vec::new();
    let mut prev_next: Option<usize> = None;
    for i in list {
        let a = alloc(&mut out, ADAPTER_SIZE);
        put(&mut out, a, (ADAPTER_SIZE as u32).to_le_bytes());
        put(&mut out, a + 4, i.index.to_le_bytes());
        if let Some(p) = prev_next {
            put(&mut out, p, (a as u64).to_le_bytes());
            ptr_fixups.push(p);
        }
        prev_next = Some(a + 8);
        // names: the adapter name as ASCII (a GUID on Windows), friendly name and description as UTF-16
        let name = alloc(&mut out, i.name.len() + 1);
        out[name..name + i.name.len()].copy_from_slice(i.name.as_bytes());
        put(&mut out, a + 16, (name as u64).to_le_bytes());
        ptr_fixups.push(a + 16);
        let wide: Vec<u8> = to_wide_nul(&i.name).iter().flat_map(|u| u.to_le_bytes()).collect();
        let friendly = alloc(&mut out, wide.len());
        out[friendly..friendly + wide.len()].copy_from_slice(&wide);
        put(&mut out, a + 72, (friendly as u64).to_le_bytes());
        put(&mut out, a + 64, (friendly as u64).to_le_bytes());
        ptr_fixups.push(a + 72);
        ptr_fixups.push(a + 64);
        let empty = alloc(&mut out, 2);
        put(&mut out, a + 56, (empty as u64).to_le_bytes());
        ptr_fixups.push(a + 56);
        out[a + 80..a + 86].copy_from_slice(&i.mac);
        put(&mut out, a + 88, 6u32.to_le_bytes());
        put(&mut out, a + 92, 0x0000_0010u32.to_le_bytes()); // IP_ADAPTER_NO_MULTICAST ...
        put(&mut out, a + 96, i.mtu.to_le_bytes());
        put(&mut out, a + 100, (if i.loopback { 24u32 } else { 6u32 }).to_le_bytes());
        let up = i.flags & libc::IFF_UP as u32 != 0 && i.flags & libc::IFF_RUNNING as u32 != 0;
        put(&mut out, a + 104, (if up { 1u32 } else { 2u32 }).to_le_bytes());
        put(&mut out, a + 108, i.index.to_le_bytes());
        put(&mut out, a + 184, 1_000_000_000u64.to_le_bytes());
        put(&mut out, a + 192, 1_000_000_000u64.to_le_bytes());
        put(&mut out, a + 216, 25u32.to_le_bytes());
        put(&mut out, a + 220, 25u32.to_le_bytes());
        put(&mut out, a + 224, (0x0006_0000_0000_0000u64 | i.index as u64).to_le_bytes());
        // unicast addresses
        let mut prev_unicast_next: Option<usize> = None;
        if flags & GAA_FLAG_SKIP_UNICAST == 0 {
            for (fam, bytes, prefix) in &i.addrs {
                if family != 0 && family != *fam as u32 {
                    continue;
                }
                let u = alloc(&mut out, UNICAST_SIZE);
                put(&mut out, u, (UNICAST_SIZE as u32).to_le_bytes());
                put(&mut out, u + 4, 0x0000_0010u32.to_le_bytes()); // DNS eligible
                let sock_len = if *fam == AF_INET { SOCKADDR_IN_SIZE } else { SOCKADDR_IN6_SIZE };
                let s = alloc(&mut out, sock_len);
                put(&mut out, s, fam.to_le_bytes());
                if *fam == AF_INET {
                    out[s + 4..s + 8].copy_from_slice(bytes);
                } else {
                    out[s + 8..s + 24].copy_from_slice(bytes);
                }
                put(&mut out, u + 16, (s as u64).to_le_bytes());
                ptr_fixups.push(u + 16);
                put(&mut out, u + 24, (sock_len as i32).to_le_bytes());
                put(&mut out, u + 32, 3i32.to_le_bytes()); // IpPrefixOriginManual
                put(&mut out, u + 36, 1i32.to_le_bytes());
                put(&mut out, u + 40, 3i32.to_le_bytes()); // preferred
                put(&mut out, u + 44, 0xffff_ffffu32.to_le_bytes());
                put(&mut out, u + 48, 0xffff_ffffu32.to_le_bytes());
                out[u + 56] = *prefix;
                match prev_unicast_next {
                    None => {
                        put(&mut out, a + 24, (u as u64).to_le_bytes());
                        ptr_fixups.push(a + 24);
                    }
                    Some(p) => {
                        put(&mut out, p, (u as u64).to_le_bytes());
                        ptr_fixups.push(p);
                    }
                }
                prev_unicast_next = Some(u + 8);
            }
        }
    }
    for at in ptr_fixups {
        let off = u64::from_le_bytes(out[at..at + 8].try_into().unwrap());
        put(&mut out, at, (off + base as u64).to_le_bytes());
    }
    out
}

crate::shims! { "iphlpapi" => IPHLPAPI_SHIMS;
    fn GetAdaptersAddresses(family: u32, flags: u32, _reserved: usize, buf: *mut u8, size: *mut u32) -> u32 {
        let list = interfaces();
        if list.is_empty() {
            return ERROR_NO_DATA;
        }
        let image = build(&list, flags, family, buf as usize);
        if buf.is_null() || (*size as usize) < image.len() {
            *size = image.len() as u32;
            return ERROR_BUFFER_OVERFLOW;
        }
        std::ptr::copy_nonoverlapping(image.as_ptr(), buf, image.len());
        *size = image.len() as u32;
        NO_ERROR
    }
    fn if_nametoindex(name: LPCSTR) -> u32 {
        libc::if_nametoindex(name as *const libc::c_char)
    }
    fn if_indextoname(index: u32, name: LPSTR) -> LPSTR {
        if libc::if_indextoname(index, name as *mut libc::c_char).is_null() { std::ptr::null_mut() } else { name }
    }
    fn ConvertInterfaceIndexToLuid(index: u32, luid: *mut u64) -> u32 {
        *luid = 0x0006_0000_0000_0000u64 | index as u64;
        NO_ERROR
    }
    fn ConvertInterfaceLuidToNameW(luid: *const u64, name: LPWSTR, len: usize) -> u32 {
        let index = (*luid & 0xffff_ffff) as u32;
        let mut buf = [0i8; 32];
        if libc::if_indextoname(index, buf.as_mut_ptr()).is_null() {
            return 87;
        }
        let s = std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
        if s.len() + 1 > len {
            return 111;
        }
        return_w(name, len as u32, &s);
        NO_ERROR
    }
    fn NotifyIpInterfaceChange(_family: u16, _cb: usize, _ctx: usize, _initial: u8, handle: *mut usize) -> u32 {
        if !handle.is_null() {
            *handle = 1;
        }
        NO_ERROR
    }
    fn CancelMibChangeNotify2(_handle: usize) -> u32 {
        NO_ERROR
    }
    fn GetBestRoute2(_luid: usize, _index: u32, _src: usize, _dst: usize, _opts: u32, _route: usize, _best: usize) -> u32 {
        50
    }
}

crate::shims! { "ws2_32" => WS2_NAME_SHIMS;
    fn GetNameInfoW(addr: *const u8, alen: i32, host: LPWSTR, hlen: u32, serv: LPWSTR, slen: u32, flags: i32) -> i32 {
        let mut h = vec![0u8; hlen as usize * 4 + 1];
        let mut s = vec![0u8; slen as usize * 4 + 1];
        let rc = crate::shim::ws2_32::getnameinfo(addr, alen, h.as_mut_ptr(), h.len() as u32, s.as_mut_ptr(), s.len() as u32, flags);
        if rc != 0 {
            return rc;
        }
        if !host.is_null() && hlen > 0 {
            return_w(host, hlen, &a_to_string(h.as_ptr()));
        }
        if !serv.is_null() && slen > 0 {
            return_w(serv, slen, &a_to_string(s.as_ptr()));
        }
        0
    }
}
