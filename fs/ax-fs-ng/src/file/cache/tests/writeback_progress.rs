use super::*;

#[derive(Clone, Copy)]
enum WritebackEntry {
    All,
    Selected,
    Sync,
    Global,
}

#[test]
fn every_writeback_entry_releases_io_before_real_backing_write() {
    with_test_page_provider(true, |_| {
        for entry in entries() {
            let (cached, backing) = dirty_file();
            let reader = cached.clone();
            let observed = Arc::new(AtomicBool::new(false));
            let observation = observed.clone();
            *backing.before_write.lock().unwrap() = Some(Box::new(move || {
                assert_read_progress(&reader);
                observation.store(true, Ordering::Release);
                Ok(())
            }));
            run_entry(&cached, entry).unwrap();
            assert!(observed.load(Ordering::Acquire));
            assert_eq!(backing.state.lock().unwrap().physical_data[0], 0x41);
            assert!(cached.dirty_pages_in_range(0, 1).is_empty());
            assert!(cached.shared.writeback_lock.try_lock().is_some());
        }
    });
}

#[test]
fn explicit_sync_releases_io_even_when_no_dirty_pages_remain() {
    with_test_page_provider(true, |_| {
        for entry in [
            WritebackEntry::All,
            WritebackEntry::Selected,
            WritebackEntry::Sync,
        ] {
            let (cached, backing) = dirty_file();
            cached.sync(false).unwrap();
            let reader = cached.clone();
            let observed = Arc::new(AtomicBool::new(false));
            let observation = observed.clone();
            *backing.before_sync.lock().unwrap() = Some(Box::new(move || {
                assert_read_progress(&reader);
                observation.store(true, Ordering::Release);
                Ok(())
            }));
            run_entry(&cached, entry).unwrap();
            assert!(observed.load(Ordering::Acquire));
        }
    });
}

#[test]
fn buffered_redirty_during_writeback_retains_new_bytes_for_retry() {
    with_test_page_provider(true, |_| {
        let (cached, backing) = dirty_file();
        let writer = cached.clone();
        *backing.before_write.lock().unwrap() = Some(Box::new(move || {
            assert_read_progress(&writer);
            writer.write_at(&[0x72][..], 0)?;
            Ok(())
        }));
        cached.sync(false).unwrap();
        assert_eq!(backing.state.lock().unwrap().physical_data[0], 0x41);
        assert_eq!(cached.dirty_pages_in_range(0, 1), [0]);
        let mut byte = [0];
        cached.read_at(&mut byte[..], 0).unwrap();
        assert_eq!(byte, [0x72]);
        cached.sync(false).unwrap();
        assert_eq!(backing.state.lock().unwrap().physical_data[0], 0x72);
        assert!(cached.dirty_pages_in_range(0, 1).is_empty());
    });
}

#[test]
fn failed_writeback_after_redirty_keeps_current_contents_and_releases_tracking() {
    with_test_page_provider(true, |_| {
        let (cached, backing) = dirty_file();
        let writer = cached.clone();
        *backing.before_write.lock().unwrap() = Some(Box::new(move || {
            assert_read_progress(&writer);
            writer.write_at(&[0x72][..], 0)?;
            Err(VfsError::Io)
        }));
        assert_eq!(cached.sync(false), Err(VfsError::Io));
        assert!(cached.shared.writeback_lock.try_lock().is_some());
        assert_eq!(cached.dirty_pages_in_range(0, 1), [0]);
        cached.sync(false).unwrap();
        assert_eq!(backing.state.lock().unwrap().physical_data[0], 0x72);
    });
}

#[test]
fn pending_writeback_cannot_evict_and_overwrite_a_newer_dirty_image() {
    with_test_page_provider(true, |_| {
        let (cached, backing) = eviction::one_page_cache();
        let writer = cached.clone();
        *backing.before_write.lock().unwrap() = Some(Box::new(move || {
            assert!(writer.shared.io_lock_is_free_for_test());
            let original = writer.pin_read_page(0)?.unwrap();
            writer.write_at(&[0x72][..], 0)?;
            writer.with_page_or_insert(1, |_, _| Ok(()))?;
            assert!(writer.shared.page_cache.lock().contains(&0));
            assert!(
                writer
                    .shared
                    .page_cache
                    .lock()
                    .get_mut(&0)
                    .unwrap()
                    .matches_pin(&original)
            );
            Ok(())
        }));
        cached.sync(false).unwrap();
        assert_eq!(backing.state.lock().unwrap().physical_data[0], 0xa5);
        assert_eq!(cached.dirty_pages_in_range(0, 1), [0]);
        cached.sync(false).unwrap();
        assert_eq!(backing.state.lock().unwrap().physical_data[0], 0x72);
    });
}

