// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Shared pagination and limit/offset helpers for glob and grep tools

/// Default maximum number of matches returned by `glob` or `grep` in a single call.
pub const MAX_SEARCH_RESULTS: usize = 200;

/// Hard upper bound for caller-supplied `limit` parameters on `glob` and `grep`.
pub const MAX_CALLER_LIMIT: usize = 1_000;

pub(crate) fn parse_limit_and_offset(arguments: &serde_json::Value) -> (usize, usize) {
    let limit = arguments
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .map_or(MAX_SEARCH_RESULTS, |n| n.clamp(1, MAX_CALLER_LIMIT));
    let offset = arguments
        .get("offset")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0);
    (limit, offset)
}

pub(crate) fn format_paginated_lines(
    items: Vec<String>,
    offset: usize,
    limit: usize,
    noun: &str,
) -> String {
    let total = items.len();
    if total == 0 || offset >= total {
        return String::new();
    }
    let end = (offset.saturating_add(limit)).min(total);
    let mut out = items[offset..end].join("\n");
    if end < total || offset > 0 {
        let shown_start = offset + 1;
        if end < total {
            out.push_str(&format!(
                "\n\n[showing {shown_start}-{end} of {total} {noun}; pass offset={end} to see more]"
            ));
        } else {
            out.push_str(&format!(
                "\n\n[showing {shown_start}-{end} of {total} {noun}]"
            ));
        }
    }
    out
}
