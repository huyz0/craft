//! Host-owned execution infrastructure behind provider-independent core ports.

#[cfg(all(feature = "synthetic-acceptance", not(debug_assertions)))]
compile_error!("synthetic-acceptance requires debug assertions and cannot be distributed");

#[cfg(feature = "synthetic-acceptance")]
pub mod synthetic_acceptance;

pub mod artifacts;
pub mod auth;
pub mod confluence_admin_policy;
pub mod confluence_read_policy;
pub mod confluence_write_policy;
pub mod control;
pub mod create_policy;
pub mod field_read_policy;
pub mod inputs;
pub mod issue_mutation_policy;
pub mod issue_read_policy;
pub mod issue_recovery_policy;
pub mod journal;
pub mod pagination;
pub mod profile_pages;
pub mod profiles;
pub mod project_read_policy;
pub mod provider_policy;
pub mod review_policy;
pub mod scan;
pub mod secrets;
pub mod transport;
pub mod vault;

#[cfg(test)]
#[path = "transport/tls_fixture.rs"]
mod tls_fixture;
