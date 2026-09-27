use alloc::{rc::Rc, vec, vec::Vec};
use core::cell::{Cell, RefCell};

use axfs_ng_vfs::{
    DirNodeOps, FileNodeOps, MetadataUpdate, NodeOps, NodePermission, NodeType, VfsError,
};

use super::*;
use crate::fs::ext4::rsext4::metadata_tests::Fixture;

mod writeback;

// These are the stable wire IDs; the tests compile before the new events exist.
const EXT4_HOLD: u8 = 8;
const BLOCK_FLUSH: u8 = 9;

#[test]
fn cached_metadata_query_does_not_acquire_ext4_mutex() {
    assert_cached_inode_query_avoids_ext4_lock(|inode| {
        let metadata = inode.metadata()?;
        assert_eq!(metadata.size, 5);
        assert_eq!(metadata.mode.bits(), 0o600);
        Ok(())
    });
}

#[test]
fn cached_length_query_does_not_acquire_ext4_mutex() {
    assert_cached_inode_query_avoids_ext4_lock(|inode| {
        assert_eq!(inode.len()?, 5);
        Ok(())
    });
}

#[test]
fn cached_inode_query_completes_while_global_filesystem_lock_is_held() {
    let fixture = Fixture::new("cached-inode-held-lock");
    let original = fixture.mount();
    fixture.create_file(&original);
    let (result, _) = capture(&fixture, |filesystem| {
        let input = captured_inode(filesystem, "/input");
        let guard = filesystem.lock();
        // The hook rejects a recursive fs acquisition before it can block.
        // This makes an accidental regression fail instead of hanging the test.
        CAPTURE.with_borrow_mut(|slot| slot.as_mut().unwrap().reject_fs_lock = true);
        assert_eq!(input.metadata()?.size, 5);
        assert_eq!(input.len()?, 5);
        CAPTURE.with_borrow_mut(|slot| slot.as_mut().unwrap().reject_fs_lock = false);
        drop(guard);
        Ok(())
    });
    assert_eq!(result, Ok(()));
}

#[test]
fn cached_inode_attributes_follow_hardlink_updates_and_unlink() {
    let fixture = Fixture::new("cached-inode-hardlink");
    let original = fixture.mount();
    let entry = fixture.create_file(&original);
    original
        .root_dir()
        .as_dir()
        .unwrap()
        .link("alias", &entry)
        .unwrap();
    entry.sync(false).unwrap();
    let (result, _) = capture(&fixture, |filesystem| {
        let input = captured_inode(filesystem, "/input");
        let alias = captured_inode(filesystem, "/alias");
        let root = captured_inode(filesystem, "/");
        alias.update_metadata(MetadataUpdate {
            mode: Some(NodePermission::from_bits_truncate(0o640)),
            owner: Some((123_456, 234_567)),
            ..Default::default()
        })?;
        alias.set_len(2)?;
        let metadata = input.metadata()?;
        assert_eq!(metadata.mode.bits(), 0o640);
        assert_eq!((metadata.uid, metadata.gid), (123_456, 234_567));
        assert_eq!(metadata.size, 2);
        assert_eq!(metadata.nlink, 2);
        root.unlink("input", false)?;
        assert_eq!(input.metadata()?.nlink, 1);
        root.unlink("alias", false)?;
        assert_eq!(input.metadata()?.nlink, 0);
        assert_eq!(input.len()?, 2);
        let mut bytes = [0; 2];
        assert_eq!(input.read_at(&mut bytes, 0)?, 2);
        assert_eq!(&bytes, b"he");
        Ok(())
    });
    assert_eq!(result, Ok(()));
}

#[test]
fn missing_cached_inode_uses_locked_reload_before_subsequent_hit() {
    let fixture = Fixture::new("cached-inode-miss");
    let original = fixture.mount();
    fixture.create_file(&original);
    let (result, _) = capture(&fixture, |filesystem| {
        let input = captured_inode(filesystem, "/input");
        filesystem.lock().fs.inodetable_cache.clear();
        clear_observations();
        assert_eq!(input.metadata()?.size, 5);
        CAPTURE.with_borrow(|slot| {
            assert!(
                slot.as_ref()
                    .unwrap()
                    .observed
                    .iter()
                    .any(|event| event.event == EXT4_HOLD)
            );
        });
        clear_observations();
        assert_eq!(input.len()?, 5);
        CAPTURE.with_borrow(|slot| assert!(slot.as_ref().unwrap().observed.is_empty()));
        Ok(())
    });
    assert_eq!(result, Ok(()));
}

