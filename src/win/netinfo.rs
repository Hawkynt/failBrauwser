//! Local network facts (Windows view): adapter addresses, the ARP table, reverse DNS —
//! the twins of `getifaddrs`, `/proc/net/arp` and `getnameinfo`.

use std::net::Ipv4Addr;

const AF_INET: u32 = 2;
// GetAdaptersAddresses/GetIpNetTable answer with bare WIN32_ERROR numbers.
const ERROR_SUCCESS: u32 = 0;
const ERROR_BUFFER_OVERFLOW: u32 = 111;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// This machine's private IPv4 networks with their prefix length.
pub fn local_networks() -> Vec<(Ipv4Addr, u32)> {
    use windows::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_FRIENDLY_NAME, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH,
    };
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER | GAA_FLAG_SKIP_FRIENDLY_NAME;
    let mut size = 15000u32;
    let mut buf = vec![0u8; size as usize];
    loop {
        let r = unsafe { GetAdaptersAddresses(AF_INET, flags, None, Some(buf.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()), &mut size) };
        if r == ERROR_SUCCESS {
            break;
        }
        if r != ERROR_BUFFER_OVERFLOW {
            return Vec::new();
        }
        if buf.len() >= size as usize {
            return Vec::new();
        }
        buf.resize(size as usize, 0);
    }
    let mut out = Vec::new();
    let mut cur = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    // SAFETY: the returned buffer is a contiguous linked list the API just built for us.
    unsafe {
        while !cur.is_null() {
            let ad = &*cur;
            let mut ua = ad.FirstUnicastAddress;
            while !ua.is_null() {
                let u = &*ua;
                let sa = u.Address.lpSockaddr;
                if !sa.is_null() && (*sa).sa_family.0 as u32 == AF_INET {
                    let d = &(*sa).sa_data;
                    let addr = Ipv4Addr::new(d[0] as u8, d[1] as u8, d[2] as u8, d[3] as u8);
                    if addr.is_private() {
                        out.push((addr, u.OnLinkPrefixLength as u32));
                    }
                }
                ua = (*ua).Next;
            }
            cur = ad.Next;
        }
    }
    out
}

/// Layout mirror of the legacy `MIB_IPNETROW`: the crate's view folds dwState/dwType
/// into one union, so the documented C layout is mirrored here (win-boundary FFI detail).
#[repr(C)]
struct IpNetRow {
    index: u32,
    phys_len: u32,
    phys: [u8; 8],
    addr: u32,
    state: u32,
    kind: u32,
}

/// The neighbor cache (ARP), complete entries only — `/proc/net/arp`'s Windows twin.
pub fn arp_table() -> Vec<Ipv4Addr> {
    use windows::Win32::NetworkManagement::IpHelper::{GetIpNetTable, MIB_IPNETTABLE};
    let mut size = 4096u32;
    let mut buf = vec![0u8; size as usize];
    loop {
        let r = unsafe { GetIpNetTable(Some(buf.as_mut_ptr().cast::<MIB_IPNETTABLE>()), &mut size, false) };
        if r == ERROR_SUCCESS {
            break;
        }
        if r != ERROR_INSUFFICIENT_BUFFER {
            return Vec::new();
        }
        if buf.len() >= size as usize {
            return Vec::new();
        }
        buf.resize(size as usize, 0);
    }
    // The rows follow the entry count; taking them by offset avoids the padded array type.
    let count = u32::from_ne_bytes(buf[..4].try_into().unwrap()) as usize;
    // SAFETY: the API filled exactly `count` rows behind the count field, laid out as mirrored.
    unsafe { std::slice::from_raw_parts(buf.as_ptr().add(std::mem::size_of::<u32>()).cast::<IpNetRow>(), count) }
        .iter()
        // State 1 is the incomplete entry ("nobody answered"), like flags 0x0 on Linux.
        .filter(|r| r.addr != 0 && r.phys_len > 0 && r.state != 1)
        .map(|r| Ipv4Addr::from(r.addr.to_ne_bytes()))
        .collect()
}

/// Reverse DNS for an address, `getnameinfo(NI_NAMEREQD)`'s Windows twin.
pub fn reverse_name(addr: Ipv4Addr) -> Option<String> {
    use windows::Win32::Networking::WinSock::{
        ADDRESS_FAMILY, IN_ADDR, IN_ADDR_0, NI_NAMEREQD, SOCKADDR, SOCKADDR_IN, socklen_t, GetNameInfoW,
    };
    winsock_init();
    let sa = SOCKADDR_IN {
        sin_family: ADDRESS_FAMILY(AF_INET as u16),
        sin_port: 0,
        sin_addr: IN_ADDR { S_un: IN_ADDR_0 { S_addr: u32::from(addr) } },
        sin_zero: [0; 8],
    };
    let mut host = [0u16; 256];
    let r = unsafe {
        GetNameInfoW(
            &sa as *const SOCKADDR_IN as *const SOCKADDR,
            socklen_t(std::mem::size_of::<SOCKADDR_IN>() as i32),
            Some(&mut host),
            None,
            NI_NAMEREQD as i32,
        )
    };
    if r != 0 {
        return None;
    }
    let end = host.iter().position(|c| *c == 0).unwrap_or(host.len());
    let name = String::from_utf16_lossy(&host[..end]);
    Some(name.trim_end_matches('.').to_string()).filter(|n| !n.is_empty())
}

fn winsock_init() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let mut data = windows::Win32::Networking::WinSock::WSADATA::default();
        let _ = windows::Win32::Networking::WinSock::WSAStartup(0x0202, &mut data);
    });
}
