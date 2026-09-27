//! File identities and filesystem-context lock boundaries.

use alloc::sync::Arc;
use core::{
    any::Any,
    sync::atomic::{AtomicUsize, Ordering},
};

use ax_fs_ng::vfs::{FileBackend, FileFlags, FsContext};
use axpoll::{IoEvents, Pollable, SharedRegistrationSink};
use axfs_ng_vfs::{
    DirEntry, DirEntrySink, DirNode, DirNodeOps, DirectoryCursor, FileNode, FileNodeOps,
    Filesystem, FilesystemOps, Location, Metadata, MetadataUpdate,
    Mountpoint, NodeOps, NodePermission, NodeType, Reference, RenameOptions, StatFs, VfsError,
    VfsResult,
};

use super::{
    FileLike, InodeKey,
    fs::{Directory, File, with_fs_context},
};
use crate::{StarryError, sync::FsMutex};

#[axtest::axtest]
fn fs_context_and_inode_identity_rules_hold() {
    fs_context_callback_releases_shared_directory_lock();
    fs_context_operation_keeps_its_original_directory_snapshot();
    fs_context_callback_error_preserves_shared_state();
    file_inode_key_does_not_query_metadata();
    directory_inode_key_does_not_query_metadata();
    inode_keys_keep_filesystem_and_inode_identity();
    other_filesystems_use_node_identity_without_metadata();
}

fn fs_context_callback_releases_shared_directory_lock() {
    let context = FsMutex::new(test_fs_context());

    with_fs_context(&context, linux_raw_sys::general::AT_FDCWD, |fs| {
        assert!(
            context.try_lock().is_some(),
            "path operation kept the shared filesystem-context lock"
        );
        assert!(matches!(fs.resolve("missing"), Err(VfsError::NotFound)));
        Ok(())
    })
    .unwrap();
}

fn fs_context_operation_keeps_its_original_directory_snapshot() {
    let context = FsMutex::new(test_fs_context());
    let original = context.lock().clone();
    let replacement = test_fs_context();
    let replacement_directory = replacement.current_dir().clone();

    with_fs_context(&context, linux_raw_sys::general::AT_FDCWD, |fs| {
        context
            .try_lock()
            .expect("directory updates must not wait for path I/O")
            .set_current_dir(replacement_directory.clone())?;
        assert!(Arc::ptr_eq(
            fs.current_dir().mountpoint(),
            original.current_dir().mountpoint(),
        ));
        assert!(Arc::ptr_eq(
            fs.root_dir().mountpoint(),
            original.root_dir().mountpoint(),
        ));
        assert!(Arc::ptr_eq(
            fs.mount_namespace(),
            original.mount_namespace()
        ));
        Ok(())
    })
    .unwrap();
    assert!(Arc::ptr_eq(
        context.lock().current_dir().mountpoint(),
        replacement_directory.mountpoint(),
    ));
}

fn fs_context_callback_error_preserves_shared_state() {
    let context = FsMutex::new(test_fs_context());
    let original = context.lock().clone();

    let result: crate::StarryResult<()> =
        with_fs_context(&context, linux_raw_sys::general::AT_FDCWD, |_| {
            Err(StarryError::Io)
        });

    assert!(matches!(result, Err(StarryError::Io)));
    let after = context.try_lock().unwrap();
    assert!(Arc::ptr_eq(
        after.current_dir().mountpoint(),
        original.current_dir().mountpoint(),
    ));
}

fn file_inode_key_does_not_query_metadata() {
    let filesystem = Filesystem::new(Arc::new(TestFs("ext4")));
    let mount = Mountpoint::new_root(&filesystem);
    let node = Arc::new(TestNode::new(42));
    let entry = DirEntry::new_file(
        FileNode::new(node.clone()),
        NodeType::RegularFile,
        Reference::root(),
    );
    let location = Location::new(mount.clone(), entry);
    let file = File::new(
        ax_fs_ng::File::new(FileBackend::Direct(location), FileFlags::READ),
        0,
    );

    assert_eq!(file.inode_key(), Some(InodeKey { filesystem: mount.filesystem_id(), inode: 42 }));
    assert_eq!(node.metadata_calls.load(Ordering::Relaxed), 0);
}

fn directory_inode_key_does_not_query_metadata() {
    let filesystem = Filesystem::new(Arc::new(TestFs("ext4")));
    let mount = Mountpoint::new_root(&filesystem);
    let root = filesystem.root_dir();
    let node = root.downcast::<TestNode>().unwrap();
    let directory = Directory::new(Location::new(mount.clone(), root), 0);

    assert_eq!(directory.inode_key(), Some(InodeKey { filesystem: mount.filesystem_id(), inode: node.inode() }));
    assert_eq!(node.metadata_calls.load(Ordering::Relaxed), 0);
}

fn inode_keys_keep_filesystem_and_inode_identity() {
    let filesystem = Filesystem::new(Arc::new(TestFs("ext4")));
    let first = Mountpoint::new_root(&filesystem);
    let second = Mountpoint::new_root(&filesystem);
    let node = Arc::new(TestNode::new(42));
    let entry = DirEntry::new_file(
        FileNode::new(node),
        NodeType::RegularFile,
        Reference::root(),
    );
    let make_file = |mount, entry| {
        File::new(
            ax_fs_ng::File::new(
                FileBackend::Direct(Location::new(mount, entry)),
                FileFlags::READ,
            ),
            0,
        )
    };
    let original = make_file(first.clone(), entry.clone());
    let reopened = make_file(first, entry.clone());
    let other_mount = make_file(second, entry);

    assert!(original.inode_key().is_some());
    assert_eq!(original.inode_key(), reopened.inode_key());
    assert_eq!(original.inode_key(), other_mount.inode_key());
    let other_filesystem = Filesystem::new(Arc::new(TestFs("ext4")));
    let other_mount = Mountpoint::new_root(&other_filesystem);
    let other_file = make_file(other_mount, other_filesystem.root_dir());
    assert_ne!(original.inode_key(), other_file.inode_key());
}

