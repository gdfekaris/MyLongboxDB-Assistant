//! A confined thread, made for the purpose in each test, since its
//! limits cannot be lifted; the rest of the test process is untouched.

use std::io::{ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixDatagram, UnixStream};

use assistant_sandbox::{Enforced, confine_thread};

fn denied<T: std::fmt::Debug>(result: std::io::Result<T>) -> bool {
    matches!(&result, Err(e) if e.kind() == ErrorKind::PermissionDenied)
}

/// The error of the last call made through libc, as `errno` has it.
fn errno() -> Option<i32> {
    std::io::Error::last_os_error().raw_os_error()
}

fn confined<T: Send + 'static>(work: impl FnOnce(Enforced) -> T + Send + 'static) -> T {
    std::thread::spawn(move || work(confine_thread().unwrap()))
        .join()
        .unwrap()
}

#[test]
fn a_confined_thread_reaches_no_files() {
    let readable = env!("CARGO_MANIFEST_PATH");
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    confined(move |_| {
        assert!(denied(std::fs::read(readable)));
        assert!(denied(std::fs::File::create(dir.join("confined.txt"))));
        assert!(denied(std::fs::read_dir(&dir)));
    });
    // The rest of the process is as it was.
    std::fs::read(readable).unwrap();
}

#[test]
fn a_confined_thread_makes_and_uses_no_sockets() {
    // Made before confining, as the session bus connection is.
    let (mut open, other) = UnixStream::pair().unwrap();
    confined(move |_| {
        assert!(denied(UnixDatagram::pair()));
        assert!(denied(UnixDatagram::unbound()));
        assert!(denied(open.write(b"x")));
        drop(other);
    });
}

#[test]
fn a_confined_thread_writes_only_to_standard_output_and_error() {
    let (open, _other) = UnixStream::pair().unwrap();
    let fd = open.as_raw_fd();
    confined(move |_| {
        let message = b"confined thread writing to standard error\n";
        // SAFETY: write() reads only the buffer passed, which outlives
        // the call; the descriptor is standard error.
        let written = unsafe { libc::write(2, message.as_ptr().cast(), message.len()) };
        assert_eq!(written, message.len() as isize);
        // Putting the socket in place of standard error is refused every
        // way there is, and standard error cannot be closed.
        // SAFETY: these take and return plain integers.
        unsafe {
            assert_eq!(libc::dup(fd), -1);
            assert_eq!(errno(), Some(libc::EACCES));
            assert_eq!(libc::dup3(fd, 2, 0), -1);
            assert_eq!(errno(), Some(libc::EACCES));
            assert_eq!(libc::fcntl(fd, libc::F_DUPFD, 0), -1);
            assert_eq!(errno(), Some(libc::EACCES));
            assert_eq!(libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0), -1);
            assert_eq!(errno(), Some(libc::EACCES));
            assert_eq!(libc::close(2), -1);
            assert_eq!(errno(), Some(libc::EACCES));
        }
        drop(open);
    });
}

#[test]
fn threads_a_confined_thread_starts_are_confined_too() {
    let readable = env!("CARGO_MANIFEST_PATH");
    confined(move |_| {
        std::thread::spawn(move || assert!(denied(std::fs::read(readable))))
            .join()
            .unwrap();
    });
}

#[test]
fn file_limits_apply_where_the_kernel_has_landlock() {
    let enforced = confined(|enforced| enforced);
    if enforced == Enforced::Not {
        eprintln!("note: Landlock is not available; only the filter applied");
    }
}
