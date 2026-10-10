use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const ZFS_SUPER_MAGIC: libc::c_long = 0x2fc12fc1;

pub(super) struct Notifications {
    file: File,
    watches: BTreeSet<i32>,
}

impl Notifications {
    pub(super) fn new() -> io::Result<Self> {
        // The returned descriptor is owned here and must not survive a spawned process.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            // A successful inotify_init1 transfers a fresh descriptor to this File.
            file: unsafe { File::from_raw_fd(fd) },
            watches: BTreeSet::new(),
        })
    }

    pub(super) fn len(&self) -> usize {
        self.watches.len()
    }

    pub(super) fn watch(&mut self, path: &Path) -> io::Result<bool> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // The path is NUL-terminated and the output points to a full statfs allocation.
        if unsafe { libc::statfs(path.as_ptr(), filesystem.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // statfs initialized the output after returning success.
        let filesystem = unsafe { filesystem.assume_init() };
        // FUSE and network mounts may miss notifications for changes made outside the mount.
        if !matches!(
            filesystem.f_type,
            libc::EXT4_SUPER_MAGIC
                | libc::BTRFS_SUPER_MAGIC
                | libc::XFS_SUPER_MAGIC
                | libc::TMPFS_MAGIC
                | libc::OVERLAYFS_SUPER_MAGIC
                | ZFS_SUPER_MAGIC
        ) {
            return Ok(false);
        }
        let mask = libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_MOVED_FROM
            | libc::IN_MOVED_TO
            | libc::IN_ATTRIB
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF
            | libc::IN_ONLYDIR
            | libc::IN_DONT_FOLLOW;
        // The descriptor remains owned by self and path is valid for the entire call.
        let watch = unsafe { libc::inotify_add_watch(self.file.as_raw_fd(), path.as_ptr(), mask) };
        if watch < 0 {
            return Err(io::Error::last_os_error());
        }
        self.watches.insert(watch);
        Ok(true)
    }

    pub(super) fn changed(&mut self) -> io::Result<bool> {
        let mut buffer = [0u8; 8192];
        match self.file.read(&mut buffer) {
            Ok(0) => Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(error) => Err(error),
        }
    }
}
