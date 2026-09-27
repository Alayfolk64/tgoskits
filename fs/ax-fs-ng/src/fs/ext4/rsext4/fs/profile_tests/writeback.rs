use super::*;

#[test]
fn inode_table_pre_read_releases_ext4_metadata_lock() {
    let fixture = Fixture::new("inode-writeback-unlocked-read");
    let original = fixture.mount();
    fixture.create_file(&original);
    let (result, observed) = capture(&fixture, |filesystem| {
        let inode = captured_inode(filesystem, "/input");
        inode.update_metadata(MetadataUpdate {
            owner: Some((123, 456)),
            ..Default::default()
        })?;
        evict_table_buffers(filesystem);
        clear_observations();
        filesystem.sync_to_disk()
    });
    assert_eq!(result, Ok(()));
    // Only inode metadata was dirtied, so the first real device read belongs
    // to its table read-modify-write. Later superblock / GDT reads stay locked.
    let first_read = observed
        .iter()
        .find(|item| item.event == ax_sync::ProfileEvent::BlockRead as u8)
        .expect("dirty cold inode table must issue a device read");
    assert!(
        !first_read.locked,
        "inode-table pre-read retained the global ext4 mutex: {first_read:?}"
    );
    fixture.check_clean();
}

#[test]
fn inode_pre_read_allows_another_file_read_to_finish() {
    let fixture = Fixture::new("inode-writeback-reader-progress");
    let original = fixture.mount();
    fixture.create_file(&original);
    create_named_file(&original, "other", b"world");
    let (result, _) = capture(&fixture, |filesystem| {
        let input = captured_inode(filesystem, "/input");
        let other = captured_inode(filesystem, "/other");
        change_owner(&input, 123);
        evict_table_buffers(filesystem);
        let fired = Rc::new(Cell::new(false));
        let in_read = fired.clone();
        let shared = CAPTURE.with_borrow(|slot| slot.as_ref().unwrap().filesystem.clone());
        CAPTURE.with_borrow_mut(|slot| {
            slot.as_mut().unwrap().after_read = Some(Rc::new(move || {
                if in_read.replace(true) {
                    return;
                }
                assert!(shared.inner.try_lock().is_some());
                let mut bytes = [0; 5];
                assert_eq!(other.read_at(&mut bytes, 0).unwrap(), 5);
                assert_eq!(&bytes, b"world");
            }));
        });
        filesystem.sync_to_disk()?;
        assert!(fired.get());
        assert_eq!(input.metadata()?.uid, 123);
        Ok(())
    });
    assert_eq!(result, Ok(()));
    fixture.check_clean();
}

#[test]
fn newer_inode_commit_is_not_overwritten_by_a_pre_read_snapshot() {
    check_interleaved_sync(None);
}

#[test]
fn saturated_write_sequence_cannot_accept_a_stale_pre_read() {
    check_interleaved_sync(Some(u64::MAX));
}

