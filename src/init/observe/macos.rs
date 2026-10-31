//! Sockets from `lsof`, which ships with macOS.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    process::Command,
};

use super::{Error, Socket};

/// The sockets of `pids`, read by running `lsof`.
pub fn read(pids: &[u32]) -> Result<Vec<Socket>, Error> {
    let list: Vec<String> = pids.iter().map(u32::to_string).collect();
    let output = Command::new("lsof")
        .args(["-a", "-p", &list.join(","), "-i", "-n", "-P"])
        .output()
        .map_err(|err| Error::Lsof(err.to_string()))?;
    let sockets = sockets_of(
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
    )?;
    // No rows reads the same for a sheep with no sockets and for pids that
    // no longer exist.
    if sockets.is_empty() && pids.first().is_some_and(|pid| !alive(*pid)) {
        return Err(Error::Gone);
    }
    Ok(sockets)
}

/// Whether a process with `pid` exists. Assumed so when `ps` cannot be run.
fn alive(pid: u32) -> bool {
    Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .map_or(true, |output| output.status.success())
}

/// The sockets in what `lsof` printed, given how it exited.
///
/// It exits 1 when any listed pid holds no internet socket, with the other
/// pids' rows on stdout, or with none at all when no pid holds one.
pub fn sockets_of(code: Option<i32>, stdout: &str, stderr: &str) -> Result<Vec<Socket>, Error> {
    match code {
        Some(0 | 1) => Ok(parse(stdout)),
        Some(code) => Err(Error::Lsof(format!(
            "exit status {code}: {}",
            stderr.trim()
        ))),
        None => Err(Error::Lsof(format!("ended by a signal: {}", stderr.trim()))),
    }
}

/// The listening and established TCP rows of `lsof -i -n -P` output.
pub fn parse(output: &str) -> Vec<Socket> {
    output.lines().filter_map(row).collect()
}

fn row(line: &str) -> Option<Socket> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let [
        _command,
        _pid,
        _user,
        _fd,
        family,
        _device,
        _size,
        "TCP",
        name,
        state,
    ] = fields[..]
    else {
        return None;
    };
    let v6 = family == "IPv6";
    match state {
        "(LISTEN)" => Some(Socket::Listening(address(name, v6)?)),
        "(ESTABLISHED)" => {
            let (local, remote) = name.split_once("->")?;
            Some(Socket::Connected {
                local: address(local, v6)?,
                remote: address(remote, v6)?,
            })
        }
        _ => None,
    }
}

/// `host:port`, with `*` for every address of the family.
fn address(text: &str, v6: bool) -> Option<SocketAddr> {
    if let Some(port) = text.strip_prefix("*:") {
        let ip = if v6 {
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        };
        return Some(SocketAddr::new(ip, port.parse().ok()?));
    }
    let addr: SocketAddr = text.parse().ok()?;
    Some(SocketAddr::new(addr.ip().to_canonical(), addr.port()))
}
