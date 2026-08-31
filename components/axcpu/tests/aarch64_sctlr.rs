#[path = "../src/aarch64/sctlr.rs"]
mod sctlr;

#[test]
fn inherited_alignment_check_is_cleared() {
    const ALIGNMENT_CHECK: u64 = 1 << 1;
    const SPAN: u64 = 1 << 23;

    assert_eq!(sctlr::prepare_el1(ALIGNMENT_CHECK), SPAN);
}
