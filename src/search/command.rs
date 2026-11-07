use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(super) fn read_pipe(
    mut pipe: impl Read + AsRawFd,
    maximum: usize,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> io::Result<Vec<u8>> {
    // SAFETY: pipe owns this descriptor throughout both fcntl calls. F_GETFL
    // takes no additional argument; F_SETFL takes the existing flags plus O_NONBLOCK.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }

    let limit = maximum.saturating_add(1);
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    while bytes.len() < limit {
        if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "command output did not close before the deadline",
            ));
        }
        let count = buffer.len().min(limit - bytes.len());
        match pipe.read(&mut buffer[..count]) {
            Ok(0) => break,
            Ok(count) => bytes.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(5)),
                );
            }
            Err(error) => return Err(error),
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, mpsc};

    #[test]
    fn captured_output_preserves_binary_bytes_and_eof() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        let expected = b"/tmp/\xff\0line\nnext\0";
        writer.write_all(expected).unwrap();
        drop(writer);
        let output = read_pipe(
            reader,
            1024,
            Instant::now() + Duration::from_secs(2),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn capture_stops_after_the_limit_sentinel() {
        for (maximum, expected) in [(0, 1), (3, 4), (6, 6), (10, 6)] {
            let (reader, mut writer) = UnixStream::pair().unwrap();
            writer.write_all(b"abcdef").unwrap();
            drop(writer);
            let output = read_pipe(
                reader,
                maximum,
                Instant::now() + Duration::from_secs(2),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(output, &b"abcdef"[..expected]);
        }
    }

    #[test]
    fn open_output_stream_cannot_outlive_the_deadline() {
        let (reader, writer) = UnixStream::pair().unwrap();
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = read_pipe(
                reader,
                1024,
                Instant::now() + Duration::from_millis(100),
                &AtomicBool::new(false),
            );
            sender.send(result).unwrap();
        });
        let result = receiver.recv_timeout(Duration::from_secs(2));
        // Release EOF before joining, so a regression cannot leave this test's reader blocked.
        drop(writer);
        handle.join().unwrap();
        assert_eq!(result.unwrap().unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn cancellation_stops_a_reader_before_its_deadline() {
        let (reader, writer) = UnixStream::pair().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let reader_cancelled = Arc::clone(&cancelled);
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            sender
                .send(read_pipe(
                    reader,
                    1024,
                    Instant::now() + Duration::from_secs(30),
                    &reader_cancelled,
                ))
                .unwrap();
        });
        std::thread::sleep(Duration::from_millis(20));
        cancelled.store(true, Ordering::Relaxed);
        let result = receiver.recv_timeout(Duration::from_secs(2));
        drop(writer);
        handle.join().unwrap();
        assert_eq!(result.unwrap().unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn expired_deadline_is_checked_even_when_output_is_ready() {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(b"already buffered").unwrap();
        let error = read_pipe(reader, 1024, Instant::now(), &AtomicBool::new(false)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
