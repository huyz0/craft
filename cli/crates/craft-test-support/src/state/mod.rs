mod artifacts;
mod clock;
mod faults;
mod journal;
mod vault;

pub use artifacts::MemoryArtifacts;
pub use clock::ManualClock;
pub use faults::{FaultPoint, Faults};
pub use journal::MemoryJournal;
pub use vault::{MemoryVault, VaultMode};

/// Names a fixture root without filesystem effects. The trusted prefix is one filename component.
/// A process-local sequence keeps equal wall-clock samples from sharing private state.
pub fn fixture_root(parent: &std::path::Path, prefix: &str, stamp: u128) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        "{prefix}-{}-{stamp}-{sequence}",
        std::process::id()
    ))
}