fn assert_cached_inode_query_avoids_ext4_lock(query: fn(&Inode) -> VfsResult<()>) {
    let fixture = Fixture::new("cached-inode-query");
    let original = fixture.mount();
    fixture.create_file(&original);
    let (result, _) = capture(&fixture, |filesystem| {
        let inode = captured_inode(filesystem, "/input");
        clear_observations();
        query(&inode)?;
        CAPTURE.with_borrow(|slot| {
            let observed = &slot.as_ref().unwrap().observed;
            assert!(
                observed.is_empty(),
                "cached inode query entered filesystem lock or I/O: {observed:?}"
            );
        });
        Ok(())
    });
    assert_eq!(result, Ok(()));
}

#[test]
fn file_data_read_releases_ext4_metadata_lock() {
    let fixture = Fixture::new("read-outside-metadata-lock");
    let original = fixture.mount();
    fixture.create_file(&original);

    let (result, observed) = capture(&fixture, |filesystem| {
        let registration = {
            let mut state = filesystem.lock();
            let (fs, dev) = state.split();
            let (number, _) = rsext4::dir::get_inode_with_num(fs, dev, "/input")
                .unwrap()
                .unwrap();
            state.inc_ref(number)
        };
        let shared = CAPTURE.with_borrow(|slot| slot.as_ref().unwrap().filesystem.clone());
        let inode = Inode::new(shared, registration, None, Some("/input".into()));
        CAPTURE.with_borrow_mut(|slot| slot.as_mut().unwrap().observed.clear());

        let mut bytes = [0; rsext4::BLOCK_SIZE];
        assert_eq!(inode.read_at(&mut bytes, 0)?, 5);
        assert_eq!(&bytes[..5], b"hello");
        Ok(())
    });

    assert_eq!(result, Ok(()));
    let reads: Vec<_> = observed
        .iter()
        .filter(|item| item.event == ax_sync::ProfileEvent::BlockRead as u8)
        .collect();
    assert!(
        !reads.is_empty(),
        "fixture did not read file data from disk"
    );
    assert!(
        reads.iter().all(|item| !item.locked),
        "file data I/O retained the ext4 metadata mutex: {reads:?}"
    );
}

#[test]
fn another_inode_read_finishes_inside_a_file_read_window() {
    let fixture = Fixture::new("interleaved-file-reads");
    let original = fixture.mount();
    fixture.create_file(&original);
    create_named_file(&original, "other", b"world");
    let (result, observed) = capture(&fixture, |filesystem| {
        let input = captured_inode(filesystem, "/input");
        let other = captured_inode(filesystem, "/other");
        watch_inode(filesystem, &input);
        let fired = Rc::new(Cell::new(false));
        let fired_in_read = fired.clone();
        let shared = CAPTURE.with_borrow(|slot| slot.as_ref().unwrap().filesystem.clone());
        CAPTURE.with_borrow_mut(|slot| {
            slot.as_mut().unwrap().before_read = Some(Rc::new(move || {
                if fired_in_read.replace(true) {
                    return;
                }
                // Fail before a recursive read could deadlock on a retained
                // global mutex. The outer inode's I/O guard remains live.
                assert!(shared.inner.try_lock().is_some());
                let mut bytes = [0; 5];
                assert_eq!(other.read_at(&mut bytes, 0).unwrap(), 5);
                assert_eq!(&bytes, b"world");
            }));
        });
        clear_observations();
        let mut bytes = [0; 5];
        assert_eq!(input.read_at(&mut bytes, 0)?, 5);
        assert_eq!(&bytes, b"hello");
        assert!(fired.get());
        Ok(())
    });
    assert_eq!(result, Ok(()));
    let reads: Vec<_> = observed
        .iter()
        .filter(|item| item.event == ax_sync::ProfileEvent::BlockRead as u8)
        .collect();
    assert_eq!(reads.len(), 4, "both real file data reads must occur");
    assert!(
        reads
            .iter()
            .all(|item| !item.locked && item.inode_locked == Some(true))
    );
}

