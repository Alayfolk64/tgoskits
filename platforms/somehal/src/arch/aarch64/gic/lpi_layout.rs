// GICR_PENDBASER encodes the physical base starting at bit 16. Keeping each
// Redistributor's table on its own 64 KiB boundary prevents adjacent CPUs from
// aliasing the same hardware-visible pending bits.
const PENDING_TABLE_ALIGNMENT: usize = 64 * 1024;

/// Returns the byte stride between per-CPU GICv3 LPI pending tables.
pub(crate) const fn pending_table_stride(pending_bytes: usize) -> usize {
    align_up(pending_bytes, PENDING_TABLE_ALIGNMENT)
}

const fn align_up(value: usize, align: usize) -> usize {
    (value + align - 1) & !(align - 1)
}
