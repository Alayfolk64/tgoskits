//! Exercises Linux open flags through the production file wrapper.

use alloc::sync::Arc;
use core::{
    any::Any,
    sync::atomic::{AtomicU8, Ordering},
};

use ax_fs_ng::file::{FileBackend, FileFlags};
use axpoll::{IoEvents, Pollable, SharedRegistrationSink};
use axfs_ng_vfs::{
    DirEntry, FileNode, FileNodeOps, Filesystem, FilesystemOps, Location,
    Metadata, MetadataUpdate, Mountpoint, NodeOps, NodeType, Reference, StatFs, VfsError,
    VfsResult,
};
use linux_raw_sys::general::{O_DSYNC, O_SYNC};

use super::File;

#[axtest::axtest]
fn open_sync_flags_reach_write_completion() {
    for (flags, expected) in [(0, 0), (O_DSYNC, 1), (O_SYNC, 2), (O_SYNC & !O_DSYNC, 2)] {
        let filesystem = Filesystem::new(Arc::new(SyncProbeFs));
        let node = Arc::new(SyncProbe {
            completion: AtomicU8::new(0),
        });
        let entry = DirEntry::new_file(
            FileNode::new(node.clone()),
            NodeType::RegularFile,
            Reference::root(),
        );
        let location = Location::new(Mountpoint::new_root(&filesystem), entry);
        let file = File::new(
            ax_fs_ng::File::new(FileBackend::Direct(location), FileFlags::WRITE),
            flags,
        );
        assert_eq!(file.inner().write_at(b"build".as_slice(), 0), Ok(5));
        assert_eq!(node.completion.load(Ordering::Relaxed), expected);
        assert_eq!(file.inner().position(), Some(0));
    }
}

struct SyncProbeFs;

impl FilesystemOps for SyncProbeFs {
    fn name(&self) -> &str {
        "write-sync-probe"
    }
    fn root_dir(&self) -> DirEntry {
        DirEntry::new_file(
            FileNode::new(Arc::new(SyncProbe {
                completion: AtomicU8::new(0),
            })),
            NodeType::RegularFile,
            Reference::root(),
        )
    }
    fn stat(&self) -> VfsResult<StatFs> {
        Err(VfsError::OperationNotSupported)
    }
}

struct SyncProbe {
    completion: AtomicU8,
}

impl NodeOps for SyncProbe {
    fn inode(&self) -> u64 {
        1
    }
    fn metadata(&self) -> VfsResult<Metadata> {
        Err(VfsError::OperationNotSupported)
    }
    fn update_metadata(&self, _: MetadataUpdate) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }
    fn filesystem(&self) -> &dyn FilesystemOps {
        &SyncProbeFs
    }
    fn sync(&self, data_only: bool) -> VfsResult<()> {
        self.completion
            .store(if data_only { 1 } else { 2 }, Ordering::Relaxed);
        Ok(())
    }
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

impl Pollable for SyncProbe {
    fn poll(&self) -> IoEvents {
        IoEvents::OUT
    }
    unsafe fn register_shared(&self, _: &mut dyn SharedRegistrationSink, _: IoEvents) {}
}

impl FileNodeOps for SyncProbe {
    fn read_at(&self, _: &mut [u8], _: u64) -> VfsResult<usize> {
        Err(VfsError::OperationNotSupported)
    }
    fn write_at(&self, buf: &[u8], _: u64) -> VfsResult<usize> {
        Ok(buf.len())
    }
    fn append(&self, _: &[u8]) -> VfsResult<(usize, u64)> {
        Err(VfsError::OperationNotSupported)
    }
    fn set_len(&self, _: u64) -> VfsResult<()> {
        Err(VfsError::OperationNotSupported)
    }
}