#[test]
fn file_mutations_hold_the_hardlink_shared_inode_gate() {
    let mutations: [fn(&Inode) -> VfsResult<()>; 3] = [
        |inode| inode.write_at(b"new", 0).map(|_| ()),
        |inode| inode.append(b"tail").map(|_| ()),
        |inode| inode.set_len(1),
    ];
    for mutate in mutations {
        let fixture = Fixture::new("mutation-inode-gate");
        let original = fixture.mount();
        let entry = fixture.create_file(&original);
        original
            .root_dir()
            .as_dir()
            .unwrap()
            .link("alias", &entry)
            .unwrap();
        entry.sync(false).unwrap();
        let (result, _) = capture(&fixture, |filesystem| {
            let input = captured_inode(filesystem, "/input");
            let alias = captured_inode(filesystem, "/alias");
            assert_eq!(input.inode(), alias.inode());
            watch_inode(filesystem, &input);
            clear_observations();
            mutate(&alias)?;
            CAPTURE.with_borrow(|slot| {
                let observed = &slot.as_ref().unwrap().observed;
                let holds: Vec<_> = observed
                    .iter()
                    .filter(|item| item.event == EXT4_HOLD && item.boundary == Boundary::Begin)
                    .collect();
                assert!(!holds.is_empty());
                assert!(
                    holds.iter().all(|item| item.inode_locked == Some(true)),
                    "mutation entered ext4 without the shared inode gate: {holds:?}"
                );
            });
            Ok(())
        });
        assert_eq!(result, Ok(()));
    }
}

#[test]
fn rename_holds_target_inode_gate_before_reclaiming_blocks() {
    let fixture = Fixture::new("rename-inode-gate");
    let original = fixture.mount();
    fixture.create_file(&original);
    create_named_file(&original, "source", b"replacement");
    let (result, _) = capture(&fixture, |filesystem| {
        let root = captured_inode(filesystem, "/");
        let target = captured_inode(filesystem, "/input");
        watch_inode(filesystem, &target);
        clear_observations();
        root.rename("source", &DirNode::new(root.clone()), "input")?;
        CAPTURE.with_borrow(|slot| {
            let holds: Vec<_> = slot
                .as_ref()
                .unwrap()
                .observed
                .iter()
                .filter(|item| item.event == EXT4_HOLD && item.boundary == Boundary::Begin)
                .map(|item| item.inode_locked)
                .collect();
            // Snapshot without waiting under fs, then protected revalidation /
            // mutation, then the existing whole-filesystem durability boundary.
            assert_eq!(holds, [Some(false), Some(true), Some(false)]);
        });
        Ok(())
    });
    assert_eq!(result, Ok(()));
}

#[test]
fn device_without_shared_reader_keeps_the_existing_read_path() {
    let fixture = Fixture::new("serialized-reader-fallback");
    fixture.disable_shared_reader();
    let original = fixture.mount();
    fixture.create_file(&original);
    let (result, observed) = capture(&fixture, |filesystem| {
        let inode = captured_inode(filesystem, "/input");
        clear_observations();
        let mut bytes = [0; 5];
        assert_eq!(inode.read_at(&mut bytes, 0)?, 5);
        assert_eq!(&bytes, b"hello");
        Ok(())
    });
    assert_eq!(result, Ok(()));
    let reads: Vec<_> = observed
        .iter()
        .filter(|item| item.event == ax_sync::ProfileEvent::BlockRead as u8)
        .collect();
    assert!(!reads.is_empty());
    assert!(reads.iter().all(|item| item.locked));
}

#[test]
fn oversized_read_fallback_returns_the_entire_requested_file() {
    let fixture = Fixture::new("oversized-read-fallback");
    let original = fixture.mount();
    let contents = vec![0x5a; 1024 * 1024 + 33];
    create_named_file(&original, "input", &contents);
    let (result, observed) = capture(&fixture, |filesystem| {
        let inode = captured_inode(filesystem, "/input");
        clear_observations();
        let mut bytes = vec![0; contents.len()];
        assert_eq!(inode.read_at(&mut bytes, 0)?, contents.len());
        assert_eq!(bytes, contents);
        Ok(())
    });
    assert_eq!(result, Ok(()));
    let reads: Vec<_> = observed
        .iter()
        .filter(|item| item.event == ax_sync::ProfileEvent::BlockRead as u8)
        .collect();
    assert!(!reads.is_empty());
    assert!(reads.iter().all(|item| item.locked));
}

