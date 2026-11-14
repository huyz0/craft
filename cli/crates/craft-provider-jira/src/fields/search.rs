use super::*;

fn offset(value: &str) -> Result<u64, CraftError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|number| *number <= i64::MAX as u64 && number.to_string() == value)
        .ok_or_else(malformed)
}
fn optional<'a>(body: &'a Value, key: &str) -> Result<Option<&'a Value>, CraftError> {
    match body.get(key) {
        Some(Value::Null) => Err(malformed()),
        value => Ok(value),
    }
}
pub(super) fn read(
    operation: &PreparedOperation,
    context: &mut dyn OperationContext,
    plan: &FieldPlan,
    position: Option<&NativePosition>,
) -> Result<NativePage, CraftError> {
    let input: FieldSearchInput =
        serde_json::from_value(operation.request().input.clone()).map_err(|_| malformed())?;
    input.validate()?;
    let start = match position {
        None => 0,
        Some(position) => {
            position.validate()?;
            if position.0.len() != 1 {
                return Err(CraftError::field(
                    "/cursor",
                    "unexpected_field_search_continuation",
                ));
            }
            offset(position.0.get("startAt").ok_or_else(malformed)?)?
        }
    };
    let mut query = BTreeMap::from([
        ("startAt".into(), start.to_string()),
        ("maxResults".into(), input.page_size.to_string()),
        (
            "orderBy".into(),
            serde_json::to_value(input.order_by)
                .map_err(|_| malformed())?
                .as_str()
                .ok_or_else(malformed)?
                .into(),
        ),
    ]);
    if let Some(query_text) = input.query {
        query.insert("query".into(), query_text);
    }
    let expansions = plan.expansions();
    if !expansions.is_empty() {
        query.insert("expand".into(), expansions.join(","));
    }
    let mut repeated_query = BTreeMap::new();
    if !input.ids.is_empty() {
        repeated_query.insert("id".into(), input.ids);
    }
    if !input.types.is_empty() {
        repeated_query.insert(
            "type".into(),
            input
                .types
                .iter()
                .map(|kind| match kind {
                    FieldType::Custom => "custom".into(),
                    FieldType::System => "system".into(),
                })
                .collect(),
        );
    }
    if !input.project_ids.is_empty() {
        repeated_query.insert(
            "projectIds".into(),
            input.project_ids.iter().map(u64::to_string).collect(),
        );
    }
    let call = NativeCall {
        route: SEARCH_ROUTE.into(),
        path_parameters: BTreeMap::new(),
        query,
        repeated_query,
        body: None,
    };
    let body = invoke(context, &call)?;
    let returned_start = body
        .get("startAt")
        .and_then(Value::as_u64)
        .ok_or_else(malformed)?;
    let maximum = body
        .get("maxResults")
        .and_then(Value::as_u64)
        .filter(|maximum| *maximum > 0 && *maximum <= u64::from(input.page_size))
        .ok_or_else(malformed)?;
    let values = body
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(malformed)?;
    if returned_start != start || values.len() as u64 > maximum {
        return Err(malformed());
    }
    if let Some(total) = optional(&body, "total")?
        && total.as_u64().is_none_or(|value| value > i64::MAX as u64)
    {
        return Err(malformed());
    }
    let last = optional(&body, "isLast")?
        .map(|value| value.as_bool().ok_or_else(malformed))
        .transpose()?;
    let link = optional(&body, "nextPage")?
        .map(|value| {
            value
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(malformed)
        })
        .transpose()?;
    if (last == Some(true) && link.is_some()) || (last.is_none() && link.is_none()) {
        return Err(malformed());
    }
    let next = if last == Some(false) && link.is_none() {
        // Live field search omits `nextPage` (and `self`) on non-final pages, unlike
        // Jira's other offset-paged endpoints. An explicit `isLast: false` is the
        // only continuation signal, so advance by the records actually returned.
        // An empty non-final page cannot advance and is refused as stalled.
        if values.is_empty() {
            return Err(CraftError::new(
                ErrorCode::PaginationStalled,
                "Field search reported more results but returned none.",
            ));
        }
        let next = start
            .checked_add(values.len() as u64)
            .filter(|next| *next <= i64::MAX as u64)
            .ok_or_else(malformed)?;
        Some(NativePosition(BTreeMap::from([(
            "startAt".into(),
            next.to_string(),
        )])))
    } else if let Some(link) = link {
        let mut link_call = call.clone();
        link_call
            .query
            .insert("maxResults".into(), maximum.to_string());
        let normalized = context.normalize_next(&link_call, link)?;
        if normalized.route != call.route
            || normalized.path_parameters != call.path_parameters
            || normalized.repeated_query != call.repeated_query
            || normalized.body.is_some()
        {
            return Err(malformed());
        }
        let mut stable = normalized.query;
        let next = offset(stable.remove("startAt").as_deref().ok_or_else(malformed)?)?;
        let mut expected = link_call.query;
        expected.remove("startAt");
        if stable != expected || next <= start {
            return Err(CraftError::new(
                ErrorCode::PaginationStalled,
                "Field continuation did not preserve filters and advance its offset.",
            ));
        }
        Some(NativePosition(BTreeMap::from([(
            "startAt".into(),
            next.to_string(),
        )])))
    } else {
        None
    };
    let items = normalize(values, plan, context)?;
    if let Some(ids) = call.repeated_query.get("id")
        && items.iter().any(|item| {
            item["id"]
                .as_str()
                .is_none_or(|id| !ids.iter().any(|expected| expected == id))
        })
    {
        return Err(malformed());
    }
    Ok(NativePage {
        context: None,
        items,
        next,
    })
}
