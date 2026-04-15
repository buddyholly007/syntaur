//! SSH client — spawns `ssh` as a child process with PTY.
//!
//! Uses the system SSH binary which automatically handles known_hosts,
//! agent forwarding, and config from ~/.ssh/config. The child gets a
//! PTY via the same openpty/fork pattern as local terminals.

use bytes::Bytes;
use log::{error, info};
use tokio::sync::{broadcast, mpsc};

/// A connected SSH session with I/O channels.
pub struct SshClient {
    pub output_tx: broadcast::Sender<Bytes>,
    pub input_tx: mpsc::Sender<Bytes>,
    master_fd: std::os::unix::io::RawFd,
    child_pid: u32,
}

impl SshClient {
    pub async fn resize(&self, cols: u16, rows: u16) {
        super::pty::resize_pty(self.master_fd, cols, rows);
    }

    pub async fn close(&self) {
        super::pty::kill_pty(self.child_pid);
        unsafe { libc::close(self.master_fd); }
    }
}

/// Connect to a remote host by spawning `ssh` with a PTY.
pub async fn connect_ssh(
    hostname: &str,
    port: u16,
    username: &str,
    key_path: &str,
    cols: u16,
    rows: u16,
) -> Result<SshClient, String> {
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

    let ssh_cmd = format!(
        "ssh -o StrictHostKeyChecking=no -o BatchMode=yes -i {} -p {} -t {}@{}",
        key_path, port, username, hostname
    );

    let pid = unsafe { libc::fork() };
    match pid {
        -1 => {
            unsafe { libc::close(master); libc::close(slave); }
            Err(format!("fork failed: {}", std::io::Error::last_os_error()))
        }
        0 => {
            // Child
            unsafe {
                libc::close(master);
                libc::setsid();
                libc::ioctl(slave, libc::TIOCSCTTY as _, 0);
                libc::dup2(slave, 0);
                libc::dup2(slave, 1);
                libc::dup2(slave, 2);
                if slave > 2 { libc::close(slave); }
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGHUP);

                let term = std::ffi::CString::new("TERM=xterm-256color").unwrap();
                libc::putenv(term.as_ptr() as *mut _);

                let sh = std::ffi::CString::new("/bin/sh").unwrap();
                let c_flag = std::ffi::CString::new("-c").unwrap();
                let cmd = std::ffi::CString::new(ssh_cmd).unwrap();
                libc::execl(sh.as_ptr(), sh.as_ptr(), c_flag.as_ptr(), cmd.as_ptr(), std::ptr::null::<libc::c_char>());
                libc::_exit(1);
            }
        }
        child_pid => {
            unsafe { libc::close(slave); }

            // Non-blocking
            unsafe {
                let flags = libc::fcntl(master, libc::F_GETFL);
                libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }

            let (output_tx, _) = broadcast::channel(256);
            let (input_tx, input_rx) = mpsc::channel::<Bytes>(256);

            // Reader task
            let out_tx = output_tx.clone();
            let fd = master;
            tokio::spawn(async move {
                super::pty::pty_reader_fd(fd, out_tx).await;
            });

            // Writer task
            tokio::spawn(async move {
                super::pty::pty_writer_fd(fd, input_rx).await;
            });

            info!("[terminal:ssh] spawned ssh {}@{}:{} (pid={})", username, hostname, port, child_pid);

            Ok(SshClient {
                output_tx,
                input_tx,
                master_fd: master,
                child_pid: child_pid as u32,
            })
        }
    }
}
