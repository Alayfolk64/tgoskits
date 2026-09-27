use super::*;

#[cfg(all(feature = "host-test", target_os = "linux"))]
#[test]
fn child_lookup_does_not_reread_root_directory() {
    use crate::fs::ext4::rsext4::metadata_tests::Fixture;

    let fixture = Fixture::new("parent-inode-lookup");
    let filesystem = fixture.mount();
    let root = filesystem.root_dir();
    let parent = root
        .as_dir()
        .unwrap()
        .create(
            "parent",
            NodeType::Directory,
            NodePermission::default(),
            0,
            0,
        )
        .unwrap();
    let child = parent
        .as_dir()
        .unwrap()
        .create(
            "child",
            NodeType::RegularFile,
            NodePermission::default(),
            0,
            0,
        )
        .unwrap();
    root.sync(false).unwrap();

    let root_inode = root.as_dir().unwrap().downcast::<Inode>().unwrap();
    let root_block = {
        let mut state = root_inode.fs.lock();
        let (fs, dev) = state.split();
        let mut inode = fs.get_inode_by_num(dev, root_inode.ino).unwrap();
        let block = rsext4::loopfile::resolve_inode_block(dev, &mut inode, 0)
            .unwrap()
            .unwrap();
        // The image has been synced. Evict only this clean ancestor block
        // so a redundant root-to-parent walk reaches the injected failure.
        fs.datablock_cache.invalidate(block);
        block
    };
    fixture.fail_read_at(Some(root_block.raw() * (BLOCK_SIZE / 512) as u64));

    // Invoke the actual ext4 backend, bypassing the VFS dentry cache populated
    // by create. The already selected parent is a real, live directory.
    let result = parent.as_dir().unwrap().inner().lookup("child");

    fixture.fail_read_at(None);
    assert_eq!(result.unwrap().inode(), child.inode());
}

/// Round-trip: encode a DeviceId to i_block, then decode it back.
fn roundtrip(major: u32, minor: u32) -> (u32, u32) {
    let dev = DeviceId::new(major, minor);
    let (b0, b1) = encode_ext4_rdev(dev);
    let mut iblock = [0u32; 15];
    iblock[0] = b0;
    iblock[1] = b1;
    let back = decode_ext4_rdev(&iblock);
    (back.major(), back.minor())
}

#[test]
fn rdev_old_format_small() {
    // (1, 3) — console, old format
    assert_eq!(roundtrip(1, 3), (1, 3));
}

#[test]
fn rdev_old_format_boundary() {
    // Maximum old-format values
    assert_eq!(roundtrip(255, 255), (255, 255));
    assert_eq!(roundtrip(0, 0), (0, 0));
}

#[test]
fn rdev_new_format_minor_exceeds_old() {
    // minor = 256 triggers new format
    assert_eq!(roundtrip(1, 256), (1, 256));
}

#[test]
fn rdev_new_format_large_minor() {
    assert_eq!(roundtrip(1, 1040), (1, 1040));
    assert_eq!(roundtrip(8, 511), (8, 511));
}

#[test]
fn rdev_old_on_disk_decodes_correctly() {
    // Simulate what Linux writes for (1,3) in old format:
    // i_block[0] = (1 << 8) | 3 = 259
    let mut iblock = [0u32; 15];
    iblock[0] = (1 << EXT4_OLD_MINOR_BITS) | 3;
    let dev = decode_ext4_rdev(&iblock);
    assert_eq!(dev.major(), 1);
    assert_eq!(dev.minor(), 3);
}

#[test]
fn rdev_new_on_disk_decodes_correctly() {
    // Simulate what Linux writes for (1, 256) in new format:
    // new_encode_dev: (256 & 0xFF) | (1 << 8) | ((256 & 0xFFF00) << 12)
    // = 0 | 0x100 | (0x100 << 12) = 0x100 | 0x100000 = 0x100100
    let encoded = (256u32 & EXT4_NEW_MINOR_LOW_MASK)
        | ((1u32 & EXT4_NEW_MAJOR_MASK) << EXT4_OLD_MINOR_BITS)
        | ((256u32 & EXT4_NEW_MINOR_HIGH_MASK) << EXT4_NEW_MAJOR_BITS);
    let mut iblock = [0u32; 15];
    iblock[0] = 0;
    iblock[1] = encoded;
    let dev = decode_ext4_rdev(&iblock);
    assert_eq!(dev.major(), 1);
    assert_eq!(dev.minor(), 256);
}
