//! Bindings to wepoll (Windows).

use std::convert::TryInto;
use std::io;
use std::os::windows::io::RawSocket;
use std::ptr;
use std::time::Duration;

use wepoll_sys_stjepang as we;
use winapi::um::winsock2;

use crate::sys::Event;

/// Calls a wepoll function and results in `io::Result`.
macro_rules! wepoll {
    ($fn:ident $args:tt) => {{
        let res = unsafe { we::$fn $args };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}

/// The I/O reactor.
pub struct Reactor {
    handle: we::HANDLE,
}

unsafe impl Send for Reactor {}
unsafe impl Sync for Reactor {}

impl Reactor {
    /// Creates a new reactor.
    pub fn new() -> io::Result<Reactor> {
        let handle = unsafe { we::epoll_create1(0) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Reactor { handle })
    }

    /// Inserts a socket.
    pub fn insert(&self, sock: RawSocket) -> io::Result<()> {
        // Put the socket in non-blocking mode.
        unsafe {
            let mut nonblocking = true as libc::c_ulong;
            let res = winsock2::ioctlsocket(
                sock as winsock2::SOCKET,
                winsock2::FIONBIO,
                &mut nonblocking,
            );
            if res != 0 {
                return Err(io::Error::last_os_error());
            }
        }

        // Register the socket in wepoll.
        let mut ev = we::epoll_event {
            events: 0,
            data: we::epoll_data { u64: 0u64 },
        };
        wepoll!(epoll_ctl(
            self.handle,
            we::EPOLL_CTL_ADD as libc::c_int,
            sock as we::SOCKET,
            &mut ev,
        ))?;

        Ok(())
    }

    /// Adds interest in a read/write event on a socket and associates a key with it.
    pub fn interest(&self, sock: RawSocket, key: usize, read: bool, write: bool) -> io::Result<()> {
        let mut flags = we::EPOLLONESHOT;
        if read {
            flags |= READ_FLAGS;
        }
        if write {
            flags |= WRITE_FLAGS;
        }

        let mut ev = we::epoll_event {
            events: flags as u32,
            data: we::epoll_data { u64: key as u64 },
        };
        wepoll!(epoll_ctl(
            self.handle,
            we::EPOLL_CTL_MOD as libc::c_int,
            sock as we::SOCKET,
            &mut ev,
        ))?;

        Ok(())
    }

    /// Removes a socket.
    pub fn remove(&self, sock: RawSocket) -> io::Result<()> {
        wepoll!(epoll_ctl(
            self.handle,
            we::EPOLL_CTL_DEL as libc::c_int,
            sock as we::SOCKET,
            ptr::null_mut(),
        ))?;
        Ok(())
    }

    /// Waits for I/O events with an optional timeout.
    ///
    /// Returns the number of processed I/O events.
    ///
    /// If a notification occurs, this method will return but the notification event will not be
    /// included in the `events` list nor contribute to the returned count.
    pub fn wait(&self, events: &mut Events, timeout: Option<Duration>) -> io::Result<usize> {
        // Convert the timeout to milliseconds.
        let timeout_ms = match timeout {
            None => -1,
            Some(t) => {
                if t == Duration::from_millis(0) {
                    0
                } else {
                    // Non-zero duration must be at least 1ms.
                    t.max(Duration::from_millis(1))
                        .as_millis()
                        .try_into()
                        .unwrap_or(libc::c_int::max_value())
                }
            }
        };

        // Wait for I/O events.
        events.len = wepoll!(epoll_wait(
            self.handle,
            events.list.as_mut_ptr(),
            events.list.len() as libc::c_int,
            timeout_ms,
        ))? as usize;

        Ok(events.len)
    }

    /// Sends a notification to wake up the current or next `wait()` call.
    pub fn notify(&self) -> io::Result<()> {
        unsafe {
            // This calls errors if a notification has already been posted, but that's okay.
            winapi::um::ioapiset::PostQueuedCompletionStatus(
                self.handle as winapi::um::winnt::HANDLE,
                0,
                0,
                ptr::null_mut(),
            );
        }
        Ok(())
    }
}

impl Drop for Reactor {
    fn drop(&mut self) {
        unsafe {
            we::epoll_close(self.handle);
        }
    }
}

/// Wepoll flags for all possible readability events.
const READ_FLAGS: u32 = we::EPOLLIN | we::EPOLLRDHUP | we::EPOLLHUP | we::EPOLLERR | we::EPOLLPRI;

/// Wepoll flags for all possible writability events.
const WRITE_FLAGS: u32 = we::EPOLLOUT | we::EPOLLHUP | we::EPOLLERR;

/// A list of reported I/O events.
pub struct Events {
    list: Box<[we::epoll_event]>,
    len: usize,
}

unsafe impl Send for Events {}

impl Events {
    /// Creates an empty list.
    pub fn new() -> Events {
        let ev = we::epoll_event {
            events: 0,
            data: we::epoll_data { u64: 0 },
        };
        Events {
            list: vec![ev; 1000].into_boxed_slice(),
            len: 0,
        }
    }

    /// Iterates over I/O events.
    pub fn iter(&self) -> impl Iterator<Item = Event> + '_ {
        self.list[..self.len].iter().map(|ev| Event {
            key: unsafe { ev.data.u64 } as usize,
            readable: (ev.events & READ_FLAGS) != 0,
            writable: (ev.events & WRITE_FLAGS) != 0,
        })
    }
}
