use ::alloc::collections::BTreeSet;

use super::*;

impl Ext4FileSystem {
    fn checked_orphan_number(&self, raw: u32) -> Ext4Result<Option<InodeNumber>> {
        if raw == 0 {
            return Ok(None);
        }
        if raw < self.superblock.s_first_ino || raw > self.superblock.s_inodes_count {
            return Err(Ext4Error::corrupted().with_operation("orphan:inode_range"));
        }
        InodeNumber::new(raw)
            .map(Some)
            .map_err(|_| Ext4Error::corrupted().with_operation("orphan:inode_number"))
    }

    fn orphan_next<B: BlockIo>(
        &mut self,
        block_dev: &mut Jbd2Dev<B>,
        inode_num: InodeNumber,
    ) -> Ext4Result<Option<InodeNumber>> {
        if !self.inode_is_allocated_checked(block_dev, inode_num)? {
            return Err(Ext4Error::corrupted().with_operation("orphan:inode_not_allocated"));
        }
        let inode = self.get_inode_by_num(block_dev, inode_num)?;
        self.checked_orphan_number(inode.i_dtime)
    }

    pub(crate) fn orphan_contains(&self, target: InodeNumber) -> bool {
        self.orphan_inodes.contains(&target)
    }

    pub(crate) fn load_orphan_index<B: BlockIo>(
        &mut self,
        block_dev: &mut Jbd2Dev<B>,
    ) -> Ext4Result<()> {
        self.orphan_inodes = self.read_orphan_index(block_dev)?;
        Ok(())
    }

    fn validate_orphan_index<B: BlockIo>(&mut self, block_dev: &mut Jbd2Dev<B>) -> Ext4Result<()> {
        if self.read_orphan_index(block_dev)? != self.orphan_inodes {
            return Err(Ext4Error::corrupted().with_operation("orphan:index_mismatch"));
        }
        Ok(())
    }

    fn read_orphan_index<B: BlockIo>(
        &mut self,
        block_dev: &mut Jbd2Dev<B>,
    ) -> Ext4Result<BTreeSet<InodeNumber>> {
        let mut current = self.checked_orphan_number(self.superblock.s_last_orphan)?;
        let mut visited = BTreeSet::new();
        while let Some(inode_num) = current {
            if !visited.insert(inode_num) {
                return Err(Ext4Error::corrupted().with_operation("orphan:cycle"));
            }
            let inode = self.get_inode_by_num(block_dev, inode_num)?;
            if inode.i_mode == 0 {
                return Err(Ext4Error::corrupted().with_operation("orphan:empty_inode"));
            }
            current = self.orphan_next(block_dev, inode_num)?;
        }
        Ok(visited)
    }

    /// Adds an inode to the classic ext4 orphan chain.
    ///
    /// The caller owns the surrounding filesystem transaction. This helper
    /// only performs the checked in-memory metadata transition; JBD2 must
    /// persist the inode-table and superblock updates atomically.
    pub(crate) fn add_orphan<B: BlockIo>(
        &mut self,
        block_dev: &mut Jbd2Dev<B>,
        inode_num: InodeNumber,
    ) -> Ext4Result<()> {
        if self.orphan_contains(inode_num) {
            return Ok(());
        }
        if !self.inode_is_allocated_checked(block_dev, inode_num)? {
            return Err(Ext4Error::corrupted().with_operation("orphan:add_unallocated"));
        }

        let head = self.checked_orphan_number(self.superblock.s_last_orphan)?;
        let inode = self.get_inode_by_num(block_dev, inode_num)?;
        if inode.i_dtime != 0 {
            return Err(Ext4Error::corrupted().with_operation("orphan:add_stale_dtime"));
        }
        self.modify_inode(block_dev, inode_num, |inode| {
            inode.i_dtime = head.map_or(0, InodeNumber::raw);
        })?;
        self.superblock.s_last_orphan = inode_num.raw();
        self.mark_superblock_dirty();
        self.orphan_inodes.insert(inode_num);
        Ok(())
    }

    /// Removes an inode from the classic ext4 orphan chain.
    pub(crate) fn remove_orphan<B: BlockIo>(
        &mut self,
        block_dev: &mut Jbd2Dev<B>,
        target: InodeNumber,
    ) -> Ext4Result<Option<InodeNumber>> {
        if !self.orphan_contains(target) {
            return Err(Ext4Error::not_found().with_operation("orphan:remove_missing"));
        }
        self.validate_orphan_index(block_dev)?;
        let mut previous = None;
        let mut current = self.checked_orphan_number(self.superblock.s_last_orphan)?;
        let mut visited = BTreeSet::new();

        while let Some(inode_num) = current {
            if !visited.insert(inode_num) {
                return Err(Ext4Error::corrupted().with_operation("orphan:cycle"));
            }
            let next = self.orphan_next(block_dev, inode_num)?;
            if inode_num == target {
                if let Some(previous_inode) = previous {
                    self.modify_inode(block_dev, previous_inode, |inode| {
                        inode.i_dtime = next.map_or(0, InodeNumber::raw);
                    })?;
                } else {
                    self.superblock.s_last_orphan = next.map_or(0, InodeNumber::raw);
                    self.mark_superblock_dirty();
                }
                self.modify_inode(block_dev, target, |inode| inode.i_dtime = 0)?;
                if !self.orphan_inodes.remove(&target) {
                    return Err(
                        Ext4Error::corrupted().with_operation("orphan:index_remove_missing")
                    );
                }
                return Ok(previous);
            }
            previous = Some(inode_num);
            current = next;
        }

        Err(Ext4Error::not_found().with_operation("orphan:remove_missing"))
    }

