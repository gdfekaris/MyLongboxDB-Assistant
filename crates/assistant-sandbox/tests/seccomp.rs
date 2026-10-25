//! The socket filter, applied in a test process of its own, since it
//! cannot be lifted once applied.

// The test makes sockets to check which the filter lets through; the app
// itself makes none but through its HTTP client.
#![allow(clippy::disallowed_types)]

use std::io::ErrorKind;
use std::net::UdpSocket;
use std::os::unix::net::{UnixDatagram, UnixListener, UnixStream};
use std::sync::Once;

fn limited() {
    static APPLY: Once = Once::new();
    APPLY.call_once(|| assistant_sandbox::limit_sockets().unwrap());
}

/// Opens and closes a socket of `family`, or says why it could not.
fn socket(family: i32, kind: i32) -> Result<(), i32> {
    // SAFETY: socket() and close() take plain integers; nothing is read
    // or written through memory.
    unsafe {
        let fd = libc::socket(family, kind, 0);
        if fd == -1 {
            return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
        }
        libc::close(fd);
    }
    Ok(())
}

#[test]
fn sockets_the_app_does_not_need_are_refused() {
    limited();
    // Raw packets, the kernel's crypto interface, virtual machine
    // sockets, and netlink, among every family not allowed.
    for family in [
        libc::AF_PACKET,
        libc::AF_ALG,
        libc::AF_VSOCK,
        libc::AF_NETLINK,
    ] {
        assert_eq!(
            socket(family, libc::SOCK_DGRAM),
            Err(libc::EACCES),
            "family {family}"
        );
    }
}

#[test]
fn internet_and_local_sockets_still_work() {
    limited();
    // Making a socket, not reaching anywhere: Landlock decides that.
    UdpSocket::bind("127.0.0.1:0").unwrap();
    assert_eq!(socket(libc::AF_INET, libc::SOCK_STREAM), Ok(()));
    // Some test machines have no IPv6 at all; only the filter's answer
    // matters here.
    assert_ne!(socket(libc::AF_INET6, libc::SOCK_STREAM), Err(libc::EACCES));
    let (a, b) = UnixDatagram::pair().unwrap();
    a.send(b"hello").unwrap();
    let mut buf = [0; 5];
    b.recv(&mut buf).unwrap();
    assert_eq!(&buf, b"hello");
    let path = std::env::temp_dir().join(format!("assistant-sandbox-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    UnixStream::connect(&path).unwrap();
    listener.accept().unwrap();
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn io_uring_cannot_be_set_up() {
    limited();
    let mut params = [0u8; 120];
    // SAFETY: io_uring_setup reads and writes only the 120-byte
    // parameter block passed, which lives for the whole call.
    let fd = unsafe { libc::syscall(libc::SYS_io_uring_setup, 1, params.as_mut_ptr()) };
    assert_eq!(fd, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EACCES)
    );
}

#[test]
fn a_refused_socket_reads_as_not_permitted() {
    limited();
    // SAFETY: as in socket() above.
    let fd = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_RAW, 0) };
    assert_eq!(fd, -1);
    assert_eq!(
        std::io::Error::last_os_error().kind(),
        ErrorKind::PermissionDenied
    );
}
