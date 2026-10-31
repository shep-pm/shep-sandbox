//! Sockets from `/proc`: the inodes a sheep holds as file descriptors,
//! matched against the namespace's TCP tables.

use std::{
    collections::HashSet,
    fs, io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

use super::{Error, Socket};

const LISTEN: &str = "0A";
const ESTABLISHED: &str = "01";

/// The sockets of `pids`, read from `/proc`.
pub fn read(pids: &[u32]) -> Result<Vec<Socket>, Error> {
    let mut inodes = HashSet::new();
    for (at, pid) in pids.iter().enumerate() {
        let dir = format!("/proc/{pid}/fd");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound && at == 0 => {
                return Err(Error::Gone);
            }
            // A child that exited since the shepherd listed it.
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => return Err(Error::Proc { path: dir, source }),
        };
        // A descriptor can close between the listing and the read.
        inodes.extend(
            entries
                .flatten()
                .filter_map(|entry| fs::read_link(entry.path()).ok())
                .filter_map(|target| socket_inode(&target.to_string_lossy())),
        );
    }

    // The tables are the network namespace's, the same for every pid in it.
    let Some(first) = pids.first() else {
        return Ok(Vec::new());
    };
    let mut sockets = Vec::new();
    for (name, v6) in [("tcp", false), ("tcp6", true)] {
        let path = format!("/proc/{first}/net/{name}");
        match fs::read_to_string(&path) {
            Ok(text) => sockets.extend(self::sockets(&text, v6, &inodes)),
            // No IPv6 on this host.
            Err(err) if err.kind() == io::ErrorKind::NotFound && !v6 => return Err(Error::Gone),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(Error::Proc { path, source }),
        }
    }
    Ok(sockets)
}

/// The inode in an fd link target such as `socket:[40001]`.
pub fn socket_inode(target: &str) -> Option<u64> {
    target
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// The listening and established rows of one `/proc/net/tcp` or `tcp6`
/// table whose inode is in `inodes`.
pub fn sockets(table: &str, v6: bool, inodes: &HashSet<u64>) -> Vec<Socket> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| row(line, v6, inodes))
        .collect()
}

fn row(line: &str, v6: bool, inodes: &HashSet<u64>) -> Option<Socket> {
    let mut fields = line.split_whitespace();
    let local = address(fields.nth(1)?, v6)?;
    let remote = address(fields.next()?, v6)?;
    let state = fields.next()?;
    // After the state: tx:rx queues, timer, retransmits, uid, timeout, inode.
    let inode: u64 = fields.nth(5)?.parse().ok()?;
    if !inodes.contains(&inode) {
        return None;
    }
    match state {
        LISTEN => Some(Socket::Listening(local)),
        ESTABLISHED => Some(Socket::Connected { local, remote }),
        _ => None,
    }
}

/// `ADDRESS:PORT`, the address as native-endian 32-bit words in hex.
fn address(text: &str, v6: bool) -> Option<SocketAddr> {
    let (ip, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let ip = if v6 {
        if ip.len() != 32 {
            return None;
        }
        let mut octets = [0u8; 16];
        for word in 0..4 {
            let value = u32::from_str_radix(ip.get(word * 8..word * 8 + 8)?, 16).ok()?;
            octets[word * 4..word * 4 + 4].copy_from_slice(&value.to_ne_bytes());
        }
        IpAddr::V6(Ipv6Addr::from(octets))
    } else {
        let value = u32::from_str_radix(ip, 16).ok()?;
        IpAddr::V4(Ipv4Addr::from(value.to_ne_bytes()))
    };
    Some(SocketAddr::new(ip.to_canonical(), port))
}
