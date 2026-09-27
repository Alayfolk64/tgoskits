use core::io::BorrowedBuf;

use super::*;

#[test]
fn full_page_pin_is_revalidated_and_partial_eof_is_not_pinned() {
    with_test_page_provider(true, |_| {
        let backing = Arc::new(CacheTestFile::new(vec![0x41; PAGE_SIZE + 9]));
        let cached = reopen_cached_file(backing);
        let pin = cached.pin_read_page(0).unwrap().unwrap();
        assert!(cached.pin_read_page(1).unwrap().is_none());
        assert_eq!(
            cached.with_current_read_page(0, &pin, || Ok(42)),
            Ok(Some(42))
        );
        cached.set_len(0).unwrap();
        assert_eq!(
            cached.with_current_read_page(0, &pin, || panic!("stale pin published")),
            Ok(None::<()>),
        );
        let mut bytes = [0; PAGE_SIZE];
        pin.copy_to(BorrowedBuf::from(&mut bytes[..]).unfilled())
            .unwrap();
        assert_eq!(bytes, [0x41; PAGE_SIZE]);
    });
}

#[test]
fn replacement_page_cannot_publish_an_old_physical_pin() {
    with_test_page_provider(true, |_| {
        let backing = Arc::new(CacheTestFile::new(vec![0x41; PAGE_SIZE]));
        let cached = reopen_cached_file(backing);
        let old = cached.pin_read_page(0).unwrap().unwrap();
        cached.set_len(0).unwrap();
        cached.write_at(&[0x72; PAGE_SIZE][..], 0).unwrap();
        let new = cached.pin_read_page(0).unwrap().unwrap();
        assert_ne!(old.paddr().unwrap(), new.paddr().unwrap());
        assert_eq!(
            cached.with_current_read_page(0, &old, || panic!("replaced pin published")),
            Ok(None::<()>),
        );
        assert_eq!(
            cached.with_current_read_page(0, &new, || Ok(())),
            Ok(Some(()))
        );
        cached.sync(false).unwrap();
    });
}