#[test]
fn independent_read_error_releases_both_inode_and_metadata_locks() {
    let fixture = Fixture::new("independent-read-error");
    let original = fixture.mount();
    fixture.create_file(&original);
    let (result, _) = capture(&fixture, |filesystem| {
        let inode = captured_inode(filesystem, "/input");
        let number = InodeNumber::new(inode.inode() as u32).unwrap();
        let (block, lock) = {
            let mut state = filesystem.lock();
            let (fs, dev) = state.split();
            let mut node = fs.get_inode_by_num(dev, number).unwrap();
            let block = rsext4::loopfile::resolve_inode_block(dev, &mut node, 0)
                .unwrap()
                .unwrap();
            (block, state.inode_io_lock(number).unwrap())
        };
        fixture.fail_read_at(Some(block.raw() * 8));
        let mut bytes = [0xa5; 5];
        let result = inode.read_at(&mut bytes, 0);
        fixture.fail_read_at(None);
        assert_eq!(result, Err(VfsError::Io));
        assert_eq!(bytes, [0xa5; 5]);
        assert!(lock.try_lock().is_some());
        assert!(filesystem.inner.try_lock().is_some());
        Ok(())
    });
    assert_eq!(result, Ok(()));
}

#[test]
fn hold_scope_begins_after_lock_and_ends_before_unlock() {
    let fixture = Fixture::new("profile-lock-owner");
    let (result, observed) = capture(&fixture, |fs| {
        drop(fs.lock());
        Ok(())
    });
    assert_eq!(result, Ok(()));
    assert_lock_boundaries(&observed);
}

#[test]
fn explicit_sync_records_lock_owner_and_device_flush() {
    let fixture = Fixture::new("profile-sync-owner");
    let (result, observed) = capture(&fixture, Ext4Filesystem::sync_to_disk);
    assert_eq!(result, Ok(()));
    assert_lock_boundaries(&observed);
    assert_flush_boundaries(&observed);
}

#[test]
fn shutdown_records_lock_owner_and_device_flush() {
    let fixture = Fixture::new("profile-shutdown-owner");
    let (result, observed) = capture(&fixture, Ext4Filesystem::shutdown_filesystem);
    assert_eq!(result, Ok(()));
    assert_lock_boundaries(&observed);
    assert_flush_boundaries(&observed);
}

