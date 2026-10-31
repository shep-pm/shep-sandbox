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
    // lsof exits 1 with nothing on stdout when no process has a socket.
    if output.status.code() != Some(0)
        && !(output.status.code() == Some(1) && output.stdout.is_empty())
    {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Lsof(format!("{}: {}", output.status, stderr.trim())));
    }
    Ok(parse(&String::from_utf8_lossy(&output.stdout)))
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
    text.parse().ok()
}
