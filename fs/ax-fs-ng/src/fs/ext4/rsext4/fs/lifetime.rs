//! Owned inode references spanning VFS publication and independent reads.

use alloc::sync::Arc;

use axfs_ng_vfs::{VfsResult, WritebackPolicy};
use rsext4::{DirectoryEntryType, InodeFlags, InodeNumber};

use super::{AccessGate, Ext4Filesystem, Ext4State, into_vfs_err, namespace::NamespaceChange};

/// A registered reference to one allocated inode on one mount. This owner can
/// exist before its VFS wrapper: an independent metadata read must not rely on
/// a bare inode number while unlink/reap can run on another task.
///
/// Drop outside the mount lock. Release only updates the atomic owner count;
/// the sleepable writeback owner performs any final orphan reap.
pub(crate) struct InodeLifetime {
    filesystem: Arc<Ext4Filesystem>,
    number: InodeNumber,
    access: Arc<AccessGate>,
}

/// An authoritative namespace result with an already retained allocation.
pub(crate) struct LocatedInode {
    pub(crate) lifetime: InodeLifetime,
    pub(crate) file_type: rsext4::DirectoryEntryType,
}

impl Ext4State {
    /// A retained directory cannot be reaped, but rmdir can have detached it.
    /// The lifetime tracker is the existing authority for that publication;
    /// do not reload inode tables or create another parent-version registry.
    pub(crate) fn ensure_linked_parent(&self, number: InodeNumber) -> rsext4::Ext4Result<()> {
        if self.lifetimes.zero_link.contains_key(&number) {
            Err(rsext4::Ext4Error::not_found().with_operation("namespace:removed_parent"))
        } else {
            Ok(())
        }
    }

    /// Acquire while the caller still owns the authoritative lookup/create
    /// result. Return the owner out of this critical section before performing
    /// fallible work, invoking callbacks, or constructing VFS directory entries.
    /// `filesystem` must own this state; the caller already holds its guard.
    pub(crate) fn retain_inode(
        &mut self,
        filesystem: &Arc<Ext4Filesystem>,
        number: InodeNumber,
    ) -> InodeLifetime {
        let access = filesystem.inode_access(number);
        access.retain_lifetime();
        InodeLifetime {
            filesystem: filesystem.clone(),
            number,
            access,
        }
    }
}

impl InodeLifetime {
    /// Replays only atomic namespace operations, never partial file mutations.
    /// Every attempt re-resolves names under fresh namespace exclusion. The
    /// closure must retain any successful child before releasing mount state.
    pub(crate) fn mutate_namespace<T>(
        &self,
        scope: NamespaceChange,
        mut operation: impl FnMut(&mut Ext4State) -> rsext4::Ext4Result<T>,
    ) -> VfsResult<T> {
        let filesystem = &self.filesystem;
        let _admission = filesystem.admission.enter().map_err(into_vfs_err)?;
        loop {
            let attempt = {
                let _namespace = filesystem.namespace.change(self.number, scope)?;
                filesystem.attempt_admitted_mutation(|state| {
                    state.ensure_linked_parent(self.number)?;
                    operation(state)
                })
            };
            // All namespace and mount guards are gone before committing or
            // persisting an abort. Admission and allocation references remain.
            if let Some(value) = filesystem
                .finish_mutation_attempt(attempt)
                .map_err(into_vfs_err)?
            {
                return Ok(value);
            }
        }
    }

    /// Lookup and reference acquisition share one namespace publication point.
    /// Later unlink may remove the name, but cannot recycle the selected inode
    /// during its independent metadata load. The VFS cache separately checks
    /// its mutation generation before publishing an in-flight lookup result.
    pub(crate) fn lookup(&self, name: rsext4::FileName<'_>) -> VfsResult<Option<LocatedInode>> {
        let _admission = self.filesystem.admission.enter().map_err(into_vfs_err)?;
        let lifetime = {
            let _namespace = self.filesystem.namespace.lookup(self.number)?;
            let Some(lifetime) = self
                .filesystem
                .lookup_admitted_child(self.number, name)
                .map_err(into_vfs_err)?
            else {
                return Ok(None);
            };
            lifetime
        };
        // On any error this owner is dropped after the mount guard, so its
        // destructor can safely enter the existing zero-link reap protocol.
        let info = self
            .filesystem
            .read_admitted_live_inode_info(lifetime.number)
            .map_err(into_vfs_err)?;
        Ok(Some(LocatedInode {
            lifetime,
            file_type: info.file_type(),
        }))
    }

    /// Inspect while retaining the same allocation across any cold table read.
    pub(crate) fn metadata(&self) -> rsext4::Ext4Result<rsext4::InodeInfo> {
        let info = self.filesystem.read_live_inode_info(self.number)?;
        if info.file_type() == DirectoryEntryType::RegularFile {
            self.access.initialize_regular_file_size(info.size);
        }
        self.access
            .initialize_writeback_policy(writeback_policy(info.flags));
        Ok(info)
    }

    /// Reads Linux-style hot `i_size` state for regular files. Directories and
    /// special nodes retain authoritative metadata reads because their size
    /// changes are not serialized by regular-file content access.
    pub(crate) fn file_size(&self) -> rsext4::Ext4Result<u64> {
        if let Some(size) = self.access.regular_file_size() {
            return Ok(size);
        }
        let info = self.metadata()?;
        Ok(self.access.regular_file_size().unwrap_or(info.size))
    }

    pub(crate) fn publish_file_size(&self, size: u64) {
        self.access.publish_regular_file_size(size);
    }

    pub(crate) fn writeback_policy(&self) -> rsext4::Ext4Result<WritebackPolicy> {
        if let Some(policy) = self.access.writeback_policy() {
            return Ok(policy);
        }
        let info = self.metadata()?;
        Ok(self
            .access
            .writeback_policy()
            .unwrap_or_else(|| writeback_policy(info.flags)))
    }

    pub(crate) fn filesystem(&self) -> &Arc<Ext4Filesystem> {
        &self.filesystem
    }

    pub(crate) fn number(&self) -> InodeNumber {
        self.number
    }

    pub(crate) fn content_access(&self) -> &Arc<AccessGate> {
        &self.access
    }
}

fn writeback_policy(flags: InodeFlags) -> WritebackPolicy {
    let mut policy = WritebackPolicy::empty();
    policy.set(
        WritebackPolicy::SYNCHRONOUS,
        flags.contains(InodeFlags::SYNC),
    );
    policy.set(
        WritebackPolicy::DIRECTORY_SYNC,
        flags.contains(InodeFlags::DIRECTORY_SYNC),
    );
    policy
}

impl Drop for InodeLifetime {
    fn drop(&mut self) {
        // Linux drops `i_count` atomically and leaves final orphan eviction to
        // a sleepable owner. Arc destruction may run while a non-sleeping VFS
        // or MM guard is held, so it must never wait for mount state here.
        if self.access.release_lifetime() {
            self.filesystem.writeback.notify();
        }
    }
}
