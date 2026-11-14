use super::*;
use crate::confluence_reads::command::{ConfluenceGetArgs, PreparedInvocation};
use crate::planning_read::{
    DESIGN_GET, DocumentGetArgs, PLAN_GET, PageFormat, PreparedDocumentRead,
};
use crate::profiles::InvocationOptions;

impl LiveBindingHost<'_> {
    fn read_document<T: serde::de::DeserializeOwned>(
        &mut self,
        profile: &str,
        page: &str,
        command: &str,
    ) -> Result<DocumentSnapshot<T>, CraftError> {
        let policy = self.capture_cf(profile)?;
        self.check_all()?;
        self.reserve_open()?;
        let options = InvocationOptions {
            profile: Some(profile),
            state_root: Some(self.store.path()),
            request_id: &self.request_id,
            max_bytes: 8_388_608,
        };
        let args = DocumentGetArgs {
            page: ConfluenceGetArgs {
                cloud_id: Some(policy.target.tenant.clone()),
                id: page.into(),
            },
            format: PageFormat::Storage,
        };
        let registry = crate::catalog::registry()?;
        let document = PreparedDocumentRead::prepare(
            &registry,
            command,
            args,
            &["document".into()],
            &options,
        )?;
        let native_options = crate::planning_read::native_options(&options);
        let invocation = PreparedInvocation::prepare(
            document.native_args(),
            &document.native_fields(),
            &native_options,
            self.store,
            &self.control,
        )?;
        let locked = self.credentials.lock(invocation.grant(), &self.control)?;
        let native = invocation.execute_cached_with_check(
            self.store,
            self.client,
            &locked,
            self.tls.clone(),
            self.control.clone(),
            || {
                self.check_review()?;
                self.store.verify_confluence_read_policy(
                    &policy,
                    CfOperation::PageGet,
                    &self.control,
                )
            },
        );
        if let Some(cache) = native
            .envelope
            .data
            .as_ref()
            .and_then(|v| v.get("credential_cache_update"))
            .filter(|v| !v.is_null())
        {
            self.receipts.push(
                serde_json::from_value(cache.clone())
                    .map_err(|_| invalid("invalid_native_cache_receipt"))?,
            );
        }
        let body = native
            .envelope
            .data
            .as_ref()
            .and_then(|v| v.pointer("/item/body_storage"))
            .and_then(Value::as_str)
            .map(String::from);
        let result = document.finish(native, &options, &self.control);
        if !result.envelope.ok {
            return Err(result
                .envelope
                .error
                .ok_or_else(|| invalid("missing_document_read_failure"))?);
        }
        let data = result
            .envelope
            .data
            .ok_or_else(|| invalid("missing_native_document_data"))?;
        let snapshot = DocumentSnapshot {
            source: serde_json::from_value(data["source"].clone())
                .map_err(|_| invalid("invalid_native_source"))?,
            body_storage: body.ok_or_else(|| invalid("missing_source_body"))?,
            document: serde_json::from_value(data["document"].clone())
                .map_err(|_| invalid("invalid_imported_document"))?,
        };
        self.check_all()?;
        Ok(snapshot)
    }
    pub(super) fn read_plan(
        &mut self,
        profile: &str,
        page: &str,
    ) -> Result<DocumentSnapshot<Plan>, CraftError> {
        self.read_document(profile, page, PLAN_GET)
    }
    pub(super) fn read_design(
        &mut self,
        profile: &str,
        page: &str,
    ) -> Result<DocumentSnapshot<Design>, CraftError> {
        self.read_document(profile, page, DESIGN_GET)
    }
}