#[test]
fn failed_sync_preserves_error_and_allows_a_later_empty_sync_retry() {
    with_test_page_provider(true, |_| {
        let (cached, backing) = dirty_file();
        let reader = cached.clone();
        *backing.before_sync.lock().unwrap() = Some(Box::new(move || {
            assert_read_progress(&reader);
            Err(VfsError::Io)
        }));
        assert_eq!(cached.sync(false), Err(VfsError::Io));
        assert!(cached.shared.writeback_lock.try_lock().is_some());
        let observed = Arc::new(AtomicBool::new(false));
        let observation = observed.clone();
        *backing.before_sync.lock().unwrap() = Some(Box::new(move || {
            observation.store(true, Ordering::Release);
            Ok(())
        }));
        cached.sync(false).unwrap();
        assert!(observed.load(Ordering::Acquire));
    });
}

#[test]
fn resident_mapping_progress_does_not_require_the_cached_io_owner() {
    with_test_page_provider(true, |_| {
        let (cached, _) = dirty_file();
        let pin = cached.pin_read_page(0).unwrap().unwrap();
        let owner = cached.shared.io_lock.lock();
        assert_eq!(
            cached.with_current_read_page(0, &pin, || Ok(42)),
            Ok(Some(42))
        );
        let second = cached.pin_read_page(0).unwrap().unwrap();
        assert_eq!(pin.paddr(), second.paddr());
        drop(owner);
        cached.sync(false).unwrap();
    });
}

#[test]
fn tentative_resize_cannot_publish_a_prepared_cache_mapping() {
    with_test_page_provider(true, |_| {
        let (cached, backing) = dirty_file();
        let pin = cached.pin_read_page(0).unwrap().unwrap();
        let reader = cached.clone();
        *backing.before_set_len.lock().unwrap() = Some(Box::new(move || {
            assert_eq!(
                reader.with_current_read_page(0, &pin, || panic!("tentative mapping published")),
                Ok(None::<()>)
            );
        }));
        backing.fail_next_set_len();
        assert_eq!(cached.set_len(16), Err(VfsError::Io));
        let pin = cached.pin_read_page(0).unwrap().unwrap();
        assert_eq!(
            cached.with_current_read_page(0, &pin, || Ok(42)),
            Ok(Some(42))
        );
        cached.sync(false).unwrap();
    });
}

fn dirty_file() -> (CachedFile, Arc<CacheTestFile>) {
    let backing = Arc::new(CacheTestFile::new(vec![0; PAGE_SIZE]));
    let cached = reopen_cached_file(backing.clone());
    cached.write_at(&[0x41; PAGE_SIZE][..], 0).unwrap();
    (cached, backing)
}

fn assert_read_progress(cached: &CachedFile) {
    assert!(
        cached.shared.io_lock_is_free_for_test(),
        "writeback retained cached io_lock across backing I/O"
    );
    assert!(cached.shared.page_cache_lock_is_free_for_test());
    assert!(cached.shared.listener_lock_is_free_for_test());
    assert!(cached.shared.writeback_lock.try_lock().is_none());
    let pin = cached.pin_read_page(0).unwrap().unwrap();
    assert_eq!(
        cached.with_current_read_page(0, &pin, || Ok(42)),
        Ok(Some(42))
    );
}

fn entries() -> Vec<WritebackEntry> {
    let mut result = vec![
        WritebackEntry::All,
        WritebackEntry::Selected,
        WritebackEntry::Sync,
    ];
    if cfg!(any(feature = "vfs", feature = "ext4")) {
        result.push(WritebackEntry::Global);
    }
    result
}

fn run_entry(cached: &CachedFile, entry: WritebackEntry) -> VfsResult<()> {
    match entry {
        WritebackEntry::All => cached.writeback().map(|_| ()),
        WritebackEntry::Selected => cached.writeback_pages(&[0, 0, u32::MAX]),
        WritebackEntry::Sync => cached.sync(false),
        #[cfg(any(feature = "vfs", feature = "ext4"))]
        WritebackEntry::Global => cached.shared.writeback_dirty_for_global_sync(),
        #[cfg(not(any(feature = "vfs", feature = "ext4")))]
        WritebackEntry::Global => unreachable!("global writeback is unavailable in this build"),
    }
}
