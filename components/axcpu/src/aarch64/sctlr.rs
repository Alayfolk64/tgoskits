const ALIGNMENT_CHECK: u64 = 1 << 1;
const SPAN: u64 = 1 << 23;

/// Applies the Linux-compatible EL1 alignment and privileged-access policy.
pub(crate) const fn prepare_el1(value: u64) -> u64 {
    (value & !ALIGNMENT_CHECK) | SPAN
}
