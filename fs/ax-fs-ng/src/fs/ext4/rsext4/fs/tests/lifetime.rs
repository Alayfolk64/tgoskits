//! Allocation references exist independently of VFS wrapper publication.

use std::{sync::mpsc, thread, time::Duration};

use axfs_ng_vfs::NodeOps;
use rsext4::{FileName, FilePermissions, MutationContext};

use super::*;

#[test]
fn unwrapped_inode_reference_prevents_reap_until_the_last_owner_is_dropped() {
    let (filesystem, _) = test_filesystem(false);
    let filesystem = Arc::new(filesystem);
    let (first, second) = {
        let mut state = filesystem.lock();
        let root = state.ext4.root_inode();
        let created = state
            .ext4
            .create_regular_file(
                MutationContext::new(0, 0, 0, 0),
                root,
                FileName::new(b"pending-publication").unwrap(),
                FilePermissions::new(0o600).unwrap(),
            )
            .unwrap();
        (
            state.retain_inode(&filesystem, created.number),
            state.retain_inode(&filesystem, created.number),
        )
    };
    let number = first.number();
    assert!(Arc::ptr_eq(first.content_access(), second.content_access()));
    {
        let mut state = filesystem.lock();
        let root = state.ext4.root_inode();
        let outcome = state
            .ext4
            .unlink(root, FileName::new(b"pending-publication").unwrap())
            .unwrap();
        assert!(outcome.requires_reap());
        let access = filesystem.inode_access(number);
        assert_eq!(state.publish_zero_link(number, access), None);
        assert!(state.claim_pending_reap().is_none());
    }

    assert_eq!(first.metadata().unwrap().links, 0);
    drop(first);
    assert!(filesystem.lock().claim_pending_reap().is_none());
    assert_eq!(second.metadata().unwrap().links, 0);
    drop(second);
    filesystem.sync_to_disk().unwrap();

    let mut state = filesystem.lock();
    assert!(!state.has_pending_reaps());
    assert_eq!(
        state.ext4.inode(number).unwrap_err().kind(),
        rsext4::Ext4ErrorKind::NotFound
    );
}

#[test]
fn moving_an_allocation_reference_into_a_wrapper_does_not_register_it_twice() {
    let (filesystem, _) = test_filesystem(false);
    let filesystem = Arc::new(filesystem);
    let lifetime = {
        let mut state = filesystem.lock();
        let root = state.ext4.root_inode();
        state.retain_inode(&filesystem, root)
    };
    let number = lifetime.number();
    let access = lifetime.content_access().clone();
    assert_eq!(access.lifetime_refs(), 1);
    let inode = Inode::new(lifetime, None);
    assert_eq!(access.lifetime_refs(), 1);
    drop(inode);
    assert_eq!(access.lifetime_refs(), 0);
    assert_eq!(number, filesystem.lock().ext4.root_inode());
}

#[test]
fn dropping_an_inode_lifetime_never_waits_for_mount_state() {
    let (filesystem, _) = test_filesystem(false);
    let filesystem = Arc::new(filesystem);
    let lifetime = {
        let mut state = filesystem.lock();
        let root = state.ext4.root_inode();
        state.retain_inode(&filesystem, root)
    };

    let state = filesystem.lock();
    let (completed_tx, completed_rx) = mpsc::channel();
    let dropper = thread::spawn(move || {
        drop(lifetime);
        completed_tx.send(()).unwrap();
    });
    let completed_without_mount_state = completed_rx
        .recv_timeout(Duration::from_millis(100))
        .is_ok();
    drop(state);
    dropper.join().unwrap();

    assert!(
        completed_without_mount_state,
        "inode lifetime drop waited for the ext4 mount-state mutex"
    );
}

#[test]
fn hot_regular_file_metadata_does_not_wait_for_mount_state() {
    let (filesystem, _) = test_filesystem(false);
    let filesystem = Arc::new(filesystem);
    let inode = {
        let mut state = filesystem.lock();
        let root = state.ext4.root_inode();
        let created = state
            .ext4
            .create_regular_file(
                MutationContext::new(0, 0, 0, 0),
                root,
                FileName::new(b"hot-metadata").unwrap(),
                FilePermissions::new(0o600).unwrap(),
            )
            .unwrap();
        Inode::new(state.retain_inode(&filesystem, created.number), None)
    };
    let expected_size = inode.metadata().unwrap().size;
    let expected_policy = inode.writeback_policy().unwrap();
    filesystem.sync_to_disk().unwrap();
    {
        let mut state = filesystem.lock();
        let root = state.ext4.root_inode();
        for index in 0..=rsext4::INODE_CACHE_MAX {
            let name = std::format!("hot-metadata-pressure-{index}");
            state
                .ext4
                .create_regular_file(
                    MutationContext::new(0, 0, 0, 0),
                    root,
                    FileName::new(name.as_bytes()).unwrap(),
                    FilePermissions::new(0o600).unwrap(),
                )
                .unwrap();
        }
    }
    assert!(
        filesystem
            .inode_metadata
            .try_get(InodeNumber::new(inode.inode() as u32).unwrap())
            .unwrap()
            .is_none(),
        "fixture did not evict the authoritative inode-table cache entry"
    );

    let state = filesystem.lock();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (completed_tx, completed_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        ready_tx.send(()).unwrap();
        assert_eq!(inode.len().unwrap(), expected_size);
        assert_eq!(inode.writeback_policy().unwrap(), expected_policy);
        completed_tx.send(()).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let completed_without_mount_state = completed_rx
        .recv_timeout(Duration::from_millis(100))
        .is_ok();
    drop(state);
    reader.join().unwrap();

    assert!(
        completed_without_mount_state,
        "hot regular-file metadata waited for the ext4 mount-state mutex"
    );
}