fn other_filesystems_use_node_identity_without_metadata() {
    let fs = TestFs("metadata-identity-test");
    let filesystem = Filesystem::new(Arc::new(fs));
    let mount = Mountpoint::new_root(&filesystem);
    let mut node = TestNode::new(42);
    node.filesystem = fs;
    node.metadata = Some(Metadata {
        inode: 77,
        device: 999,
        nlink: 1,
        mode: NodePermission::default(),
        node_type: NodeType::RegularFile,
        uid: 0,
        gid: 0,
        size: 0,
        block_size: 4096,
        blocks: 0,
        rdev: axfs_ng_vfs::DeviceId::default(),
        atime: core::time::Duration::ZERO,
        mtime: core::time::Duration::ZERO,
        ctime: core::time::Duration::ZERO,
    });
    let node = Arc::new(node);
    let entry = DirEntry::new_file(
        FileNode::new(node.clone()),
        NodeType::RegularFile,
        Reference::root(),
    );
    let file = File::new(
        ax_fs_ng::File::new(
            FileBackend::Direct(Location::new(mount.clone(), entry)),
            FileFlags::READ,
        ),
        0,
    );

    assert_eq!(file.inode_key(), Some(InodeKey { filesystem: mount.filesystem_id(), inode: 42 }));
    assert_eq!(node.metadata_calls.load(Ordering::Relaxed), 0);
}

fn test_fs_context() -> FsContext {
    let filesystem = Filesystem::new(Arc::new(TestFs("context-test")));
    FsContext::new(Mountpoint::new_root(&filesystem).root_location())
}

#[derive(Clone, Copy)]
struct TestFs(&'static str);

impl FilesystemOps for TestFs {
    fn name(&self) -> &str {
        self.0
    }
    fn root_dir(&self) -> DirEntry {
        let mut node = TestNode::new(2);
        node.filesystem = *self;
        DirEntry::new_dir(|_| DirNode::new(Arc::new(node)), Reference::root())
    }
    fn stat(&self) -> VfsResult<StatFs> {
        Err(VfsError::Io)
    }
}

struct TestNode {
    inode: u64,
    metadata_calls: AtomicUsize,
    filesystem: TestFs,
    metadata: Option<Metadata>,
}

impl TestNode {
    fn new(inode: u64) -> Self {
        Self {
            inode,
            metadata_calls: AtomicUsize::new(0),
            filesystem: TestFs("ext4"),
            metadata: None,
        }
    }
}

impl NodeOps for TestNode {
    fn inode(&self) -> u64 {
        self.inode
    }
    fn metadata(&self) -> VfsResult<Metadata> {
        self.metadata_calls.fetch_add(1, Ordering::Relaxed);
        self.metadata.clone().ok_or(VfsError::Io)
    }
    fn update_metadata(&self, _: MetadataUpdate) -> VfsResult<()> {
        Err(VfsError::Io)
    }
    fn filesystem(&self) -> &dyn FilesystemOps {
        &self.filesystem
    }
    fn sync(&self, _: bool) -> VfsResult<()> {
        Err(VfsError::Io)
    }
    fn into_any(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {
        self
    }
}

impl Pollable for TestNode {
    fn poll(&self) -> IoEvents {
        IoEvents::IN
    }
    unsafe fn register_shared(&self, _: &mut dyn SharedRegistrationSink, _: IoEvents) {}
}

impl FileNodeOps for TestNode {
    fn read_at(&self, _: &mut [u8], _: u64) -> VfsResult<usize> {
        Err(VfsError::Io)
    }
    fn write_at(&self, _: &[u8], _: u64) -> VfsResult<usize> {
        Err(VfsError::Io)
    }
    fn append(&self, _: &[u8]) -> VfsResult<(usize, u64)> {
        Err(VfsError::Io)
    }
    fn set_len(&self, _: u64) -> VfsResult<()> {
        Err(VfsError::Io)
    }
}

impl DirNodeOps for TestNode {
    fn read_dir(&self, _: DirectoryCursor, _: &mut dyn DirEntrySink) -> VfsResult<usize> {
        Err(VfsError::Io)
    }
    fn lookup(&self, _: &str) -> VfsResult<DirEntry> {
        Err(VfsError::NotFound)
    }
    fn create(
        &self,
        _: &str,
        _: NodeType,
        _: NodePermission,
        _: u32,
        _: u32,
    ) -> VfsResult<DirEntry> {
        Err(VfsError::Io)
    }
    fn link(&self, _: &str, _: &DirEntry) -> VfsResult<DirEntry> {
        Err(VfsError::Io)
    }
    fn unlink(&self, _: &str, _: bool) -> VfsResult<()> {
        Err(VfsError::Io)
    }
    fn create_symlink(
        &self,
        _: &str,
        _: &str,
        _: NodePermission,
        _: u32,
        _: u32,
    ) -> VfsResult<DirEntry> {
        Err(VfsError::Io)
    }
    fn rename(&self, _: &str, _: &DirNode, _: &str, _: RenameOptions) -> VfsResult<()> {
        Err(VfsError::Io)
    }
}
