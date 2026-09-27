//! Exercise metadata writeback through the production ext4/VFS adapter.

use alloc::{boxed::Box, format, sync::Arc};
use core::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use std::{
    fs::{File as ImageFile, OpenOptions},
    os::unix::fs::FileExt,
    path::PathBuf,
    process::Command,
};

use axfs_ng_vfs::{
    DirEntry, Filesystem, Location, MetadataUpdate, Mountpoint, NodePermission, NodeType, VfsError,
};

use super::Ext4Filesystem;
use crate::{
    BlockError, BlockResult,
    block::{BlockRegion, FsBlockDevice, read::FsBlockReader},
    file::{File, FileBackend, FileFlags},
};

#[test]
fn metadata_update_defers_writeback_until_explicit_sync() {
    let fixture = Fixture::new("metadata-writeback");
    let filesystem = fixture.mount();
    let entry = fixture.create_file(&filesystem);
    let update = MetadataUpdate {
        mode: Some(NodePermission::from_bits_truncate(0o640)),
        owner: Some((123_456, 234_567)),
        atime: Some(Duration::from_secs(123)),
        mtime: Some(Duration::from_secs(456)),
        ..Default::default()
    };
    let before = fixture.counts();

    entry.update_metadata(update.clone()).unwrap();

    let metadata = entry.metadata().unwrap();
    assert_eq!(metadata.mode.bits(), update.mode.unwrap().bits());
    assert_eq!((metadata.uid, metadata.gid), update.owner.unwrap());
    assert_eq!(metadata.atime, update.atime.unwrap());
    assert_eq!(metadata.mtime, update.mtime.unwrap());
    assert_eq!(
        fixture.counts(),
        before,
        "metadata update flushed the filesystem"
    );

    entry.sync(false).unwrap();
    assert!(fixture.counts().0 > before.0);
    assert!(fixture.counts().1 > before.1);
    let reopened = fixture.mount();
    let persisted = reopened
        .root_dir()
        .as_dir()
        .unwrap()
        .lookup("input")
        .unwrap();
    let metadata = persisted.metadata().unwrap();
    assert_eq!(metadata.mode.bits(), update.mode.unwrap().bits());
    assert_eq!((metadata.uid, metadata.gid), update.owner.unwrap());
    assert_eq!(metadata.atime, update.atime.unwrap());
    assert_eq!(metadata.mtime, update.mtime.unwrap());
    fixture.check_clean();
}

#[test]
fn closing_a_read_file_updates_atime_without_flushing_the_filesystem() {
    let fixture = Fixture::new("close-atime");
    let filesystem = fixture.mount();
    let entry = fixture.create_file(&filesystem);
    entry
        .update_metadata(MetadataUpdate {
            atime: Some(Duration::from_secs(u32::MAX as u64)),
            ..Default::default()
        })
        .unwrap();
    entry.sync(false).unwrap();
    let before = fixture.counts();
    let location = Location::new(Mountpoint::new_root(&filesystem), entry.clone());
    let file = File::new(FileBackend::new_direct(location), FileFlags::READ);
    let mut bytes = [0; 5];

    assert_eq!(file.read(&mut bytes[..]).unwrap(), bytes.len());
    assert_eq!(&bytes, b"hello");
    drop(file);

    assert_ne!(entry.metadata().unwrap().atime.as_secs(), u32::MAX as u64);
    assert_eq!(
        fixture.counts(),
        before,
        "read-close flushed the filesystem"
    );
    entry.sync(true).unwrap();
    assert!(fixture.counts().1 > before.1);
    fixture.check_clean();
}

