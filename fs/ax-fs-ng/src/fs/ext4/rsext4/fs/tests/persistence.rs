//! Migrated metadata persistence checks use independent device snapshots.

use core::time::Duration;

use axfs_ng_vfs::{Location, MetadataUpdate, Mountpoint, NodePermission, NodeType};

use super::*;
use crate::{
    file::{File, FileBackend, FileFlags},
    os::memory::test_support::with_test_page_provider,
};

#[test]
fn buffered_growth_persists_data_and_size_after_sync() {
    with_test_page_provider(true, |_| {
        let (storage, flushes) = formatted_test_storage();
        let (filesystem, root) = mount_background(storage.clone(), flushes);
        let entry = root
            .create(
                "buffered-output",
                NodeType::RegularFile,
                NodePermission::default(),
                0,
                0,
            )
            .unwrap();
        let file = File::new(
            FileBackend::new_cached(entry.clone()).unwrap(),
            FileFlags::WRITE,
        );

        assert_eq!(file.write_at(&b"compiler output"[..], 0), Ok(15));
        assert_eq!(entry.metadata().unwrap().size, 15);
        file.sync(false).unwrap();
        filesystem.sync_to_disk().unwrap();

        let snapshot = Arc::new(StdMutex::new(storage.lock().unwrap().clone()));
        let (_recovered, recovered_root) =
            mount_background(snapshot, Arc::new(AtomicUsize::new(0)));
        let persisted = recovered_root.lookup_no_follow("buffered-output").unwrap();
        let reader = File::new(FileBackend::new_direct(persisted.clone()), FileFlags::READ);
        let mut bytes = [0; 15];

        assert_eq!(persisted.metadata().unwrap().size, 15);
        assert_eq!(reader.read_at(&mut bytes[..], 0), Ok(bytes.len()));
        assert_eq!(&bytes, b"compiler output");
    });
}

#[test]
fn metadata_owner_and_times_remain_durable_after_explicit_sync() {
    let (storage, flushes) = formatted_test_storage();
    let (filesystem, root) = mount_background(storage.clone(), flushes.clone());
    let file = root
        .create(
            "input",
            NodeType::RegularFile,
            NodePermission::default(),
            0,
            0,
        )
        .unwrap();
    filesystem.sync_to_disk().unwrap();
    let before = flushes.load(Ordering::Relaxed);
    let update = MetadataUpdate {
        mode: Some(NodePermission::from_bits_truncate(0o640)),
        owner: Some((123_456, 234_567)),
        atime: Some(Duration::from_secs(123)),
        mtime: Some(Duration::from_secs(456)),
        ..Default::default()
    };

    file.update_metadata(update.clone()).unwrap();

    assert_metadata(&file, &update);
    assert_eq!(flushes.load(Ordering::Relaxed), before);
    file.sync(false).unwrap();
    assert!(flushes.load(Ordering::Relaxed) > before);
    // Mount a byte-for-byte disk snapshot with a fresh device/cache identity;
    // neither live VFS entries nor the original mount's caches can satisfy it.
    let snapshot = Arc::new(StdMutex::new(storage.lock().unwrap().clone()));
    let (_recovered, recovered_root) = mount_background(snapshot, Arc::new(AtomicUsize::new(0)));
    let persisted = recovered_root.lookup_no_follow("input").unwrap();
    assert_metadata(&persisted, &update);
}

#[test]
fn read_close_changes_atime_without_forcing_a_filesystem_commit() {
    let (storage, flushes) = formatted_test_storage();
    let (filesystem, root) = mount_background(storage, flushes.clone());
    let entry = root
        .create(
            "input",
            NodeType::RegularFile,
            NodePermission::default(),
            0,
            0,
        )
        .unwrap();
    entry
        .entry()
        .as_file()
        .unwrap()
        .write_at(b"hello", 0)
        .unwrap();
    entry
        .update_metadata(MetadataUpdate {
            atime: Some(Duration::from_secs(u32::MAX as u64)),
            ..Default::default()
        })
        .unwrap();
    filesystem.sync_to_disk().unwrap();
    let before = flushes.load(Ordering::Relaxed);
    let file = File::new(FileBackend::new_direct(entry.clone()), FileFlags::READ);
    let mut bytes = [0; 5];

    assert_eq!(file.read(&mut bytes[..]).unwrap(), bytes.len());
    drop(file);

    assert_eq!(&bytes, b"hello");
    assert_ne!(entry.metadata().unwrap().atime.as_secs(), u32::MAX as u64);
    assert_eq!(flushes.load(Ordering::Relaxed), before);
    entry.sync(true).unwrap();
    assert!(flushes.load(Ordering::Relaxed) > before);
}

fn assert_metadata(file: &Location, update: &MetadataUpdate) {
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.mode.bits(), update.mode.unwrap().bits());
    assert_eq!((metadata.uid, metadata.gid), update.owner.unwrap());
    assert_eq!(metadata.atime, update.atime.unwrap());
    assert_eq!(metadata.mtime, update.mtime.unwrap());
}

fn mount_background(
    storage: Arc<StdMutex<Vec<u8>>>,
    flushes: Arc<AtomicUsize>,
) -> (Arc<Ext4Filesystem>, Location) {
    let (mut filesystem, _) = mount_test_storage(storage, flushes, false);
    filesystem
        .lock()
        .ext4
        .enable_background_writeback()
        .unwrap();
    filesystem.writeback = Writeback::new(true);
    let filesystem = Arc::new_cyclic(|weak| {
        filesystem.self_ref = weak.clone();
        filesystem
    });
    let root = Mountpoint::new_root(&Filesystem::new(filesystem.clone())).root_location();
    (filesystem, root)
}
