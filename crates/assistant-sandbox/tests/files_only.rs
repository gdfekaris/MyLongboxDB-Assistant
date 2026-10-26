//! A thread limited to the files it works on, made for the purpose in
//! each test, since its limits cannot be lifted.

use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;

use assistant_sandbox::{Enforced, confine_to_files};

fn denied<T: std::fmt::Debug>(result: std::io::Result<T>) -> bool {
    matches!(&result, Err(e) if e.kind() == ErrorKind::PermissionDenied)
}

/// A folder of its own for each test, holding a file to read and a
/// folder to write in.
fn folders(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&root);
    let out = root.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let input = root.join("input.db");
    std::fs::write(&input, b"input").unwrap();
    std::fs::write(root.join("other.txt"), b"other").unwrap();
    (root, input, out)
}

fn limited<T: Send + 'static>(
    read: PathBuf,
    write: PathBuf,
    work: impl FnOnce(Enforced) -> T + Send + 'static,
) -> T {
    std::thread::spawn(move || work(confine_to_files(&[&read], &[&write]).unwrap()))
        .join()
        .unwrap()
}

#[test]
fn the_thread_reads_its_input_and_writes_only_its_folder() {
    let (root, input, out) = folders("files-only-reach");
    let other = root.join("other.txt");
    limited(input.clone(), out.clone(), move |enforced| {
        assert_eq!(std::fs::read(&input).unwrap(), b"input");
        std::fs::write(out.join("index.sqlite"), b"index").unwrap();
        std::fs::rename(out.join("index.sqlite"), out.join("kept.sqlite")).unwrap();
        if enforced == Enforced::Not {
            eprintln!("note: Landlock is not available; only the filter applied");
            return;
        }
        assert!(denied(std::fs::read(&other)));
        assert!(denied(std::fs::write(&input, b"changed")));
        assert!(denied(std::fs::File::create(root.join("new.txt"))));
        assert!(denied(std::fs::read(env!("CARGO_MANIFEST_PATH"))));
    });
    // The rest of the process is as it was.
    std::fs::read(env!("CARGO_MANIFEST_PATH")).unwrap();
}

#[test]
fn the_thread_makes_no_sockets_and_starts_no_programs() {
    let (_, input, out) = folders("files-only-sockets");
    let (open, _other) = UnixDatagram::pair().unwrap();
    limited(input, out, move |_| {
        assert!(denied(UnixDatagram::pair()));
        assert!(denied(UnixDatagram::unbound()));
        // Sending is refused; a plain write is not, as the thread writes
        // its own files.
        let message = b"x";
        // SAFETY: send() reads only the buffer passed, which outlives the
        // call, on a descriptor this test owns.
        let sent = unsafe { libc::send(open.as_raw_fd(), message.as_ptr().cast(), 1, 0) };
        assert_eq!(sent, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EACCES)
        );
        // Any program would do; it is refused before it is looked for.
        #[allow(clippy::disallowed_methods, clippy::disallowed_types)]
        let started = std::process::Command::new("/bin/true").status();
        assert!(denied(started));
    });
}