#[test]
fn explicit_metadata_sync_propagates_device_flush_failure() {
    let fixture = Fixture::new("metadata-sync-error");
    let filesystem = fixture.mount();
    let entry = fixture.create_file(&filesystem);
    entry
        .update_metadata(MetadataUpdate {
            mtime: Some(Duration::from_secs(789)),
            ..Default::default()
        })
        .unwrap();
    fixture.io.fail_flush.store(true, Ordering::Relaxed);

    assert_eq!(entry.sync(false), Err(VfsError::Io));

    fixture.io.fail_flush.store(false, Ordering::Relaxed);
    entry.filesystem().flush().unwrap();
    let reopened = fixture.mount();
    let persisted = reopened
        .root_dir()
        .as_dir()
        .unwrap()
        .lookup("input")
        .unwrap();
    assert_eq!(persisted.metadata().unwrap().mtime.as_secs(), 789);
    fixture.check_clean();
}

#[test]
fn file_creation_keeps_published_directory_entry_consistent() {
    check_creation_consistency(NodeType::RegularFile);
}

#[test]
fn directory_creation_keeps_published_directory_entry_consistent() {
    check_creation_consistency(NodeType::Directory);
}

fn check_creation_consistency(node_type: NodeType) {
    let fixture = Fixture::new("create-writeback");
    let filesystem = fixture.mount();
    let root = filesystem.root_dir();
    root.sync(false).unwrap();
    let created = root
        .as_dir()
        .unwrap()
        .create(
            "created",
            node_type,
            NodePermission::from_bits_truncate(0o751),
            123,
            456,
        )
        .unwrap();
    let visible = root.as_dir().unwrap().lookup("created").unwrap();
    assert_eq!(visible.inode(), created.inode());
    let metadata = visible.metadata().unwrap();
    assert_eq!(metadata.node_type, node_type);
    assert_eq!(metadata.mode.bits(), 0o751);
    assert_eq!((metadata.uid, metadata.gid), (123, 456));
    // Directory insertion currently writes its data block outside the journal.
    // A published entry must not refer to an inode that is still free on disk.
    // Inspect the backing image without invoking any additional VFS sync.
    fixture.check_clean();

    root.sync(false).unwrap();
    let reopened = fixture.mount();
    let persisted = reopened
        .root_dir()
        .as_dir()
        .unwrap()
        .lookup("created")
        .unwrap();
    assert_eq!(persisted.inode(), created.inode());
    let metadata = persisted.metadata().unwrap();
    assert_eq!(metadata.node_type, node_type);
    assert_eq!(metadata.mode.bits(), 0o751);
    assert_eq!((metadata.uid, metadata.gid), (123, 456));
    fixture.check_clean();
}

#[derive(Default)]
struct DeviceIo {
    // Observation counters and fault injection, not publication of shared data.
    writes: AtomicUsize,
    flushes: AtomicUsize,
    fail_flush: AtomicBool,
    fail_read_sector: std::sync::Mutex<Option<u64>>,
    disable_shared_reads: AtomicBool,
}

pub(super) struct Fixture {
    image: PathBuf,
    io: Arc<DeviceIo>,
}

impl Fixture {
    pub(super) fn new(name: &str) -> Self {
        static NEXT_IMAGE: AtomicUsize = AtomicUsize::new(0);
        let sequence = NEXT_IMAGE.fetch_add(1, Ordering::Relaxed);
        let directory =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tmp/ext4-metadata-tests");
        std::fs::create_dir_all(&directory).unwrap();
        let image = directory.join(format!("{name}-{}-{sequence}.img", std::process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&image)
            .unwrap();
        file.set_len(32 * 1024 * 1024).unwrap();
        drop(file);
        let fixture = Self {
            image,
            io: Arc::new(DeviceIo::default()),
        };
        fixture.run_image_tool("mkfs.ext4", &["-F", "-b", "4096"]);
        fixture
    }

    pub(super) fn mount(&self) -> Filesystem {
        let (device, region) = self.open_device();
        Ext4Filesystem::new(device, region).unwrap()
    }

    pub(super) fn open_device(&self) -> (Box<dyn FsBlockDevice>, BlockRegion) {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.image)
            .unwrap();
        let blocks = file.metadata().unwrap().len() / 512;
        let device = ImageDevice {
            file: Arc::new(file),
            blocks,
            io: self.io.clone(),
        };
        (Box::new(device), BlockRegion::from_num_blocks(blocks))
    }

