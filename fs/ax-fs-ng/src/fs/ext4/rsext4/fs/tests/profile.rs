//! Observe the real guard and detached device boundaries, including errors.

use core::cell::RefCell;

use ax_sync::ProfileEvent;
use axfs_ng_vfs::{MetadataUpdate, NodePermission, NodeType, WritebackPolicy};

use super::{super::access::WriteAccess, *};

std::thread_local! {
    static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) };
}

#[test]
fn hold_profile_begins_after_locking_and_ends_before_unlocking() {
    let (filesystem, _) = test_filesystem(false);
    let filesystem = Arc::new(filesystem);
    let trace = TraceGuard::begin(&filesystem);
    drop(filesystem.lock());
    let records = trace.finish();

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].event, ProfileEvent::Ext4);
    assert!(!records[0].locked_at_begin);
    assert_eq!(records[0].locked_at_end, Some(false));
    assert_eq!(records[1].event, ProfileEvent::Ext4LockHold);
    assert!(records[1].locked_at_begin);
    assert_eq!(records[1].locked_at_end, Some(true));
}

#[test]
fn explicit_background_sync_profiles_writes_and_flushes_outside_ext4() {
    let (filesystem, root, _) = super::sync_policy::background_mount(WritebackPolicy::empty());
    make_dirty_file(&root);
    let trace = TraceGuard::begin(&filesystem);

    filesystem.sync_to_disk().unwrap();

    assert_detached_io(
        &trace.finish(),
        &[ProfileEvent::BlockWrite, ProfileEvent::BlockFlush],
    );
}

#[test]
fn background_shutdown_profiles_final_clean_io_outside_ext4() {
    let (filesystem, root, _) = super::sync_policy::background_mount(WritebackPolicy::empty());
    make_dirty_file(&root);
    let trace = TraceGuard::begin(&filesystem);

    filesystem.shutdown_filesystem().unwrap();

    assert_detached_io(
        &trace.finish(),
        &[ProfileEvent::BlockWrite, ProfileEvent::BlockFlush],
    );
    assert!(filesystem.lock().shutdown_attempted);
}

#[test]
fn failed_flush_closes_profile_scopes_and_propagates_the_device_error() {
    let (filesystem, root, _) = super::sync_policy::background_mount(WritebackPolicy::empty());
    make_dirty_file(&root);
    let trace = TraceGuard::begin(&filesystem);
    TRACE.with_borrow_mut(|trace| trace.as_mut().unwrap().fail_flush = true);

    assert_eq!(filesystem.sync_to_disk(), Err(VfsError::Io));

    TRACE.with_borrow(|trace| assert!(!trace.as_ref().unwrap().fail_flush));
    // The first ordering flush may fail before the first journal write.
    assert_detached_io(&trace.finish(), &[ProfileEvent::BlockFlush]);
    assert!(filesystem.inner.try_lock().is_some());
}

#[test]
fn contended_inode_read_remains_profiled_through_shared_acquisition() {
    let (filesystem, _) = test_filesystem(false);
    let filesystem = Arc::new(filesystem);
    let number = filesystem.lock().ext4.root_inode();
    let gate = filesystem.inode_access(number);
    let writer = gate.write().unwrap();
    let trace = TraceGuard::begin(&filesystem);
    TRACE.with_borrow_mut(|slot| slot.as_mut().unwrap().release_access = Some(writer));

    let reader = gate.read().unwrap();

    let records = trace.finish();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event, ProfileEvent::MutexWait);
    assert!(!records[0].locked_at_begin);
    assert_eq!(records[0].locked_at_end, Some(false));
    assert!(gate.try_write().is_none());
    drop(reader);
    assert!(gate.try_write().is_some());
}

fn make_dirty_file(root: &axfs_ng_vfs::Location) {
    let file = root
        .create(
            "dirty",
            NodeType::RegularFile,
            NodePermission::default(),
            0,
            0,
        )
        .unwrap();
    file.update_metadata(MetadataUpdate {
        mode: Some(NodePermission::from_bits_truncate(0o640)),
        ..Default::default()
    })
    .unwrap();
}

fn assert_detached_io(records: &[Record], required: &[ProfileEvent]) {
    for event in required {
        assert!(
            records.iter().any(|record| record.event == *event),
            "sync did not reach {event:?}"
        );
    }
    for event in [ProfileEvent::BlockWrite, ProfileEvent::BlockFlush] {
        let matching: Vec<_> = records
            .iter()
            .filter(|record| record.event == event)
            .collect();
        for record in matching {
            assert!(
                !record.locked_at_begin,
                "{event:?} began under ext4 exclusion"
            );
            assert_eq!(
                record.locked_at_end,
                Some(false),
                "{event:?} ended under ext4 exclusion"
            );
        }
    }
    assert!(records.iter().all(|record| record.locked_at_end.is_some()));
}

struct Trace {
    filesystem: Weak<Ext4Filesystem>,
    records: Vec<Record>,
    fail_flush: bool,
    release_access: Option<WriteAccess>,
}

struct Record {
    event: ProfileEvent,
    locked_at_begin: bool,
    locked_at_end: Option<bool>,
}

struct TraceGuard;

impl TraceGuard {
    fn begin(filesystem: &Arc<Ext4Filesystem>) -> Self {
        install_hooks();
        TRACE.with_borrow_mut(|slot| {
            assert!(slot.is_none());
            *slot = Some(Trace {
                filesystem: Arc::downgrade(filesystem),
                records: Vec::new(),
                fail_flush: false,
                release_access: None,
            });
        });
        Self
    }

    fn finish(self) -> Vec<Record> {
        TRACE.with_borrow_mut(Option::take).unwrap().records
    }
}

impl Drop for TraceGuard {
    fn drop(&mut self) {
        TRACE.with_borrow_mut(Option::take);
    }
}

pub(super) fn install_hooks() {
    // All active ext4 tests install the same hooks. Per-thread observations
    // cannot replace another test's begin/end callback during a live scope.
    ax_sync::register_profile_hooks(begin, end);
}

fn begin(event: ProfileEvent, _: usize) -> u64 {
    super::metadata::inspect_profile_event(event);
    super::inode_io::inspect_profile_event(event);
    let (token, release) = TRACE.with_borrow_mut(|slot| {
        let Some(trace) = slot else { return (0, None) };
        let filesystem = trace.filesystem.upgrade().unwrap();
        trace.records.push(Record {
            event,
            locked_at_begin: filesystem.inner.try_lock().is_none(),
            locked_at_end: None,
        });
        let release = (event == ProfileEvent::MutexWait)
            .then(|| trace.release_access.take())
            .flatten();
        (trace.records.len() as u64, release)
    });
    // Release the competing owner at the actual contended-acquisition boundary,
    // outside the trace borrow. The acquiring reader must still close its scope.
    drop(release);
    token
}

fn end(token: u64) {
    TRACE.with_borrow_mut(|slot| {
        let Some(trace) = slot else { return };
        let filesystem = trace.filesystem.upgrade().unwrap();
        let record = &mut trace.records[token as usize - 1];
        assert!(record.locked_at_end.is_none(), "profile scope ended twice");
        record.locked_at_end = Some(filesystem.inner.try_lock().is_none());
    });
}

pub(super) fn observe_device_flush() -> BlockResult {
    TRACE.with_borrow_mut(|slot| {
        if slot
            .as_mut()
            .is_some_and(|trace| core::mem::take(&mut trace.fail_flush))
        {
            Err(BlockError::Io)
        } else {
            Ok(())
        }
    })
}
