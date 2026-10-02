//! `resource_reads` cases (the resource-side read surface, RSRC-03/04/05/06,
//! Suite-Extension binding module 11).
//!
//! Content lands in plan 02-05. This stub keeps the frozen `parity_suite!`
//! compiling: `parity_resource_reads_cases!` is a documented empty expansion
//! point until 02-05 fills it (D-14 architecture-complete).

/// Expand the `resource_reads` cases for one backend (empty until 02-05).
#[macro_export]
macro_rules! parity_resource_reads_cases {
    ($backend:ty) => {};
}
