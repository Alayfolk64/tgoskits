//! Synchronous durability with a revalidated, lock-free table pre-read phase.

use core::sync::atomic::Ordering;

use super::*;

impl Ext4Filesystem {
    pub(crate) fn sync_to_disk(&self) -> VfsResult<()> {
        if self.readonly {
            return Ok(());
        }

        let mut state = self.lock();
        flush_data_and_bitmaps(&mut state)?;
        let write_sequence = self.write_sequence.load(Ordering::Relaxed);
        let plan = match (&self.reader, write_sequence) {
            (Some(reader), sequence) if sequence < u64::MAX => state
                .fs
                .inodetable_cache
                .prepare_flush(&state.dev)
                .map_err(into_vfs_err)?
                .map(|plan| (reader, plan)),
            _ => None,
        };
        if let Some((reader, plan)) = plan {
            drop(state);
            let read = plan
                .read(|block, bytes| reader.read(block, bytes))
                .map_err(into_vfs_err)?;
            state = self.lock();
            // Other tasks can dirty caches while we read. Preserve sync order
            // before deciding whether their writes invalidated the table bytes.
            flush_data_and_bitmaps(&mut state)?;
            if self.write_sequence.load(Ordering::Relaxed) == write_sequence {
                let (fs, dev) = state.split();
                read.flush(&fs.inodetable_cache, dev)
                    .map_err(into_vfs_err)?;
            } else {
                // A write may have changed a table or committed a neighbor.
                // Re-read through the original path; never reuse stale bytes.
                drop(read);
                flush_inodes(&mut state)?;
            }
        } else {
            flush_inodes(&mut state)?;
        }
        finish_sync(&mut state)
    }
}

fn flush_data_and_bitmaps(state: &mut Ext4State) -> VfsResult<()> {
    let (fs, dev) = state.split();
    fs.datablock_cache.flush_all(dev).map_err(into_vfs_err)?;
    fs.bitmap_cache.flush_all(dev).map_err(into_vfs_err)
}

fn flush_inodes(state: &mut Ext4State) -> VfsResult<()> {
    let (fs, dev) = state.split();
    fs.inodetable_cache.flush_all(dev).map_err(into_vfs_err)
}

fn finish_sync(state: &mut Ext4State) -> VfsResult<()> {
    let (fs, dev) = state.split();
    // Preserve the existing clean-state and durability boundary.
    fs.superblock.s_state = Ext4Superblock::EXT4_VALID_FS;
    fs.sync_superblock(dev).map_err(into_vfs_err)?;
    fs.sync_group_descriptors(dev).map_err(into_vfs_err)?;
    if dev.is_use_journal() {
        dev.umount_commit().map_err(into_vfs_err)?;
    }
    dev.cantflush().map_err(into_vfs_err)
}
