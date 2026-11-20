//! Network audit: TCP connections and listeners owned by given processes,
//! read from the OS tables (GetExtendedTcpTable). Used by the Privacy panel
//! to show, live, that Hyper Clips talks to nobody unless it should.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub pid: u32,
    pub local: SocketAddr,
    /// None for listening sockets.
    pub remote: Option<SocketAddr>,
    pub state: &'static str,
}

fn state_name(s: u32) -> &'static str {
    match s {
        1 => "closed",
        2 => "listen",
        3 => "syn-sent",
        4 => "syn-received",
        5 => "established",
        6 => "fin-wait-1",
        7 => "fin-wait-2",
        8 => "close-wait",
        9 => "closing",
        10 => "last-ack",
        11 => "time-wait",
        12 => "delete-tcb",
        _ => "unknown",
    }
}

fn port(p: u32) -> u16 {
    u16::from_be(p as u16)
}

fn table(family: u16) -> Vec<u8> {
    let mut size = 0u32;
    // SAFETY: first call queries the size; the second fills our buffer of that size.
    unsafe {
        let _ = GetExtendedTcpTable(
            None,
            &mut size,
            false,
            u32::from(family),
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );
        for _ in 0..3 {
            let mut buf = vec![0u8; size as usize + 4096];
            size = buf.len() as u32;
            let rc = GetExtendedTcpTable(
                Some(buf.as_mut_ptr().cast()),
                &mut size,
                false,
                u32::from(family),
                TCP_TABLE_OWNER_PID_ALL,
                0,
            );
            if rc == 0 {
                buf.truncate(size as usize);
                return buf;
            }
        }
    }
    Vec::new()
}

/// TCP sockets (IPv4 and IPv6) owned by any of `pids`.
pub fn tcp_connections(pids: &[u32]) -> Vec<Connection> {
    let mut out = Vec::new();
    let v4 = table(AF_INET.0);
    if v4.len() >= 4 {
        let n = u32::from_ne_bytes([v4[0], v4[1], v4[2], v4[3]]) as usize;
        let rows = v4[4..].as_ptr().cast::<MIB_TCPROW_OWNER_PID>();
        let max = (v4.len() - 4) / std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
        for k in 0..n.min(max) {
            // SAFETY: k < rows that fit in the buffer; read_unaligned avoids alignment issues.
            let r = unsafe { rows.add(k).read_unaligned() };
            if !pids.contains(&r.dwOwningPid) {
                continue;
            }
            let ip = |a: u32| IpAddr::V4(Ipv4Addr::from(u32::from_be(a)));
            let listen = r.dwState == 2;
            out.push(Connection {
                pid: r.dwOwningPid,
                local: SocketAddr::new(ip(r.dwLocalAddr), port(r.dwLocalPort)),
                remote: (!listen)
                    .then(|| SocketAddr::new(ip(r.dwRemoteAddr), port(r.dwRemotePort))),
                state: state_name(r.dwState),
            });
        }
    }
    let v6 = table(AF_INET6.0);
    if v6.len() >= 4 {
        let n = u32::from_ne_bytes([v6[0], v6[1], v6[2], v6[3]]) as usize;
        let rows = v6[4..].as_ptr().cast::<MIB_TCP6ROW_OWNER_PID>();
        let max = (v6.len() - 4) / std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>();
        for k in 0..n.min(max) {
            // SAFETY: as above.
            let r = unsafe { rows.add(k).read_unaligned() };
            if !pids.contains(&r.dwOwningPid) {
                continue;
            }
            let listen = r.dwState == 2;
            out.push(Connection {
                pid: r.dwOwningPid,
                local: SocketAddr::new(
                    IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr)),
                    port(r.dwLocalPort),
                ),
                remote: (!listen).then(|| {
                    SocketAddr::new(
                        IpAddr::V6(Ipv6Addr::from(r.ucRemoteAddr)),
                        port(r.dwRemotePort),
                    )
                }),
                state: state_name(r.dwState),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_own_listener() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let p = l.local_addr().unwrap().port();
        let conns = tcp_connections(&[std::process::id()]);
        assert!(
            conns
                .iter()
                .any(|c| c.local.port() == p && c.state == "listen" && c.remote.is_none()),
            "{conns:?}"
        );
        assert!(tcp_connections(&[u32::MAX - 1]).is_empty());
    }
}
