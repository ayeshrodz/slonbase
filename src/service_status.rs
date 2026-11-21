//! Reports what the process is doing to the service manager (systemd's `STATUS=` line, shown
//! by `systemctl status`). Without `NOTIFY_SOCKET` in the environment, or on a platform
//! without Unix sockets, it does nothing.

/// Sets the status line the service manager shows for this process (an empty string clears
/// it). Errors are ignored: the status is informational.
pub fn set(status: &str) {
    #[cfg(unix)]
    {
        if let Some(path) = std::env::var_os("NOTIFY_SOCKET") {
            send(&path, &format!("STATUS={}", one_line(status)));
        }
    }
    #[cfg(not(unix))]
    let _ = status;
}

/// The status as one line, which is what the protocol carries.
fn one_line(status: &str) -> String {
    status.replace(['\n', '\r'], " ")
}

#[cfg(unix)]
fn send(path: &std::ffi::OsStr, message: &str) {
    use std::os::unix::{ffi::OsStrExt, net::UnixDatagram};
    let bytes = path.as_bytes();
    let Ok(socket) = UnixDatagram::unbound() else {
        return;
    };
    // a leading '@' names an abstract socket (Linux)
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if let Some(name) = bytes.strip_prefix(b"@") {
        use std::os::linux::net::SocketAddrExt;
        if let Ok(addr) = std::os::unix::net::SocketAddr::from_abstract_name(name) {
            let _ = socket.send_to_addr(message.as_bytes(), &addr);
        }
        return;
    }
    let sent = socket.send_to(message.as_bytes(), std::path::Path::new(path));
    let _ = sent;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_status_reaches_the_notify_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify");
        let listener = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        listener
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        send(path.as_os_str(), "STATUS=waiting for the database");
        let mut buf = [0u8; 128];
        let n = listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"STATUS=waiting for the database");
    }

    #[test]
    fn a_status_is_one_line() {
        assert_eq!(one_line("a\nb\r\nc"), "a b  c");
    }
}
