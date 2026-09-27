extern crate std;

use std::{vec, vec::Vec};

use super::{Gic, LPI, RedistributorV3, RedistributorV4, SGI, VirtAddr};

#[test]
fn size_lpi() {
    assert_eq!(size_of::<LPI>(), 0x10000);
}

#[test]
fn size_sgi() {
    assert_eq!(size_of::<SGI>(), 0x10000);
}

#[test]
fn test_v3_rd() {
    assert_eq!(size_of::<RedistributorV3>(), 0x20000);
}

#[test]
fn test_v4_rd() {
    assert_eq!(size_of::<RedistributorV4>(), 0x40000);
}

#[test]
fn lpi_tables_reject_sub_64k_stride_before_register_writes() {
    let mut registers = LpiRegisters::new(8);
    let before = registers.redistributors.clone();
    let result = registers
        .gic()
        .init_lpi_tables(0x100000, 16, 0x200000, 8192);
    assert!(
        result.is_err(),
        "8 KiB stride aliases all eight pending tables"
    );
    assert_eq!(registers.redistributors, before);
}

#[test]
fn lpi_tables_reject_unaligned_pending_base_before_register_writes() {
    let mut registers = LpiRegisters::new(2);
    let before = registers.redistributors.clone();
    let result = registers
        .gic()
        .init_lpi_tables(0x100000, 16, 0x202000, 65536);
    assert!(
        result.is_err(),
        "PENDBASER silently discards address bits 15:0"
    );
    assert_eq!(registers.redistributors, before);
}

#[test]
fn lpi_tables_program_distinct_pending_addresses_for_eight_cpus() {
    let mut registers = LpiRegisters::new(8);
    registers
        .gic()
        .init_lpi_tables(0x100000, 16, 0x200000, 65536)
        .unwrap();
    for cpu in 0..8 {
        let base = cpu * LpiRegisters::WORDS_PER_REDISTRIBUTOR;
        let pending = registers.redistributors[base + 0x78 / 8];
        assert_eq!(
            pending & 0x000f_ffff_ffff_0000,
            0x200000 + cpu as u64 * 65536
        );
        assert_eq!(registers.redistributors[base] & 1, 1);
    }
}

#[test]
fn lpi_tables_reject_unrepresentable_layouts_before_register_writes() {
    let invalid_layouts = [
        (0x200000, 0),
        (0x200000, !0xffff),
        (!0xffff, 65536),
        ((1 << 52) - 65536, 65536),
    ];
    for (pending_base, pending_stride) in invalid_layouts {
        let mut registers = LpiRegisters::new(8);
        let before = registers.redistributors.clone();
        let result = registers
            .gic()
            .init_lpi_tables(0x100000, 16, pending_base, pending_stride);
        assert!(
            result.is_err(),
            "invalid layout was accepted: {pending_base:#x}, {pending_stride:#x}"
        );
        assert_eq!(registers.redistributors, before);
    }
}

struct LpiRegisters {
    distributor: Vec<u64>,
    redistributors: Vec<u64>,
}

impl LpiRegisters {
    const WORDS_PER_REDISTRIBUTOR: usize = 0x20000 / 8;

    fn new(cpus: usize) -> Self {
        let mut redistributors = vec![0; cpus * Self::WORDS_PER_REDISTRIBUTOR];
        redistributors[(cpus - 1) * Self::WORDS_PER_REDISTRIBUTOR + 1] = 1 << 4;
        Self {
            distributor: vec![0; 0x10000 / 8],
            redistributors,
        }
    }

    fn gic(&mut self) -> Gic {
        // SAFETY: these aligned, zeroed register fixtures outlive the Gic and
        // cover every accessed register. The last redistributor terminates
        // enumeration; each test is the sole register owner.
        unsafe {
            Gic::new(
                VirtAddr::from(self.distributor.as_mut_ptr().cast::<u8>()),
                VirtAddr::from(self.redistributors.as_mut_ptr().cast::<u8>()),
            )
        }
    }
}
