use super::*;

impl FileNodeOps for Inode {
    fn read_at(&self, buf: &mut [u8], offset: u64) -> VfsResult<usize> {
        let _io = self.io_lock.lock();
        self.fs.read_inode(self.ino, offset, buf)
    }

    fn write_at(&self, buf: &[u8], offset: u64) -> VfsResult<usize> {
        let _io = self.io_lock.lock();
        let mut state = self.fs.lock();
        let (fs, dev) = state.split();
        // Use inode-number-based write so open-unlinked regular files remain
        // writable after their directory entry has been removed. The modified
        // rsext4 cache stays dirty until NodeOps::sync supplies the fsync
        // durability boundary.
        rsext4::write_inode_data(dev, fs, self.ino, offset, buf).map_err(into_vfs_err)?;
        Ok(buf.len())
    }

    fn append(&self, buf: &[u8]) -> VfsResult<(usize, u64)> {
        let _io = self.io_lock.lock();
        let mut state = self.fs.lock();
        let (fs, dev) = state.split();
        let inode = fs.get_inode_by_num(dev, self.ino).map_err(into_vfs_err)?;
        let length = inode.size();
        rsext4::write_inode_data(dev, fs, self.ino, length, buf).map_err(into_vfs_err)?;
        Ok((buf.len(), length + buf.len() as u64))
    }

    fn set_len(&self, len: u64) -> VfsResult<()> {
        let _io = self.io_lock.lock();
        let mut state = self.fs.lock();
        let (fs, dev) = state.split();
        // An open-unlinked regular file stays alive by inode number, not by a
        // directory entry. Keep the size update dirty with the data; fsync
        // flushes both through NodeOps::sync.
        rsext4::truncate_inode(dev, fs, self.ino, len).map_err(into_vfs_err)
    }

    fn set_symlink(&self, target: &str) -> VfsResult<()> {
        let _io = self.io_lock.lock();
        let Some(_path) = self.path.clone() else {
            return Err(VfsError::InvalidInput);
        };

        {
            let mut state = self.fs.lock();
            let (fs, dev) = state.split();
            let mut inode = fs.get_inode_by_num(dev, self.ino).map_err(into_vfs_err)?;

            if !inode.is_symlink() {
                return Err(VfsError::InvalidInput);
            }

            if let Ok(blocks) = rsext4::loopfile::resolve_inode_block_allextend(fs, dev, &mut inode)
            {
                for blk in blocks.values() {
                    let _ = fs.free_block(dev, *blk);
                }
            }

            let target_bytes = target.as_bytes();
            let target_len = target_bytes.len();
            inode.i_size_lo = (target_len as u64 & 0xffffffff) as u32;
            inode.i_size_high = ((target_len as u64) >> 32) as u32;
            inode.i_blocks_lo = 0;
            inode.l_i_blocks_high = 0;
            inode.i_block = [0; 15];

            if target_len == 0 {
                inode.i_flags &= !rsext4::disknode::Ext4Inode::EXT4_EXTENTS_FL;
            } else if target_len <= 60 {
                inode.i_flags &= !rsext4::disknode::Ext4Inode::EXT4_EXTENTS_FL;
                let mut raw = [0u8; 60];
                raw[..target_len].copy_from_slice(target_bytes);
                for i in 0..15 {
                    inode.i_block[i] = u32::from_le_bytes([
                        raw[i * 4],
                        raw[i * 4 + 1],
                        raw[i * 4 + 2],
                        raw[i * 4 + 3],
                    ]);
                }
            } else {
                if !fs.superblock.has_extents() {
                    return Err(VfsError::Unsupported);
                }

                let mut data_blocks = alloc::vec::Vec::new();
                let mut remaining = target_len;
                let mut src_off = 0usize;
                while remaining > 0 {
                    let blk = fs.alloc_block(dev).map_err(into_vfs_err)?;
                    let write_len = core::cmp::min(remaining, BLOCK_SIZE);
                    fs.datablock_cache
                        .modify_new(dev, blk, |data| {
                            for b in data.iter_mut() {
                                *b = 0;
                            }
                            let end = src_off + write_len;
                            data[..write_len].copy_from_slice(&target_bytes[src_off..end]);
                        })
                        .map_err(into_vfs_err)?;
                    data_blocks.push(blk);
                    remaining -= write_len;
                    src_off += write_len;
                }

                let used_datablocks = data_blocks.len() as u64;
                let iblocks_used = used_datablocks.saturating_mul(BLOCK_SIZE as u64 / 512) as u32;
                inode.i_blocks_lo = iblocks_used;
                inode.l_i_blocks_high = 0;
                rsext4::file::build_file_block_mapping_with_inode_num(
                    fs,
                    &mut inode,
                    self.ino,
                    &data_blocks,
                    dev,
                )
                .map_err(into_vfs_err)?;
            }

            fs.modify_inode(dev, self.ino, |on_disk| {
                *on_disk = inode;
            })
            .map_err(into_vfs_err)?;
        }

        self.fs.sync_to_disk()
    }
}

impl FsPollable for Inode {
    fn poll(&self) -> FsIoEvents {
        FsIoEvents::IN | FsIoEvents::OUT
    }

    fn register(&self, _context: &mut core::task::Context<'_>, _events: FsIoEvents) {}
}
