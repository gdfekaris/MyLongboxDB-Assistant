//! The file and TCP limits, applied in a test process of their own,
//! since they cannot be lifted once applied.

// The test listens and connects to check that the limits refuse it; the
// app itself never does either but through its HTTP client.
#![allow(clippy::disallowed_types)]

use std::fs;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

use assistant_sandbox::{Enforced, HTTPS_PORT, limit_access};

fn denied<T: std::fmt::Debug>(result: std::io::Result<T>) -> bool {
    matches!(&result, Err(e) if e.kind() == ErrorKind::PermissionDenied)
}

/// The Landlock ABI the kernel offers, or 0 where it has none.
fn kernel_abi() -> i64 {
    // SAFETY: with no attributes and the version flag, the call only
    // returns the ABI version; it reads and writes no memory.
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            1u32,
        )
    };
    abi.max(0)
}

/// Run as a program of its own, without the test runner's threads, since
/// the limits refuse to apply while other threads are running.
fn main() {
    files_change_only_in_the_allowed_folders_and_tcp_reaches_only_https();
    the_limits_refuse_to_apply_while_another_thread_runs();
    println!("landlock: ok");
}

fn files_change_only_in_the_allowed_folders_and_tcp_reaches_only_https() {
    // CI sets this, so that a runner kernel too old to check everything
    // below fails the run rather than skipping part of it unseen.
    let abi = kernel_abi();
    if std::env::var_os("ASSISTANT_REQUIRE_LANDLOCK_NET").is_some() {
        assert!(abi >= 4, "Landlock ABI {abi} cannot check TCP; 4 is needed");
    }
    // In the build folder, cleared at the start of each run, since once
    // the limits apply nothing outside the allowed folder can be removed.
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("landlock");
    let _ = fs::remove_dir_all(&base);
    let allowed = base.join("allowed");
    let other = base.join("other");
    fs::create_dir_all(allowed.join("inner")).unwrap();
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("kept.txt"), "kept").unwrap();
    fs::write(allowed.join("old.txt"), "old").unwrap();
    // Listening before the limits, as nothing may after them.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let other_port = listener.local_addr().unwrap().port();
    assert_ne!(other_port, HTTPS_PORT);

    let missing = PathBuf::from("/no/such/folder");
    let enforced = limit_access(&[&allowed, &missing]).unwrap();
    if enforced == Enforced::Not {
        // Nothing to check where the kernel has no Landlock; the socket
        // filter still holds.
        eprintln!("skipped: Landlock is not available on this kernel");
        return;
    }

    // Inside the allowed folder, every change works.
    fs::write(allowed.join("new.txt"), "new").unwrap();
    fs::write(allowed.join("inner/deeper.txt"), "deeper").unwrap();
    fs::rename(allowed.join("new.txt"), allowed.join("renamed.txt")).unwrap();
    fs::remove_file(allowed.join("old.txt")).unwrap();
    fs::create_dir(allowed.join("made")).unwrap();

    // Elsewhere, files can be read but not changed, made, or removed.
    assert_eq!(fs::read_to_string(other.join("kept.txt")).unwrap(), "kept");
    assert!(denied(fs::write(other.join("kept.txt"), "changed")));
    assert!(denied(fs::write(other.join("new.txt"), "new")));
    assert!(denied(fs::remove_file(other.join("kept.txt"))));
    assert!(denied(fs::create_dir(other.join("made"))));
    // Emptying a file by its path, without opening it, is refused too.
    let path = std::ffi::CString::new(other.join("kept.txt").into_os_string().into_encoded_bytes())
        .unwrap();
    // SAFETY: truncate() reads the path, a valid C string that outlives
    // the call, and writes no memory.
    let truncated = unsafe { libc::truncate(path.as_ptr(), 0) };
    assert_eq!(truncated, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EACCES)
    );
    assert!(denied(fs::rename(
        allowed.join("renamed.txt"),
        other.join("moved.txt")
    )));
    assert!(denied(fs::write(
        std::env::temp_dir().join("assistant-escape.txt"),
        "x"
    )));
    assert_eq!(fs::read_to_string(other.join("kept.txt")).unwrap(), "kept");

    // Threads started afterwards are limited too.
    let in_thread = other.join("from-thread.txt");
    let thread = std::thread::spawn(move || fs::write(in_thread, "x"));
    assert!(denied(thread.join().unwrap()));

    // TCP is limited where the kernel checks it (Landlock ABI 4 and up):
    // no listening, and connections only to the HTTPS port. Nothing
    // listens on port 443 here, so that connection is turned away by the
    // system, not by the limits.
    if abi >= 4 {
        assert!(denied(TcpListener::bind("127.0.0.1:0")));
        assert!(denied(TcpStream::connect(("127.0.0.1", other_port))));
        let https = TcpStream::connect(("127.0.0.1", HTTPS_PORT));
        assert!(!denied(https), "the HTTPS port must stay reachable");
    }
    drop(listener);
}

fn the_limits_refuse_to_apply_while_another_thread_runs() {
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let running = std::thread::spawn(move || wait.recv().unwrap());
    let refused = limit_access(&[]).unwrap_err();
    assert!(refused.to_string().contains("threads"), "{refused}");
    go.send(()).unwrap();
    running.join().unwrap();
}
