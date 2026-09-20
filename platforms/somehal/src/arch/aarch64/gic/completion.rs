/// Controller operations surrounding one GICv3 action dispatch.
///
/// Split EOI drops priority before dispatch and deactivates ordinary
/// interrupts afterwards. LPIs do not have an active state, so issuing DIR for
/// them is both unnecessary and incompatible with the GICv3 completion model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CompletionPlan {
    pub(crate) drop_priority_before_dispatch: bool,
    pub(crate) deactivate_after_dispatch: bool,
}

pub(crate) const fn completion_plan(two_step_eoi: bool, is_lpi: bool) -> CompletionPlan {
    CompletionPlan {
        drop_priority_before_dispatch: two_step_eoi,
        deactivate_after_dispatch: two_step_eoi && !is_lpi,
    }
}
