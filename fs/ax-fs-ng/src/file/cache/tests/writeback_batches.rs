use super::*;

#[test]
fn large_contiguous_writeback_is_bounded_and_clipped_to_eof() {
    with_test_page_provider(true, |_| {
        let len = PAGE_SIZE * 515 + 137;
        let (cached, backing, contents) = dirty_contents(len);
        let written = cached.writeback().unwrap();
        assert_eq!(written, (0..516).collect::<Vec<_>>());
        let state = backing.state.lock().unwrap();
        assert_eq!(state.physical_data, contents);
        assert_eq!(state.logical_len, len);
        assert_eq!(
            state.write_ranges,
            [
                (0, PAGE_SIZE * 256),
                (PAGE_SIZE * 256, PAGE_SIZE * 256),
                (PAGE_SIZE * 512, PAGE_SIZE * 3 + 137),
            ]
        );
        drop(state);
        assert!(cached.dirty_pages_in_range(0, 516).is_empty());
        assert_no_writeback_reservations(&cached);
    });
}

#[test]
fn selected_disjoint_writeback_never_persists_unselected_pages() {
    with_test_page_provider(true, |_| {
        let (cached, backing, contents) = dirty_contents(PAGE_SIZE * 6 + 37);
        cached.writeback_pages(&[6, 0, 4, 3, 0, u32::MAX]).unwrap();
        let state = backing.state.lock().unwrap();
        assert_eq!(
            state.write_ranges,
            [
                (0, PAGE_SIZE),
                (3 * PAGE_SIZE, 2 * PAGE_SIZE),
                (6 * PAGE_SIZE, 37)
            ]
        );
        for number in 0..7 {
            let start = number * PAGE_SIZE;
            let end = (start + PAGE_SIZE).min(contents.len());
            if [0, 3, 4, 6].contains(&number) {
                assert_eq!(state.physical_data[start..end], contents[start..end]);
            } else {
                assert!(
                    state.physical_data[start..end]
                        .iter()
                        .all(|byte| *byte == 0)
                );
            }
        }
        drop(state);
        let mut dirty = cached.dirty_pages_in_range(0, 7);
        dirty.sort_unstable();
        assert_eq!(dirty, [1, 2, 5]);
        assert_no_writeback_reservations(&cached);
        cached.sync(false).unwrap();
        assert_eq!(backing.state.lock().unwrap().physical_data, contents);
    });
}

#[test]
fn second_batch_failure_cleans_only_the_completed_prefix_and_retry_preserves_bytes() {
    with_test_page_provider(true, |_| {
        let (cached, backing, contents) = dirty_contents(PAGE_SIZE * 258);
        let next_write = backing.clone();
        let reader = cached.clone();
        *backing.before_write.lock().unwrap() = Some(Box::new(move || {
            *next_write.before_write.lock().unwrap() = Some(Box::new(move || {
                assert!(reader.shared.io_lock_is_free_for_test());
                assert!(reader.dirty_pages_in_range(0, 256).is_empty());
                Err(VfsError::Io)
            }));
            Ok(())
        }));
        assert_eq!(cached.sync(false), Err(VfsError::Io));
        let mut dirty = cached.dirty_pages_in_range(0, 258);
        dirty.sort_unstable();
        assert_eq!(dirty, [256, 257]);
        assert_no_writeback_reservations(&cached);
        assert_eq!(backing.state.lock().unwrap().write_calls, 1);
        cached.sync(false).unwrap();
        let state = backing.state.lock().unwrap();
        assert_eq!(state.physical_data, contents);
        assert_eq!(state.write_calls, 2);
        drop(state);
        assert!(cached.dirty_pages_in_range(0, 258).is_empty());
    });
}

#[cfg(feature = "vfs")]
#[test]
fn reclaim_skips_completed_pages_until_the_writeback_round_releases_them() {
    with_test_page_provider(true, |_| {
        let (cached, backing, _) = dirty_contents(PAGE_SIZE * 257);
        let next_write = backing.clone();
        let reader = cached.clone();
        let observed = Arc::new(AtomicBool::new(false));
        let observation = observed.clone();
        *backing.before_write.lock().unwrap() = Some(Box::new(move || {
            *next_write.before_write.lock().unwrap() = Some(Box::new(move || {
                assert!(reader.dirty_pages_in_range(0, 256).is_empty());
                assert_eq!(reader.shared.try_evict_clean_pages(256), 0);
                assert_eq!(reader.shared.page_cache.lock().len(), 257);
                observation.store(true, Ordering::Release);
                Ok(())
            }));
            Ok(())
        }));
        cached.sync(false).unwrap();
        assert!(observed.load(Ordering::Acquire));
        assert_no_writeback_reservations(&cached);
        assert_eq!(cached.shared.try_evict_clean_pages(256), 256);
    });
}