    pub(super) fn fail_read_at(&self, sector: Option<u64>) {
        *self.io.fail_read_sector.lock().unwrap() = sector;
    }

    #[cfg(feature = "profile")]
    pub(super) fn disable_shared_reader(&self) {
        self.io.disable_shared_reads.store(true, Ordering::Relaxed);
    }

    #[cfg(feature = "profile")]
    pub(super) fn fail_flush(&self) {
        self.io.fail_flush.store(true, Ordering::Relaxed);
    }

    pub(super) fn create_file(&self, filesystem: &Filesystem) -> DirEntry {
        let entry = filesystem
            .root_dir()
            .as_dir()
            .unwrap()
            .create(
                "input",
                NodeType::RegularFile,
                NodePermission::from_bits_truncate(0o600),
                0,
                0,
            )
            .unwrap();
        assert_eq!(entry.as_file().unwrap().write_at(b"hello", 0).unwrap(), 5);
        entry.sync(false).unwrap();
        entry
    }

    fn counts(&self) -> (usize, usize) {
        (
            self.io.writes.load(Ordering::Relaxed),
            self.io.flushes.load(Ordering::Relaxed),
        )
    }

    pub(super) fn check_clean(&self) {
        self.run_image_tool("e2fsck", &["-fn"]);
    }

    fn run_image_tool(&self, tool: &str, args: &[&str]) {
        let output = Command::new(tool)
            .args(args)
            .arg(&self.image)
            .output()
            .unwrap_or_else(|error| panic!("cannot execute {tool}: {error}"));
        assert!(
            output.status.success(),
            "{tool} failed: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            std::string::String::from_utf8_lossy(&output.stdout),
            std::string::String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[derive(Clone)]
struct ImageDevice {
    file: Arc<ImageFile>,
    blocks: u64,
    io: Arc<DeviceIo>,
}

impl FsBlockDevice for ImageDevice {
    fn name(&self) -> &str {
        "ext4-metadata-test"
    }
    fn num_blocks(&self) -> u64 {
        self.blocks
    }
    fn block_size(&self) -> usize {
        512
    }

    fn read_block(&mut self, block_id: u64, buf: &mut [u8]) -> BlockResult {
        FsBlockReader::read_block(self, block_id, buf)
    }

    fn shared_reader(&self) -> Option<Arc<dyn FsBlockReader>> {
        if self.io.disable_shared_reads.load(Ordering::Relaxed) {
            None
        } else {
            Some(Arc::new(self.clone()))
        }
    }

    fn write_block(&mut self, block_id: u64, buf: &[u8]) -> BlockResult {
        self.io.writes.fetch_add(1, Ordering::Relaxed);
        self.file
            .write_all_at(buf, block_id * 512)
            .map_err(|_| BlockError::Io)
    }

    fn flush(&mut self) -> BlockResult {
        self.io.flushes.fetch_add(1, Ordering::Relaxed);
        if self.io.fail_flush.load(Ordering::Relaxed) {
            return Err(BlockError::Io);
        }
        self.file.sync_all().map_err(|_| BlockError::Io)
    }
}

impl FsBlockReader for ImageDevice {
    fn read_block(&self, block_id: u64, buf: &mut [u8]) -> BlockResult {
        let end = block_id + (buf.len() / 512) as u64;
        if self
            .io
            .fail_read_sector
            .lock()
            .unwrap()
            .is_some_and(|sector| (block_id..end).contains(&sector))
        {
            return Err(BlockError::Io);
        }
        self.file
            .read_exact_at(buf, block_id * 512)
            .map_err(|_| BlockError::Io)
    }
}
