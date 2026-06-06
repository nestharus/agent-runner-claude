// declared_role: orchestration
// intrinsic_surface_declarations:
//   - component: tests/support/mod.rs
//     role: intrinsic-surface
//     Domain: contract_test_support_module_index
//     Owns:
//       - contract and characterization support module declaration set
//       - shared test harness support namespace

#![allow(dead_code)]

pub mod assertions;
pub mod fixtures;
pub mod invoke;
pub mod requests;
pub mod schema;
pub mod scripts;
