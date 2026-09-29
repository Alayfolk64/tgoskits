//! Mutation contexts retain backend owners until the caller retires translations.

use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use ax_memory_addr::{VirtAddr, VirtAddrRange};
use ax_memory_set::{MappingBackend, MappingError, MemoryArea, MemorySet};

#[derive(Default)]
struct RetirementContext {
    owners: Vec<Rc<()>>,
    fail_at: Option<usize>,
    detached: Vec<(usize, usize)>,
}

#[derive(Clone)]
struct ObservedBackend {
    anchor: Rc<()>,
    retired: Rc<RefCell<Vec<(usize, usize)>>>,
}

impl MappingBackend for ObservedBackend {
    type Addr = VirtAddr;
    type Flags = ();
    type PageTable = ();
    type MutationContext = RetirementContext;

    fn map(&self, _: VirtAddr, _: usize, _: (), _: &mut RetirementContext, _: &mut ()) -> bool {
        true
    }
    fn unmap(
        &self,
        start: VirtAddr,
        size: usize,
        context: &mut RetirementContext,
        _: &mut (),
    ) -> bool {
        if context.fail_at == Some(start.as_usize()) {
            return false;
        }
        context.owners.push(self.anchor.clone());
        context.detached.push((start.into(), size));
        self.retired.borrow_mut().push((start.into(), size));
        true
    }
    fn protect(&self, _: VirtAddr, _: usize, _: (), _: &mut RetirementContext, _: &mut ()) -> bool {
        true
    }
    fn split(&mut self, _: usize) -> Option<Self> {
        Some(self.clone())
    }
    fn shrink_left(&mut self, _: usize) -> bool {
        true
    }
    fn shrink_right(&mut self, _: usize) -> bool {
        true
    }
}

fn mappings() -> (MemorySet<ObservedBackend>, Vec<Weak<()>>) {
    let mut set = MemorySet::new();
    let mut owners = Vec::new();
    for start in [0, 0x2000] {
        let anchor = Rc::new(());
        owners.push(Rc::downgrade(&anchor));
        set.map(
            MemoryArea::new(
                start.into(),
                0x1000,
                (),
                ObservedBackend {
                    anchor,
                    retired: Rc::new(RefCell::new(Vec::new())),
                },
            ),
            &mut RetirementContext::default(),
            &mut (),
            false,
        )
        .unwrap();
    }
    (set, owners)
}

#[test]
fn completed_unmap_keeps_backing_owners_in_the_retirement_context() {
    let (mut set, owners) = mappings();
    let mut retirement = RetirementContext::default();
    set.unmap(0.into(), 0x3000, &mut retirement, &mut ())
        .unwrap();
    assert!(set.is_empty());
    assert_eq!(retirement.detached, [(0, 0x1000), (0x2000, 0x1000)]);
    assert!(owners.iter().all(|owner| owner.upgrade().is_some()));
    drop(retirement);
    assert!(owners.iter().all(|owner| owner.upgrade().is_none()));
}

#[test]
fn partial_failure_retains_metadata_and_already_detached_owners() {
    let (mut set, owners) = mappings();
    let mut retirement = RetirementContext {
        fail_at: Some(0x2000),
        ..Default::default()
    };
    assert_eq!(
        set.unmap(0.into(), 0x3000, &mut retirement, &mut ()),
        Err(MappingError::BadState)
    );
    assert_eq!(
        set.iter().map(|area| area.va_range()).collect::<Vec<_>>(),
        [
            VirtAddrRange::new(0.into(), 0x1000.into()),
            VirtAddrRange::new(0x2000.into(), 0x3000.into()),
        ]
    );
    assert_eq!(retirement.detached, [(0, 0x1000)]);
    assert!(owners.iter().all(|owner| owner.upgrade().is_some()));
    let before = retirement.detached.clone();
    set.unmap(0.into(), 0, &mut retirement, &mut ()).unwrap();
    assert_eq!(
        set.unmap(usize::MAX.into(), 1, &mut retirement, &mut ()),
        Err(MappingError::InvalidParam)
    );
    assert_eq!(retirement.detached, before);
}