    pub(crate) fn recover_orphans<B: BlockIo>(
        &mut self,
        block_dev: &mut Jbd2Dev<B>,
    ) -> Ext4Result<()> {
        let mut visited = BTreeSet::new();
        loop {
            let Some(head) = self.checked_orphan_number(self.superblock.s_last_orphan)? else {
                return Ok(());
            };
            if !visited.insert(head) {
                return Err(Ext4Error::corrupted().with_operation("orphan:recovery_cycle"));
            }
            let inode = self.get_inode_by_num(block_dev, head)?;
            if inode.i_links_count != 0 {
                crate::file::recover_linked_truncate_inode(block_dev, self, head, inode.size())?;
                continue;
            }
            crate::file::reap_unlinked_inode(self, block_dev, head)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use ::alloc::{format, rc::Rc, vec, vec::Vec};

    use super::*;
    use crate::{
        Clock, DeviceCapabilities, DeviceGeometry, Ext4Timestamp, SectorId, mkfile, mkfs, unlink,
    };

    struct ReadCountingDevice {
        bytes: Vec<u8>,
        reads: Rc<Cell<usize>>,
    }

    impl BlockIo for ReadCountingDevice {
        fn read(&mut self, buffer: &mut [u8], sector: SectorId, _: u32) -> Ext4Result<()> {
            self.reads.set(self.reads.get() + 1);
            let sector_number = sector.to_u32()?;
            let total_blocks = (self.bytes.len() / BLOCK_SIZE) as u64;
            let start = sector.as_usize()? * BLOCK_SIZE;
            let end = start
                .checked_add(buffer.len())
                .ok_or_else(Ext4Error::overflow)?;
            let source = self
                .bytes
                .get(start..end)
                .ok_or_else(|| Ext4Error::block_out_of_range(sector_number, total_blocks))?;
            buffer.copy_from_slice(source);
            Ok(())
        }

        fn write(&mut self, buffer: &[u8], sector: SectorId, _: u32) -> Ext4Result<()> {
            let sector_number = sector.to_u32()?;
            let total_blocks = (self.bytes.len() / BLOCK_SIZE) as u64;
            let start = sector.as_usize()? * BLOCK_SIZE;
            let end = start
                .checked_add(buffer.len())
                .ok_or_else(Ext4Error::overflow)?;
            let destination = self
                .bytes
                .get_mut(start..end)
                .ok_or_else(|| Ext4Error::block_out_of_range(sector_number, total_blocks))?;
            destination.copy_from_slice(buffer);
            Ok(())
        }

        fn geometry(&self) -> DeviceGeometry {
            DeviceGeometry::new(BLOCK_SIZE_U32, (self.bytes.len() / BLOCK_SIZE) as u64)
        }

        fn capabilities(&self) -> DeviceCapabilities {
            DeviceCapabilities {
                flush: true,
                ..Default::default()
            }
        }

        fn flush(&mut self) -> Ext4Result<()> {
            Ok(())
        }
    }

    impl Clock for ReadCountingDevice {
        fn now(&self) -> Ext4Result<Ext4Timestamp> {
            Ok(Ext4Timestamp::UNIX_EPOCH)
        }
    }

    #[test]
    fn non_member_lookup_does_not_read_the_persistent_orphan_chain() {
        let reads = Rc::new(Cell::new(0));
        let device = ReadCountingDevice {
            bytes: vec![0; 32 * 1024 * 1024],
            reads: Rc::clone(&reads),
        };
        let mut journal = Jbd2Dev::initial_jbd2dev(0, device, true);
        mkfs(&mut journal).expect("format orphan index fixture");
        let mut filesystem = Ext4FileSystem::mount(&mut journal).expect("mount orphan fixture");
        mkfile(&mut journal, &mut filesystem, "/orphan", None, None)
            .expect("create orphan fixture");
        for index in 0..32 {
            let path = format!("/padding-{index}");
            mkfile(&mut journal, &mut filesystem, &path, None, None)
                .expect("create inode-table padding fixture");
        }
        mkfile(&mut journal, &mut filesystem, "/ordinary", None, None)
            .expect("create ordinary fixture");
        let ordinary = crate::dir::get_inode_with_num(&mut filesystem, &mut journal, "/ordinary")
            .expect("look up ordinary inode")
            .expect("ordinary inode exists")
            .0;
        let removed =
            unlink(&mut filesystem, &mut journal, "/orphan").expect("publish live orphan");
        assert!(removed.requires_reap());
        filesystem
            .sync_filesystem(&mut journal)
            .expect("persist live orphan fixture");
        journal.flush().expect("checkpoint live orphan fixture");

        filesystem.inodetable_cache.clear();
        filesystem
            .get_inode_by_num(&mut journal, ordinary)
            .expect("preload only the ordinary inode");
        reads.set(0);

        assert!(!filesystem.orphan_contains(ordinary));
        assert_eq!(reads.get(), 0, "membership lookup must remain memory-only");
    }
}
