use super::*;

impl DirNodeOps for Inode {
    fn read_dir(&self, offset: u64, sink: &mut dyn DirEntrySink) -> VfsResult<usize> {
        let mut state = self.fs.lock();
        let (fs, dev) = state.split();
        let mut inode = fs.get_inode_by_num(dev, self.ino).map_err(into_vfs_err)?;

        let blocks = rsext4::loopfile::resolve_inode_block_allextend(fs, dev, &mut inode)
            .map_err(into_vfs_err)?;

        let mut byte_offset: u64 = 0;
        let mut count = 0usize;
        for &phys in blocks.values() {
            let cached = fs
                .datablock_cache
                .get_or_load(dev, phys)
                .map_err(into_vfs_err)?;
            let data = &cached.data[..BLOCK_SIZE];

            // Manually iterate entries, tracking byte_offset for ALL entries
            // (including inode==0 deleted ones) so offset stays physical.
            let mut pos = 0usize;
            while pos + 8 <= data.len() {
                let entry_inode =
                    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
                let rec_len = u16::from_le_bytes([data[pos + 4], data[pos + 5]]);
                if rec_len < 8 {
                    break;
                }
                let rec_usize = rec_len as usize;
                if pos + rec_usize > data.len() {
                    break;
                }

                let entry_offset = byte_offset;
                byte_offset += rec_len as u64;
                pos += rec_usize;

                if entry_inode == 0 {
                    continue;
                }
                if entry_offset < offset {
                    continue;
                }

                let name_len = data[pos - rec_usize + 6] as usize;
                let file_type = data[pos - rec_usize + 7];
                let name_start = pos - rec_usize + 8;
                if name_len > rec_usize - 8 {
                    continue;
                }
                let name = core::str::from_utf8(&data[name_start..name_start + name_len])
                    .map_err(|_| VfsError::InvalidData)?
                    .to_owned();
                let node_type = dir_entry_type_to_vfs(file_type);
                if !sink.accept(&name, entry_inode as u64, node_type, byte_offset) {
                    return Ok(count);
                }
                count += 1;
            }
        }

        Ok(count)
    }

    fn lookup(&self, name: &str) -> VfsResult<DirEntry> {
        if name == "." {
            return self
                .this
                .as_ref()
                .and_then(WeakDirEntry::upgrade)
                .ok_or(VfsError::NotFound);
        }
        if name == ".." {
            return self
                .this
                .as_ref()
                .and_then(WeakDirEntry::upgrade)
                .and_then(|entry| entry.parent())
                .ok_or(VfsError::NotFound);
        }
        self.lookup_locked(name)
    }

    fn create(
        &self,
        name: &str,
        node_type: NodeType,
        permission: NodePermission,
        uid: u32,
        gid: u32,
    ) -> VfsResult<DirEntry> {
        let Some(dir_path) = self.dir_path().ok() else {
            return Err(VfsError::InvalidInput);
        };
        let path = join_child_path(&dir_path, name);
        let registration = {
            let mut state = self.fs.lock();
            let (fs, dev) = state.split();
            if rsext4::dir::get_inode_with_num(fs, dev, &path)
                .map_err(into_vfs_err)?
                .is_some()
            {
                return Err(VfsError::AlreadyExists);
            }

            if node_type == NodeType::Directory {
                rsext4::mkdir_with_owner(dev, fs, &path, uid, gid).map_err(into_vfs_err)?;
            } else {
                let file_type = vfs_type_to_dir_entry(node_type).ok_or(VfsError::InvalidData)?;
                rsext4::mkfile_with_owner(dev, fs, &path, None, Some(file_type), uid, gid)
                    .map_err(into_vfs_err)?;
            };

            let (ino, _inode) = rsext4::dir::get_inode_with_num(fs, dev, &path)
                .map_err(into_vfs_err)?
                .ok_or(VfsError::NotFound)?;

            let mode_bits = permission.bits();
            fs.modify_inode(dev, ino, |node| {
                node.i_mode = (node.i_mode & rsext4::disknode::Ext4Inode::S_IFMT) | mode_bits;
            })
            .map_err(into_vfs_err)?;
            Self::update_ctime_with(fs, dev, ino)?;
            state.inc_ref(ino)
        };

        self.fs.sync_to_disk()?;

        let reference = Reference::new(
            self.this.as_ref().and_then(WeakDirEntry::upgrade),
            name.to_owned(),
        );
        Ok(if node_type == NodeType::Directory {
            DirEntry::new_dir(
                |this| {
                    DirNode::new(Inode::new(
                        self.fs.clone(),
                        registration,
                        Some(this),
                        Some(path),
                    ))
                },
                reference,
            )
        } else {
            DirEntry::new_file(
                FileNode::new(Inode::new(self.fs.clone(), registration, None, Some(path))),
                node_type,
                reference,
            )
        })
    }

    fn link(&self, name: &str, node: &DirEntry) -> VfsResult<DirEntry> {
        let dir_path = self.dir_path()?;
        let link_path = join_child_path(&dir_path, name);
        let target_path = node.absolute_path()?.to_string();
        {
            let mut state = self.fs.lock();
            let (fs, dev) = state.split();

            if rsext4::dir::get_inode_with_num(fs, dev, &target_path)
                .map_err(into_vfs_err)?
                .is_none()
            {
                return Err(VfsError::NotFound);
            }
            if rsext4::dir::get_inode_with_num(fs, dev, &link_path)
                .map_err(into_vfs_err)?
                .is_some()
            {
                return Err(VfsError::AlreadyExists);
            }

            rsext4::link(fs, dev, &link_path, &target_path).map_err(into_vfs_err)?;
            let target_ino = InodeNumber::new(node.inode() as u32).map_err(into_vfs_err)?;
            Self::update_ctime_with(fs, dev, target_ino)?;
        }
        self.lookup_locked(name)
    }

