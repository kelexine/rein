// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Antigravity model catalog parsing, AI Code experience mapping, and effort tier expansion

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::provider::AvailableModel;

use super::AntigravityProvider;

pub(super) type EffortWireMap = HashMap<String, (String, String)>;
pub(super) type ExperienceMap = HashMap<String, String>;

pub(super) struct AicodeConfigParse {
    pub models: Vec<AvailableModel>,
    pub wire_map: EffortWireMap,
    pub aicode_ids: HashSet<String>,
    pub experiences: ExperienceMap,
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn parse_available_models(root: &Value) -> Vec<AvailableModel> {
    parse_available_models_with_wire(root).0
}

pub(super) fn parse_available_models_with_wire(
    root: &Value,
) -> (Vec<AvailableModel>, EffortWireMap) {
    let mut wire_map = EffortWireMap::new();
    let Some(models_map) = root.get("models").and_then(Value::as_object) else {
        return (Vec::new(), wire_map);
    };
    let recommended_ids = collect_recommended_ids(root);
    let mut out = Vec::new();
    for id in &recommended_ids {
        if let Some(entry) = models_map.get(id) {
            out.extend(expand_model_entry(id, entry, true, &mut wire_map));
        }
    }

    let mut extra_ids: Vec<&String> = models_map
        .keys()
        .filter(|id| !recommended_ids.iter().any(|r| r == *id))
        .collect();
    extra_ids.sort();
    for id in extra_ids {
        if is_noise_model_id(id) {
            continue;
        }
        if let Some(entry) = models_map.get(id) {
            out.extend(expand_model_entry(id, entry, false, &mut wire_map));
        }
    }
    (out, wire_map)
}

pub(super) fn parse_aicode_fetch_config(root: &Value) -> AicodeConfigParse {
    let mut wire_map = EffortWireMap::new();
    let mut out = Vec::new();
    let mut aicode_ids = HashSet::new();
    let mut experiences = ExperienceMap::new();
    let mut seen = HashSet::new();

    if let Some(arr) = root.get("availableModels").and_then(Value::as_array) {
        for entry in arr {
            push_aicode_model_entry(
                entry,
                false,
                &mut seen,
                &mut out,
                &mut wire_map,
                &mut aicode_ids,
                &mut experiences,
            );
        }
    }
    if let Some(exps) = root.get("availableExperiences").and_then(Value::as_array) {
        for exp in exps {
            push_aicode_experience_entry(
                exp,
                &mut seen,
                &mut out,
                &mut wire_map,
                &mut aicode_ids,
                &mut experiences,
            );
        }
    }
    AicodeConfigParse {
        models: out,
        wire_map,
        aicode_ids,
        experiences,
    }
}

pub(super) fn parse_aicode_entitlement(root: &Value) -> Option<Value> {
    let tier = root
        .get("licenses")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|lic| lic.get("userTier"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    Some(json!({ "userTier": tier }))
}

fn push_aicode_experience_entry(
    exp: &Value,
    seen: &mut HashSet<String>,
    out: &mut Vec<AvailableModel>,
    wire_map: &mut EffortWireMap,
    aicode_ids: &mut HashSet<String>,
    experiences: &mut ExperienceMap,
) {
    let raw_exp_name = exp
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(exp_name) = raw_exp_name {
        if is_noise_model_id(exp_name) {
            return;
        }
        aicode_ids.insert(exp_name.to_owned());
        experiences.insert(exp_name.to_owned(), exp_name.to_owned());
        let display_name = exp
            .get("displayName")
            .or_else(|| exp.pointer("/model/displayName"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map_or_else(|| humanize_model_stem(exp_name), str::to_owned);
        let quota_percent = exp
            .get("model")
            .and_then(parse_quota_percent)
            .or_else(|| parse_quota_percent(exp));
        if seen.insert(exp_name.to_owned()) {
            out.push(AvailableModel {
                id: exp_name.to_owned(),
                display_name,
                quota_percent,
                recommended: false,
            });
        }
        return;
    }

    if let Some(entry) = exp.get("model") {
        push_aicode_model_entry(entry, false, seen, out, wire_map, aicode_ids, experiences);
    }
}

fn push_aicode_model_entry(
    entry: &Value,
    recommended: bool,
    seen: &mut HashSet<String>,
    out: &mut Vec<AvailableModel>,
    wire_map: &mut EffortWireMap,
    aicode_ids: &mut HashSet<String>,
    experiences: &mut ExperienceMap,
) {
    let Some(id) = extract_aicode_model_id(entry) else {
        return;
    };
    if is_noise_model_id(&id) {
        return;
    }
    aicode_ids.insert(id.clone());
    experiences.insert(id.clone(), id.clone());
    for model in expand_model_entry(&id, entry, recommended, wire_map) {
        aicode_ids.insert(model.id.clone());
        experiences
            .entry(model.id.clone())
            .or_insert_with(|| model.id.clone());
        if seen.insert(model.id.clone()) {
            out.push(model);
        }
    }
}

fn extract_aicode_model_id(entry: &Value) -> Option<String> {
    let raw = entry
        .get("modelId")
        .or_else(|| entry.get("baseModelId"))
        .or_else(|| entry.get("name"))
        .and_then(Value::as_str)?
        .trim();
    let cleaned = raw.strip_prefix("models/").unwrap_or(raw).trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

pub(super) fn merge_model_catalogs(
    base_models: &mut Vec<AvailableModel>,
    base_wire: &mut EffortWireMap,
    ai_models: Vec<AvailableModel>,
    ai_wire: EffortWireMap,
) {
    for model in ai_models {
        if let Some(existing) = base_models.iter_mut().find(|m| m.id == model.id) {
            if existing.quota_percent.is_none() {
                existing.quota_percent = model.quota_percent;
            }
            existing.recommended |= model.recommended;
        } else {
            base_models.push(model);
        }
    }
    for (ui_id, mapping) in ai_wire {
        base_wire.entry(ui_id).or_insert(mapping);
    }
}

fn collect_recommended_ids(root: &Value) -> Vec<String> {
    let mut ids_out = Vec::new();
    if let Some(sorts) = root.get("agentModelSorts").and_then(Value::as_array) {
        for sort in sorts {
            if let Some(groups) = sort.get("groups").and_then(Value::as_array) {
                for group in groups {
                    if let Some(ids) = group.get("modelIds").and_then(Value::as_array) {
                        for id in ids.iter().filter_map(Value::as_str) {
                            if !ids_out.iter().any(|existing| existing == id) {
                                ids_out.push(id.to_owned());
                            }
                        }
                    }
                }
            }
        }
    }
    ids_out
}

fn is_noise_model_id(id: &str) -> bool {
    id.starts_with("chat_") || id.starts_with("tab_") || id.ends_with("-image")
}

fn expand_model_entry(
    id: &str,
    entry: &Value,
    recommended: bool,
    wire_map: &mut EffortWireMap,
) -> Vec<AvailableModel> {
    if entry.get("isInternal").and_then(Value::as_bool) == Some(true) {
        return Vec::new();
    }
    let quota_percent = parse_quota_percent(entry);
    let raw_display = entry
        .get("displayName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if let Some(levels) = entry
        .get("supportedThinkingLevels")
        .and_then(Value::as_array)
        .filter(|arr| !arr.is_empty())
    {
        let expanded = expand_from_supported_levels(
            id,
            raw_display,
            levels,
            quota_percent,
            recommended,
            wire_map,
        );
        if !expanded.is_empty() {
            return expanded;
        }
    }

    if let Some(stem) = id.strip_suffix("-tiered") {
        return expand_default_tiered(id, stem, raw_display, quota_percent, recommended, wire_map);
    }

    vec![AvailableModel {
        id: id.to_owned(),
        display_name: raw_display.unwrap_or(id).to_owned(),
        quota_percent,
        recommended,
    }]
}

fn expand_from_supported_levels(
    id: &str,
    raw_display: Option<&str>,
    levels: &[Value],
    quota_percent: Option<u8>,
    recommended: bool,
    wire_map: &mut EffortWireMap,
) -> Vec<AvailableModel> {
    let base_id = id.strip_suffix("-tiered").unwrap_or(id);
    let base_display = resolve_base_display(base_id, raw_display);
    let mut expanded = Vec::with_capacity(levels.len());
    for level in levels {
        let Some(suffix) = level
            .get("thinkingSuffix")
            .and_then(Value::as_str)
            .map(|s| s.trim().trim_start_matches('-'))
        else {
            continue;
        };
        let Some(enum_val) = thinking_enum_for_suffix(suffix) else {
            continue;
        };
        let label = level
            .get("thinkingDisplayName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| humanize_model_stem(suffix));
        let variant_id = format!("{base_id}-{suffix}");
        wire_map.insert(variant_id.clone(), (id.to_owned(), enum_val.to_owned()));
        expanded.push(AvailableModel {
            id: variant_id,
            display_name: format!("{base_display} ({label})"),
            quota_percent,
            recommended,
        });
    }
    expanded
}

fn expand_default_tiered(
    wire_id: &str,
    stem: &str,
    raw_display: Option<&str>,
    quota_percent: Option<u8>,
    recommended: bool,
    wire_map: &mut EffortWireMap,
) -> Vec<AvailableModel> {
    let base_display = resolve_base_display(stem, raw_display);
    [
        ("low", "Low", "LOW"),
        ("medium", "Medium", "MEDIUM"),
        ("high", "High", "HIGH"),
    ]
    .into_iter()
    .map(|(suffix, label, enum_val)| {
        let variant_id = format!("{stem}-{suffix}");
        wire_map.insert(
            variant_id.clone(),
            (wire_id.to_owned(), enum_val.to_owned()),
        );
        AvailableModel {
            id: variant_id,
            display_name: format!("{base_display} ({label})"),
            quota_percent,
            recommended,
        }
    })
    .collect()
}

fn resolve_base_display(stem: &str, raw_display: Option<&str>) -> String {
    raw_display
        .map(strip_tiered_label)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| humanize_model_stem(stem))
}

fn parse_quota_percent(entry: &Value) -> Option<u8> {
    entry
        .pointer("/quotaInfo/remainingFraction")
        .and_then(Value::as_f64)
        .map(|frac| {
            let pct = (frac * 100.0).clamp(0.0, 100.0).round();
            format!("{pct:.0}").parse::<u8>().unwrap_or(0)
        })
}

fn strip_tiered_label(display: &str) -> String {
    for suffix in [" (Tiered)", " (tiered)", " Tiered", " tiered"] {
        if let Some(stripped) = display.strip_suffix(suffix) {
            return stripped.trim().to_owned();
        }
    }
    display.trim().to_owned()
}

fn humanize_model_stem(stem: &str) -> String {
    stem.split('-')
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(first) if first.is_ascii_alphabetic() => {
                    format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
                }
                _ => seg.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn thinking_enum_for_suffix(suffix: &str) -> Option<&'static str> {
    match suffix.to_ascii_lowercase().as_str() {
        "minimal" => Some("MINIMAL"),
        "low" => Some("LOW"),
        "medium" => Some("MEDIUM"),
        "high" => Some("HIGH"),
        "xhigh" => Some("XHIGH"),
        _ => None,
    }
}

pub(super) fn resolve_static_model_and_thinking(trimmed: &str) -> (String, Option<String>) {
    let wire = AntigravityProvider::normalize_model_id(trimmed).to_owned();
    let effort = match trimmed {
        "gemini-3.8-flash-minimal" => Some("MINIMAL"),
        "gemini-3.8-flash-low" => Some("LOW"),
        "gemini-3.8-flash" | "gemini-3.8-flash-medium" => Some("MEDIUM"),
        "gemini-3.8-flash-high" | "gemini-3.8-flash-tiered" => Some("HIGH"),
        "gemini-3.8-flash-xhigh" => Some("XHIGH"),
        other if other.ends_with("-tiered") => Some("HIGH"),
        _ => None,
    };
    (wire, effort.map(str::to_owned))
}
