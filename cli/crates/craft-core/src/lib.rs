//! Provider-independent public contracts for the Craft cloud CLI.

pub mod contracts;
pub mod control;
pub mod error;
pub mod execution;
pub mod input;
pub mod metadata;
pub mod pagination;
pub mod presence;
pub mod profiles;
pub mod projection;
pub mod registry;
pub mod storage;

/// Identifies this distribution independently of other executables named Craft.
pub const PRODUCT: &str = "Craft AI Factory CLI";

/// Workspace release version, shared by the CLI and its public contracts.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
