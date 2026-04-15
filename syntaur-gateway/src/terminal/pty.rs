//! Local PTY allocation.
//!
//! Unix: raw POSIX openpty + fork (real implementation).
//! Windows: stubs that return errors — the Coders UI loads but local shell
//! sessions aren't supported on Windows today (no ConPTY wiring yet).

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};

#[cfg(unix)]
pub type RawFd = std::os::unix::io::RawFd;

#[cfg(not(unix))]
pub type RawFd = i32;

pub struct PtyHandle {
    pub master_fd: RawFd,
    pub child_pid: u32,
    pub output_tx: broadcast::Sender<Bytes>,
    pub input_tx: mpsc::Sender<Bytes>,
}

// ── Unix implementation ────────────────────────────────────────────────────

#[cfg(unix)]
mod imp {
    use super::{Bytes, PtyHandle, RawFd};
    use std::os::unix::io::AsRawFd;
    use std::ffi::CString;
    use log::{error, info};
    use tokio::io::unix::AsyncFd;
    use tokio::sync::{broadcast, mpsc};

    pub fn spawn_pty(shell: &str, cols: u16, rows: u16) -> Result<PtyHandle, String> {
        let mut master: libc::c_int = 0;
        let mut slave: libc::c_int = 0;

        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        let ret = unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &ws) };
        if ret != 0 {
            return Err(format!("openpty failed: {}", std::io::Error::last_os_error()));
        }

        let pid = unsafe { libc::fork() };
        match pid {
            -1 => {
                unsafe { libc::close(master); libc::close(slave); }
                Err(format!("fork failed: {}", std::io::Error::last_os_error()))
            }
            0 => {
                unsafe {
                    libc::close(master);
                    libc::setsid();
                    libc::ioctl(slave, libc::TIOCSCTTY as _, 0);

                    libc::dup2(slave, 0);
                    libc::dup2(slave, 1);
                    libc::dup2(slave, 2);
                    if slave > 2 {
                        libc::close(slave);
                    }

                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGHUP);

                    let term = CString::new("TERM=xterm-256color").unwrap();
                    libc::putenv(term.as_ptr() as *mut _);
                    let colorterm = CString::new("COLORTERM=truecolor").unwrap();
                    libc::putenv(colorterm.as_ptr() as *mut _);

                    let shell_c = CString::new(shell).unwrap();
                    libc::execl(shell_c.as_ptr(), shell_c.as_ptr(), std::ptr::null::<libc::c_char>());
                    libc::_exit(1);
                }
            }
            child_pid => {
                unsafe { libc::close(slave); }

                unsafe {
                    let flags = libc::fcntl(master, libc::F_GETFL);
                    libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
                }

                let (output_tx, _) = broadcast::channel(256);
                let (input_tx, input_rx) = mpsc::channel(256);

                let handle = PtyHandle {
                    master_fd: master,
                    child_pid: child_pid as u32,
                    output_tx: output_tx.clone(),
                    input_tx,
                };

                let out_tx = output_tx.clone();
                tokio::spawn(async move {
                    pty_reader(master, out_tx).await;
                });

                tokio::spawn(async move {
                    pty_writer(master, input_rx).await;
                });

                info!("[terminal:pty] spawned {} (pid={}, fd={})", shell, child_pid, master);
                Ok(handle)
            }
        }
    }

    pub async fn pty_reader(fd: RawFd, tx: broadcast::Sender<Bytes>) {
        let async_fd = match AsyncFd::new(fd) {
            Ok(f) => f,
            Err(e) => {
                error!("[terminal:pty] AsyncFd failed: {}", e);
                return;
            }
        };

        let mut buf = [0u8; 4096];
        loop {
            let mut guard = match async_fd.readable().await {
                Ok(g) => g,
                Err(_) => break,
            };

            match guard.try_io(|inner| {
                let n = unsafe { libc::read(inner.as_raw_fd(), buf.as_mut_ptr() as _, buf.len()) };
                if n > 0 {
                    Ok(n as usize)
                } else if n == 0 {
                    Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "pty closed"))
                } else {
                    Err(std::io::Error::last_os_error())
                }
            }) {
                Ok(Ok(n)) => {
                    let _ = tx.send(Bytes::copy_from_slice(&buf[..n]));
                }
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }
    }

    pub async fn pty_writer(fd: RawFd, mut rx: mpsc::Receiver<Bytes>) {
        while let Some(data) = rx.recv().await {
            let ptr = data.as_ptr();
            let len = data.len();
            unsafe {
                libc::write(fd, ptr as _, len);
            }
        }
    }

    pub fn resize_pty(fd: RawFd, cols: u16, rows: u16) {
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::ioctl(fd, libc::TIOCSWINSZ, &ws);
        }
    }

    pub fn kill_pty(pid: u32) {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
}

// ── Windows stub ────────────────────────────────────────────────────────────

#[cfg(not(unix))]
mod imp {
    use super::{Bytes, PtyHandle, RawFd};
    use tokio::sync::{broadcast, mpsc};

    const ERR: &str = "local PTY sessions are not supported on Windows in this build (ConPTY not wired yet) — use Remote SSH hosts instead";

    pub fn spawn_pty(_shell: &str, _cols: u16, _rows: u16) -> Result<PtyHandle, String> {
        Err(ERR.to_string())
    }

    pub async fn pty_reader(_fd: RawFd, _tx: broadcast::Sender<Bytes>) {}

    pub async fn pty_writer(_fd: RawFd, mut _rx: mpsc::Receiver<Bytes>) {}

    pub fn resize_pty(_fd: RawFd, _cols: u16, _rows: u16) {}

    pub fn kill_pty(_pid: u32) {}
}

// ── Public surface ──────────────────────────────────────────────────────────

pub use imp::{kill_pty, resize_pty, spawn_pty};

pub async fn pty_reader_fd(fd: RawFd, tx: broadcast::Sender<Bytes>) {
    imp::pty_reader(fd, tx).await;
}

pub async fn pty_writer_fd(fd: RawFd, rx: mpsc::Receiver<Bytes>) {
    imp::pty_writer(fd, rx).await;
}
