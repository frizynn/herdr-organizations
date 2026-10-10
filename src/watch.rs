//! Blocks until a directory's entries change (a file created, renamed into
//! place or removed). The popup and the dock wait on the projects root so a
//! new state file wakes them without a timer. kqueue on macOS, inotify on
//! Linux; both through `libc`.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::{Result, bail};

pub struct Watcher {
    fd: i32,
    #[cfg(target_os = "macos")]
    dir_fd: i32,
}

impl Watcher {
    pub fn new(dir: &Path) -> Result<Watcher> {
        let c_dir = CString::new(dir.as_os_str().as_bytes())?;
        Self::open(&c_dir)
    }

    #[cfg(target_os = "macos")]
    fn open(dir: &CString) -> Result<Watcher> {
        // SAFETY: plain syscalls on owned descriptors; failures are checked.
        unsafe {
            let dir_fd = libc::open(dir.as_ptr(), libc::O_EVTONLY);
            if dir_fd < 0 {
                bail!("could not open the directory to watch");
            }
            let fd = libc::kqueue();
            if fd < 0 {
                libc::close(dir_fd);
                bail!("kqueue failed");
            }
            let change = libc::kevent {
                ident: dir_fd as usize,
                filter: libc::EVFILT_VNODE,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: libc::NOTE_WRITE | libc::NOTE_DELETE | libc::NOTE_RENAME,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            if libc::kevent(fd, &change, 1, std::ptr::null_mut(), 0, std::ptr::null()) < 0 {
                libc::close(fd);
                libc::close(dir_fd);
                bail!("kevent registration failed");
            }
            Ok(Watcher { fd, dir_fd })
        }
    }

    #[cfg(target_os = "linux")]
    fn open(dir: &CString) -> Result<Watcher> {
        // SAFETY: plain syscalls on owned descriptors; failures are checked.
        unsafe {
            let fd = libc::inotify_init1(libc::IN_CLOEXEC);
            if fd < 0 {
                bail!("inotify_init1 failed");
            }
            let mask = libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_DELETE | libc::IN_CLOSE_WRITE;
            if libc::inotify_add_watch(fd, dir.as_ptr(), mask) < 0 {
                libc::close(fd);
                bail!("inotify_add_watch failed");
            }
            Ok(Watcher { fd })
        }
    }

    /// Blocks until the next change.
    pub fn wait(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        // SAFETY: `event` is a valid out-buffer for one entry.
        unsafe {
            let mut event: libc::kevent = std::mem::zeroed();
            loop {
                let n = libc::kevent(
                    self.fd,
                    std::ptr::null(),
                    0,
                    &mut event,
                    1,
                    std::ptr::null(),
                );
                if n > 0 {
                    return Ok(());
                }
                if n < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                {
                    bail!("kevent wait failed");
                }
            }
        }
        #[cfg(target_os = "linux")]
        // SAFETY: the buffer outlives the read.
        unsafe {
            let mut buf = [0u8; 4096];
            loop {
                let n = libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len());
                if n > 0 {
                    return Ok(());
                }
                if n < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                {
                    bail!("inotify read failed");
                }
            }
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // SAFETY: descriptors owned by this watcher.
        unsafe {
            libc::close(self.fd);
            #[cfg(target_os = "macos")]
            libc::close(self.dir_fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn an_atomic_replace_wakes_the_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = Watcher::new(dir.path()).unwrap();
        let path = dir.path().join("state.json");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(watcher.wait().is_ok());
        });
        std::thread::sleep(Duration::from_millis(50));
        crate::project::write_atomic(&path, b"{}").unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true));
    }
}