fn check_interleaved_sync(sequence: Option<u64>) {
    let fixture = Fixture::new("inode-writeback-stale-snapshot");
    let original = fixture.mount();
    fixture.create_file(&original);
    create_named_file(&original, "neighbor", b"world");
    let committed_newer = Rc::new(Cell::new(false));
    let (result, _) = capture(&fixture, |filesystem| {
        let input = captured_inode(filesystem, "/input");
        let neighbor = captured_inode(filesystem, "/neighbor");
        change_owner(&input, 123);
        evict_table_buffers(filesystem);
        if let Some(sequence) = sequence {
            filesystem
                .write_sequence
                .store(sequence, core::sync::atomic::Ordering::Relaxed);
        }
        let fired = Rc::new(Cell::new(false));
        let in_read = fired.clone();
        let changed = input.clone();
        let shared = CAPTURE.with_borrow(|slot| slot.as_ref().unwrap().filesystem.clone());
        let committed = committed_newer.clone();
        CAPTURE.with_borrow_mut(|slot| {
            slot.as_mut().unwrap().after_read = Some(Rc::new(move || {
                if in_read.replace(true) {
                    return;
                }
                // The saturated case should not start an unlocked pre-read.
                // Run the interleaving only when the implementation releases
                // the lock; otherwise finish the original serialized sync.
                if shared.inner.try_lock().is_none() {
                    assert!(sequence.is_some());
                    return;
                }
                change_owner(&changed, 321);
                change_owner(&neighbor, 654);
                shared.sync_to_disk().unwrap();
                committed.set(true);
            }));
        });
        filesystem.sync_to_disk()?;
        assert!(fired.get());
        if sequence.is_none() {
            assert_eq!(input.metadata()?.uid, 321);
        }
        Ok(())
    });
    assert_eq!(result, Ok(()));
    let reopened = fixture.mount();
    let root = reopened.root_dir();
    let root = root.as_dir().unwrap();
    let input_uid = root.lookup("input").unwrap().metadata().unwrap().uid;
    let neighbor_uid = root.lookup("neighbor").unwrap().metadata().unwrap().uid;
    let expected = if committed_newer.get() {
        (321, 654)
    } else {
        (123, 0)
    };
    assert_eq!(
        (input_uid, neighbor_uid),
        expected,
        "pre-read overwrote a newer committed inode"
    );
    if sequence.is_none() {
        assert_eq!((input_uid, neighbor_uid), (321, 654));
    }
    fixture.check_clean();
}

#[test]
fn failed_table_pre_read_keeps_dirty_inode_for_retry() {
    let fixture = Fixture::new("inode-writeback-pre-read-error");
    let original = fixture.mount();
    fixture.create_file(&original);
    let (result, _) = capture(&fixture, |filesystem| {
        let input = captured_inode(filesystem, "/input");
        change_owner(&input, 123);
        let table = filesystem
            .lock()
            .fs
            .inodetable_cache
            .get(InodeNumber::new(input.inode() as u32).unwrap())
            .unwrap()
            .block_num;
        evict_table_buffers(filesystem);
        fixture.fail_read_at(Some(table.raw() * 8));
        assert_eq!(filesystem.sync_to_disk(), Err(VfsError::Io));
        assert!(filesystem.inner.try_lock().is_some());
        assert!(filesystem.lock().fs.inodetable_cache.stats().dirty_entries > 0);
        fixture.fail_read_at(None);
        filesystem.sync_to_disk()
    });
    assert_eq!(result, Ok(()));
    let reopened = fixture.mount();
    assert_eq!(
        reopened
            .root_dir()
            .as_dir()
            .unwrap()
            .lookup("input")
            .unwrap()
            .metadata()
            .unwrap()
            .uid,
        123
    );
    fixture.check_clean();
}

#[test]
fn table_writeback_without_shared_reader_keeps_serialized_io() {
    let fixture = Fixture::new("inode-writeback-reader-fallback");
    fixture.disable_shared_reader();
    let original = fixture.mount();
    fixture.create_file(&original);
    let (result, observed) = capture(&fixture, |filesystem| {
        let inode = captured_inode(filesystem, "/input");
        change_owner(&inode, 123);
        evict_table_buffers(filesystem);
        clear_observations();
        filesystem.sync_to_disk()
    });
    assert_eq!(result, Ok(()));
    let first = observed
        .iter()
        .find(|item| item.event == ax_sync::ProfileEvent::BlockRead as u8)
        .unwrap();
    assert!(first.locked);
    fixture.check_clean();
}

fn change_owner(inode: &Inode, uid: u32) {
    inode
        .update_metadata(MetadataUpdate {
            owner: Some((uid, 456)),
            ..Default::default()
        })
        .unwrap();
}

fn evict_table_buffers(filesystem: &Ext4Filesystem) {
    let mut state = filesystem.lock();
    // Read four distinct, valid fixture blocks without modifying disk. This
    // evicts the small device table cache, not the authoritative inode cache.
    for block in 6000..6004 {
        state
            .dev
            .read_block(rsext4::bmalloc::AbsoluteBN::new(block))
            .unwrap();
    }
}
