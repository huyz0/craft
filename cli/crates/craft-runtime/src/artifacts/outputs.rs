use super::*;
use craft_platform::{OutputCommitError, OutputRoot, OverwritePolicy};

impl NativeArtifacts {
    /// Exports complete verified bytes only to an explicitly selected output authority.
    /// The default create-new policy and explicit replacement are separate caller choices.
    /// Once publication is attempted, uncertain effects must be inspected before retry.
    pub fn export_file(
        &self,
        name: &str,
        output_root: &OutputRoot,
        destination: &Path,
        policy: OverwritePolicy,
    ) -> Result<ArtifactReceipt, StoreError> {
        self.export_impl(name, output_root, destination, policy, None)
    }

    /// Binds publication to an observed digest on the same opened artifact being copied.
    pub fn export_file_checked(
        &self,
        name: &str,
        output_root: &OutputRoot,
        destination: &Path,
        policy: OverwritePolicy,
        expected_digest: &str,
    ) -> Result<ArtifactReceipt, StoreError> {
        if !expected_digest
            .strip_prefix("sha256:")
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        {
            return Err(StoreError::Invalid);
        }
        self.export_impl(
            name,
            output_root,
            destination,
            policy,
            Some(expected_digest),
        )
    }

    fn export_impl(
        &self,
        name: &str,
        output_root: &OutputRoot,
        destination: &Path,
        policy: OverwritePolicy,
        expected_digest: Option<&str>,
    ) -> Result<ArtifactReceipt, StoreError> {
        check(&self.control)?;
        let (mut receipt, mut source) = self.open_artifact(name)?.ok_or(StoreError::Invalid)?;
        if expected_digest.is_some_and(|expected| expected != receipt.digest) {
            return Err(StoreError::Conflict);
        }
        let mut staged = output_root
            .begin_excluding(
                destination,
                &random_id("export_").map_err(|_| StoreError::Unavailable)?,
                receipt.bytes,
                policy,
                self.root.as_ref(),
            )
            .map_err(|error| match error.kind() {
                io::ErrorKind::InvalidInput | io::ErrorKind::PermissionDenied => {
                    StoreError::Invalid
                }
                _ => io_error(error),
            })?;
        let path = staged
            .destination()
            .to_str()
            .ok_or(StoreError::Invalid)?
            .to_owned();
        self.verify(&receipt, &mut source, Some(&mut staged))?;
        staged.prepare().map_err(io_error)?;
        #[cfg(test)]
        if let Some(token) = self
            .cancel_before_export
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .take()
        {
            token.cancel();
        }
        check(&self.control)?;
        #[cfg(test)]
        if self.fault.load(std::sync::atomic::Ordering::SeqCst) == 3 {
            return Err(StoreError::Unavailable);
        }
        staged.commit().map_err(|error| match error {
            OutputCommitError::NotPublished(error) => io_error(error),
            OutputCommitError::OutcomeUnknown => StoreError::OutcomeUnknown,
        })?;
        #[cfg(test)]
        if self.fault.load(std::sync::atomic::Ordering::SeqCst) == 4 {
            return Err(StoreError::OutcomeUnknown);
        }
        #[cfg(test)]
        if let Some(token) = self
            .cancel_after_publish
            .lock()
            .map_err(|_| StoreError::OutcomeUnknown)?
            .take()
        {
            token.cancel();
        }
        receipt.local_path = Some(path);
        // A confirmed local rename must retain its receipt despite later cancellation.
        Ok(receipt)
    }
}