#[test]
fn protection_redirty_cannot_be_cleaned_even_when_it_precedes_the_byte_snapshot() {
    with_test_page_provider(true, |_| {
        let (cached, backing, _) = dirty_contents(PAGE_SIZE);
        let writer = cached.clone();
        let listener = cached.add_page_listener(
            |_, _| true,
            move |number| {
                assert!(writer.shared.io_lock_is_free_for_test());
                assert!(writer.shared.page_cache_lock_is_free_for_test());
                assert!(writer.shared.listener_lock_is_free_for_test());
                writer.mark_mmap_dirty_page(number).unwrap();
                true
            },
        );
        cached.sync(false).unwrap();
        assert_eq!(backing.state.lock().unwrap().write_calls, 1);
        assert_eq!(cached.dirty_pages_in_range(0, 1), [0]);
        assert_no_writeback_reservations(&cached);
        // SAFETY: this cache issued this live listener, removed exactly once.
        unsafe { cached.remove_evict_listener(listener) };
        cached.sync(false).unwrap();
        assert!(cached.dirty_pages_in_range(0, 1).is_empty());
    });
}

#[test]
fn mmap_redirty_after_snapshot_keeps_the_later_contents_dirty() {
    with_test_page_provider(true, |_| {
        let (cached, backing, original) = dirty_contents(PAGE_SIZE);
        let writer = cached.clone();
        *backing.before_write.lock().unwrap() = Some(Box::new(move || {
            // Exercise the real writable-fault dirty boundary before emulating
            // its later store into the page. There is no buffered write here.
            writer.mark_mmap_dirty_page(0)?;
            writer.with_page_or_insert(0, |page, _| {
                page.data().fill(0x72);
                Ok(())
            })
        }));
        cached.sync(false).unwrap();
        assert_eq!(backing.state.lock().unwrap().physical_data, original);
        assert_eq!(cached.dirty_pages_in_range(0, 1), [0]);
        assert_no_writeback_reservations(&cached);
        cached.sync(false).unwrap();
        assert_eq!(
            backing.state.lock().unwrap().physical_data,
            vec![0x72; PAGE_SIZE]
        );
        assert!(cached.dirty_pages_in_range(0, 1).is_empty());
    });
}

#[test]
fn protection_failure_releases_all_page_reservations_without_backing_io() {
    with_test_page_provider(true, |_| {
        let (cached, backing, contents) = dirty_contents(PAGE_SIZE * 3);
        let listener = cached.add_page_listener(|_, _| true, |number| number != 1);
        assert_eq!(cached.writeback(), Err(VfsError::ResourceBusy));
        assert_eq!(backing.state.lock().unwrap().write_calls, 0);
        assert_eq!(cached.dirty_pages_in_range(0, 3).len(), 3);
        assert_no_writeback_reservations(&cached);
        // SAFETY: this cache issued this live listener, removed exactly once.
        unsafe { cached.remove_evict_listener(listener) };
        cached.sync(false).unwrap();
        assert_eq!(backing.state.lock().unwrap().physical_data, contents);
    });
}

fn dirty_contents(len: usize) -> (CachedFile, Arc<CacheTestFile>, Vec<u8>) {
    let contents = (0..len)
        .map(|index| (index % 251 + 1) as u8)
        .collect::<Vec<_>>();
    let backing = Arc::new(CacheTestFile::new(vec![0; len]));
    let cached = reopen_cached_file(backing.clone());
    assert_eq!(cached.write_at(contents.as_slice(), 0).unwrap(), len);
    (cached, backing, contents)
}

fn assert_no_writeback_reservations(cached: &CachedFile) {
    assert!(cached.shared.io_lock_is_free_for_test());
    assert!(cached.shared.writeback_lock.try_lock().is_some());
    assert!(
        cached
            .shared
            .page_cache
            .lock()
            .iter()
            .all(|(_, page)| !page.writeback_in_progress())
    );
}
