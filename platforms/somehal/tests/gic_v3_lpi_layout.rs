#[path = "../src/arch/aarch64/gic/lpi_layout.rs"]
mod lpi_layout;

use lpi_layout::pending_table_stride;

#[test]
fn each_redistributor_gets_a_distinct_64k_aligned_pending_table() {
    assert_eq!(pending_table_stride(1 << 13), 1 << 16);
    assert_eq!(pending_table_stride(1 << 16), 1 << 16);
    assert_eq!(pending_table_stride((1 << 16) + 1), 1 << 17);
}