#[test]
fn failed_sync_ends_both_scopes_and_releases_lock() {
    let fixture = Fixture::new("profile-sync-failure");
    let (result, observed) = capture(&fixture, |fs| {
        fixture.fail_flush();
        fs.sync_to_disk()
    });
    assert_eq!(result, Err(VfsError::Io));
    assert_lock_boundaries(&observed);
    assert_flush_boundaries(&observed);
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Boundary {
    Begin,
    End,
}

#[derive(Debug, PartialEq)]
struct Observation {
    event: u8,
    boundary: Boundary,
    locked: bool,
    inode_locked: Option<bool>,
}

struct Capture {
    filesystem: Arc<Ext4Filesystem>,
    observed: Vec<Observation>,
    inode_io_lock: Option<Arc<Mutex<()>>>,
    before_read: Option<Rc<dyn Fn()>>,
    after_read: Option<Rc<dyn Fn()>>,
    reject_fs_lock: bool,
}

std::thread_local! {
    // All tests install identical hooks; unrelated threads have no capture.
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
}

fn capture(
    fixture: &Fixture,
    operation: impl FnOnce(&Ext4Filesystem) -> VfsResult<()>,
) -> (VfsResult<()>, Vec<Observation>) {
    let (device, region) = fixture.open_device();
    let disk = Ext4Disk::new(device, region);
    let reader = disk.shared_reader();
    let write_sequence = disk.write_sequence.clone();
    let mut dev = Jbd2Dev::initial_jbd2dev(0, disk, true);
    let fs = rsext4::mount(&mut dev).unwrap();
    let filesystem = Arc::new(Ext4Filesystem {
        inode_cache: fs.inodetable_cache.reader(),
        block_size: fs.superblock.block_size(),
        inner: Mutex::new(Ext4State {
            fs,
            dev,
            live_refs: BTreeMap::new(),
            zero_link: BTreeSet::new(),
        }),
        reader,
        write_sequence,
        root_dir: OnceCell::new(),
        readonly: false,
    });
    CAPTURE.with_borrow_mut(|slot| {
        assert!(slot.is_none());
        *slot = Some(Capture {
            filesystem: filesystem.clone(),
            observed: Vec::new(),
            inode_io_lock: None,
            before_read: None,
            after_read: None,
            reject_fs_lock: false,
        });
    });
    ax_sync::register_profile_hooks(begin, end);
    let result = operation(&filesystem);
    let capture = CAPTURE.with_borrow_mut(Option::take).unwrap();
    assert!(filesystem.inner.try_lock().is_some());
    (result, capture.observed)
}

fn begin(event: ax_sync::ProfileEvent, _object: usize) -> u64 {
    if event == ax_sync::ProfileEvent::Ext4 {
        CAPTURE.with_borrow(|slot| {
            assert!(
                !slot.as_ref().is_some_and(|capture| capture.reject_fs_lock),
                "cached inode query tried to reacquire a held filesystem lock"
            );
        });
    }
    record(event as u8, Boundary::Begin);
    if event == ax_sync::ProfileEvent::BlockRead {
        let callback = CAPTURE.with_borrow(|slot| {
            slot.as_ref()
                .and_then(|capture| capture.before_read.clone())
        });
        if let Some(callback) = callback {
            callback();
        }
    }
    event as u64
}

fn end(token: u64) {
    record(token as u8, Boundary::End);
    if token == ax_sync::ProfileEvent::BlockRead as u64 {
        let callback = CAPTURE
            .with_borrow(|slot| slot.as_ref().and_then(|capture| capture.after_read.clone()));
        if let Some(callback) = callback {
            callback();
        }
    }
}

fn record(event: u8, boundary: Boundary) {
    CAPTURE.with_borrow_mut(|slot| {
        if let Some(capture) = slot {
            capture.observed.push(Observation {
                event,
                boundary,
                locked: capture.filesystem.inner.try_lock().is_none(),
                inode_locked: capture
                    .inode_io_lock
                    .as_ref()
                    .map(|lock| lock.try_lock().is_none()),
            });
        }
    });
}

fn captured_inode(filesystem: &Ext4Filesystem, path: &str) -> Arc<Inode> {
    let registration = {
        let mut state = filesystem.lock();
        let (fs, dev) = state.split();
        let (number, _) = rsext4::dir::get_inode_with_num(fs, dev, path)
            .unwrap()
            .unwrap();
        state.inc_ref(number)
    };
    let shared = CAPTURE.with_borrow(|slot| slot.as_ref().unwrap().filesystem.clone());
    Inode::new(shared, registration, None, Some(path.into()))
}

fn watch_inode(filesystem: &Ext4Filesystem, inode: &Inode) {
    let lock = filesystem
        .lock()
        .inode_io_lock(InodeNumber::new(inode.inode() as u32).unwrap())
        .unwrap();
    CAPTURE.with_borrow_mut(|slot| slot.as_mut().unwrap().inode_io_lock = Some(lock));
}

fn clear_observations() {
    CAPTURE.with_borrow_mut(|slot| slot.as_mut().unwrap().observed.clear());
}

fn create_named_file(filesystem: &Filesystem, name: &str, bytes: &[u8]) {
    let entry = filesystem
        .root_dir()
        .as_dir()
        .unwrap()
        .create(
            name,
            NodeType::RegularFile,
            NodePermission::from_bits_truncate(0o600),
            0,
            0,
        )
        .unwrap();
    assert_eq!(
        entry.as_file().unwrap().write_at(bytes, 0).unwrap(),
        bytes.len()
    );
    entry.sync(false).unwrap();
}

fn assert_lock_boundaries(observed: &[Observation]) {
    let boundaries: Vec<_> = observed
        .iter()
        .filter(|item| item.event == 2 || item.event == EXT4_HOLD)
        .map(|item| (item.event, item.boundary, item.locked))
        .collect();
    assert_eq!(
        boundaries,
        [
            (2, Boundary::Begin, false),
            (EXT4_HOLD, Boundary::Begin, true),
            (EXT4_HOLD, Boundary::End, true),
            (2, Boundary::End, false),
        ]
    );
}

fn assert_flush_boundaries(observed: &[Observation]) {
    let boundaries: Vec<_> = observed
        .iter()
        .filter(|item| item.event == BLOCK_FLUSH)
        .collect();
    assert!(!boundaries.is_empty(), "device flush was not profiled");
    let (pairs, remainder) = boundaries.as_chunks::<2>();
    assert!(remainder.is_empty());
    for [begin, end] in pairs {
        assert_eq!(begin.boundary, Boundary::Begin);
        assert_eq!(end.boundary, Boundary::End);
        assert!(begin.locked && end.locked);
    }
}
