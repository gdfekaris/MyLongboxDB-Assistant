//! The socket filter reaches threads that were running before it was
//! applied. A test process of its own, so the filter is surely applied
//! after the thread starts.

#[test]
fn threads_started_earlier_are_covered_too() {
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let earlier = std::thread::spawn(move || {
        wait.recv().unwrap();
        // SAFETY: socket() takes plain integers and returns a descriptor
        // or -1; nothing is read or written through memory.
        let fd = unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_DGRAM, 0) };
        (fd, std::io::Error::last_os_error().raw_os_error())
    });
    assistant_sandbox::limit_sockets().unwrap();
    go.send(()).unwrap();
    assert_eq!(earlier.join().unwrap(), (-1, Some(libc::EACCES)));
}
