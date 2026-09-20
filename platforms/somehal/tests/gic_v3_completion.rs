#[path = "../src/arch/aarch64/gic/completion.rs"]
mod completion;

use completion::{CompletionPlan, completion_plan};

#[test]
fn split_eoi_does_not_deactivate_lpis() {
    assert_eq!(
        completion_plan(true, true),
        CompletionPlan {
            drop_priority_before_dispatch: true,
            deactivate_after_dispatch: false,
        }
    );
}

#[test]
fn split_eoi_deactivates_non_lpis_after_dispatch() {
    assert_eq!(
        completion_plan(true, false),
        CompletionPlan {
            drop_priority_before_dispatch: true,
            deactivate_after_dispatch: true,
        }
    );
}

#[test]
fn combined_eoi_completes_at_the_end_of_dispatch() {
    assert_eq!(
        completion_plan(false, false),
        CompletionPlan {
            drop_priority_before_dispatch: false,
            deactivate_after_dispatch: false,
        }
    );
}
