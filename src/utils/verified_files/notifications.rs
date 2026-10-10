use std::collections::HashMap;
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};

use super::Stamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Ticket {
    watch: i32,
    serial: u64,
}

pub(super) struct Notifications {
    file: File,
    watches: HashMap<i32, (Stamp, Ticket)>,
    serial: u64,
}

impl Notifications {
    pub(super) fn new() -> io::Result<Self> {
        // Own the descriptor and prevent it leaking into launched tools.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            // A successful inotify_init1 transfers a fresh descriptor to this File.
            file: unsafe { File::from_raw_fd(fd) },
            watches: HashMap::new(),
            serial: 0,
        })
    }

    pub(super) fn len(&self) -> usize {
        self.watches.len()
    }

    pub(super) fn valid(&self, ticket: Ticket) -> bool {
        self.watches
            .get(&ticket.watch)
            .is_some_and(|(_, current)| *current == ticket)
    }

    pub(super) fn watch(&mut self, file: &File, stamp: Stamp) -> io::Result<Option<Ticket>> {
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // The live file descriptor and full statfs allocation remain valid for this call.
        if unsafe { libc::fstatfs(file.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // fstatfs initialized the allocation after returning success.
        let filesystem = unsafe { filesystem.assume_init() };
        if !supported(filesystem.f_type) {
            return Ok(None);
        }
        // Follow our owned descriptor to watch the opened inode, even after a rename.
        let path = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd()))?;
        let mask = libc::IN_MODIFY
            | libc::IN_CLOSE_WRITE
            | libc::IN_ATTRIB
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF;
        // Both the notification descriptor and NUL-terminated path are valid here.
        let watch = unsafe { libc::inotify_add_watch(self.file.as_raw_fd(), path.as_ptr(), mask) };
        if watch < 0 {
            return Err(io::Error::last_os_error());
        }
        if let Some((previous, ticket)) = self.watches.get(&watch)
            && *previous == stamp
        {
            return Ok(Some(*ticket));
        }
        self.serial = self.serial.wrapping_add(1);
        let ticket = Ticket {
            watch,
            serial: self.serial,
        };
        self.watches.insert(watch, (stamp, ticket));
        Ok(Some(ticket))
    }

    pub(super) fn refresh(&mut self) -> io::Result<()> {
        let mut buffer = [0u8; 8192];
        loop {
            let length = match self.file.read(&mut buffer) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(length) => length,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            self.consume(&buffer[..length])?;
        }
    }

    fn consume(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut offset = 0;
        let header = std::mem::size_of::<libc::inotify_event>();
        while offset < bytes.len() {
            if bytes.len() - offset < header {
                return Err(io::ErrorKind::InvalidData.into());
            }
            // The checked slice contains a complete header; the byte buffer need not be aligned.
            let event = unsafe {
                bytes
                    .as_ptr()
                    .add(offset)
                    .cast::<libc::inotify_event>()
                    .read_unaligned()
            };
            if event.mask & libc::IN_Q_OVERFLOW != 0 {
                return Err(io::ErrorKind::InvalidData.into());
            }
            let length = header
                .checked_add(event.len as usize)
                .ok_or(io::ErrorKind::InvalidData)?;
            if length > bytes.len() - offset {
                return Err(io::ErrorKind::InvalidData.into());
            }
            if self.watches.remove(&event.wd).is_some() && event.mask & libc::IN_IGNORED == 0 {
                // The removed watch belongs to this instance; queued IN_IGNORED is drained too.
                let result = unsafe { libc::inotify_rm_watch(self.file.as_raw_fd(), event.wd) };
                if result != 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EINVAL) {
                    return Err(io::Error::last_os_error());
                }
            }
            offset += length;
        }
        Ok(())
    }
}

fn supported(kind: libc::c_long) -> bool {
    // Remote and FUSE changes need not produce local notifications.
    matches!(
        kind,
        libc::EXT4_SUPER_MAGIC
            | libc::BTRFS_SUPER_MAGIC
            | libc::XFS_SUPER_MAGIC
            | libc::TMPFS_MAGIC
            | libc::OVERLAYFS_SUPER_MAGIC
            | 0x2fc12fc1
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn overflow_and_malformed_events_cannot_authorize_reuse() -> io::Result<()> {
        let mut notifications = Notifications::new()?;
        let mut overflow = Vec::new();
        overflow.extend_from_slice(&(-1i32).to_ne_bytes());
        overflow.extend_from_slice(&libc::IN_Q_OVERFLOW.to_ne_bytes());
        overflow.extend_from_slice(&0u32.to_ne_bytes());
        overflow.extend_from_slice(&0u32.to_ne_bytes());
        assert!(notifications.consume(&overflow).is_err());
        assert!(notifications.consume(&[0; 3]).is_err());
        overflow[4..8].copy_from_slice(&libc::IN_MODIFY.to_ne_bytes());
        overflow[12..16].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(notifications.consume(&overflow).is_err());
        Ok(())
    }

    // @variants: both
    #[test]
    fn remote_and_portal_filesystems_require_full_reads() {
        assert!(!supported(libc::NFS_SUPER_MAGIC));
        assert!(!supported(libc::FUSE_SUPER_MAGIC));
        assert!(!supported(0));
        assert!(supported(libc::EXT4_SUPER_MAGIC));
    }
}