    fn unlink(&self, name: &str, is_dir: bool) -> VfsResult<()> {
        let dir_path = self.dir_path()?;
        let path = join_child_path(&dir_path, name);
        let forget_file_ino: Cell<Option<InodeNumber>> = Cell::new(None);
        {
            let mut state = self.fs.lock();
            let (fs, dev) = state.split();
            let inode_info =
                rsext4::dir::get_inode_with_num(fs, dev, &path).map_err(into_vfs_err)?;
            if inode_info.is_none() {
                return Err(VfsError::NotFound);
            }
            let (ino, inode) = inode_info.unwrap();
            match (inode.is_dir(), is_dir) {
                (true, false) => return Err(VfsError::IsADirectory),
                (false, true) => return Err(VfsError::NotADirectory),
                _ => {}
            }
            let mut deferred_zero_link: Option<InodeNumber> = None;
            if inode.is_dir() {
                let mut dir_inode = inode; // Ext4Inode is Copy
                if !rsext4::is_dir_empty(fs, dev, &mut dir_inode).map_err(into_vfs_err)? {
                    return Err(VfsError::DirectoryNotEmpty);
                }
                rsext4::delete_dir(fs, dev, &path).map_err(into_vfs_err)?;
            } else if inode.i_links_count > 1 {
                // Multiple hard links remain after this one is removed.
                rsext4::unlink(fs, dev, &path).map_err(into_vfs_err)?;
            } else {
                // Last (or only) link.  Mark the inode zero-link,
                // remove the directory entry, and defer the
                // zero-link / free_inode check until after the
                // fs/dev split borrow ends.
                fs.modify_inode(dev, ino, |on_disk| {
                    on_disk.i_links_count = 0;
                })
                .map_err(into_vfs_err)?;
                rsext4::remove_inodeentry_from_parentdir(fs, dev, &dir_path, name)
                    .map_err(into_vfs_err)?;
                deferred_zero_link = Some(ino);
            }
            // Capture the inode number for page-cache key cleanup.
            // This must be set before the fs/dev borrow ends because
            // deferred_zero_link is bound inside this scope.
            forget_file_ino.set(deferred_zero_link);
            // fs/dev borrow ends here (last use in all branches).
            // `state` is accessible again.
            if let Some(ino) = deferred_zero_link
                && state.mark_zero_link(ino)
            {
                // No live Inode Arcs — free immediately.
                let (fs, dev) = state.split();
                if let Ok(mut on_disk) = fs.get_inode_by_num(dev, ino) {
                    let _ = rsext4::free_inode(fs, dev, ino, &mut on_disk);
                }
            }
        }
        if let Some(ino) = forget_file_ino.get() {
            forget_cached_file_key(&*self.fs, ino.as_u64());
        }
        self.fs.sync_to_disk()
    }

    fn rename(&self, src_name: &str, dst_dir: &DirNode, dst_name: &str) -> VfsResult<()> {
        let dst_dir: Arc<Self> = dst_dir.downcast().map_err(|_| VfsError::InvalidInput)?;
        if !Arc::ptr_eq(&self.fs, &dst_dir.fs) {
            return Err(VfsError::CrossesDevices);
        }
        let src_path = join_child_path(&self.dir_path()?, src_name);
        let dst_path = join_child_path(&dst_dir.dir_path()?, dst_name);
        let replaced_file_ino = loop {
            let target = RenameTarget::lookup(&mut self.fs.lock(), &dst_path)?;
            // Never wait for inode I/O while holding the metadata mutex: a
            // reader needs that mutex again to finish its access-time update.
            let _io = target.io_lock.as_ref().map(|lock| lock.lock());
            let mut state = self.fs.lock();
            let current = RenameTarget::lookup(&mut state, &dst_path)?;
            if !target.same_identity(&current) {
                continue;
            }
            let (fs, dev) = state.split();
            rsext4::rename(dev, fs, &src_path, &dst_path).map_err(into_vfs_err)?;
            break current.replaced_file;
        };
        if let Some(ino) = replaced_file_ino {
            forget_cached_file_key(&*self.fs, ino.as_u64());
        }
        self.fs.sync_to_disk()
    }
}

struct RenameTarget {
    number: Option<InodeNumber>,
    io_lock: Option<Arc<SleepMutex<()>>>,
    replaced_file: Option<InodeNumber>,
}

impl RenameTarget {
    fn lookup(state: &mut Ext4State, path: &str) -> VfsResult<Self> {
        let (fs, dev) = state.split();
        let entry = rsext4::dir::get_inode_with_num(fs, dev, path).map_err(into_vfs_err)?;
        let number = entry.as_ref().map(|(number, _)| *number);
        let replaced_file = entry.and_then(|(number, inode)| {
            (!inode.is_dir() && inode.i_links_count <= 1).then_some(number)
        });
        let io_lock = number.and_then(|number| state.inode_io_lock(number));
        Ok(Self {
            number,
            io_lock,
            replaced_file,
        })
    }

    fn same_identity(&self, other: &Self) -> bool {
        self.number == other.number
            && match (&self.io_lock, &other.io_lock) {
                (Some(left), Some(right)) => Arc::ptr_eq(left, right),
                (None, None) => true,
                _ => false,
            }
    }
}
