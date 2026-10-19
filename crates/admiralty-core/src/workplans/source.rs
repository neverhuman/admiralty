//! Explicit source formats. Markdown observations require a reviewed contract.
use super::{MAX_WORKPLAN_BYTES, MAX_WORKPLAN_TASKS, Workplan, WorkplanError, preview_workplan};
use crate::{TaskId, content_digest, intake::SourceSpan};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const PARSER_VERSION: &str = "workplan-source-v1";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanSource {
    Json,
    Toml,
    Markdown,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ObservedTask {
    pub original_id: String,
    pub title: String,
    pub dependencies: Vec<String>,
    pub owned_paths: Vec<String>,
    pub criteria: Vec<String>,
    /// Imported status is preserved for review and never treated as acceptance.
    pub legacy_status: Option<String>,
    pub source_span: SourceSpan,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SourceDiagnostic {
    pub line: usize,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SourcePreview {
    pub format: PlanSource,
    pub parser_version: String,
    pub source_digest: String,
    pub canonical_digest: Option<String>,
    pub canonical_workplan: Option<Workplan>,
    pub observed_tasks: Vec<ObservedTask>,
    pub diagnostics: Vec<SourceDiagnostic>,
    pub execution_granted: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SourceError {
    #[error(transparent)]
    Workplan(#[from] WorkplanError),
    #[error("source is not valid UTF-8")]
    Utf8,
    #[error("invalid typed TOML workplan: {0}")]
    Toml(String),
    #[error("Markdown preview exceeds the task or diagnostic bound")]
    MarkdownBound,
}

/// The caller selects the format. File extensions and imported status do not
/// select canonical authority. This function performs no I/O or task admission.
pub fn preview_source(source: &[u8], format: PlanSource) -> Result<SourcePreview, SourceError> {
    if source.len() > MAX_WORKPLAN_BYTES {
        return Err(WorkplanError::TooLarge.into());
    }
    let text = std::str::from_utf8(source).map_err(|_| SourceError::Utf8)?;
    let mut result = SourcePreview {
        format,
        parser_version: PARSER_VERSION.into(),
        source_digest: format!("sha256:{:x}", Sha256::digest(source)),
        canonical_digest: None,
        canonical_workplan: None,
        observed_tasks: vec![],
        diagnostics: vec![],
        execution_granted: false,
    };
    if format == PlanSource::Markdown {
        markdown(text, &mut result)?;
        return Ok(result);
    }
    let plan: Workplan = match format {
        PlanSource::Json => {
            preview_workplan(source)?;
            serde_json::from_slice(source).map_err(|e| WorkplanError::Json(e.to_string()))?
        }
        PlanSource::Toml => toml::from_str(text).map_err(|e| SourceError::Toml(e.to_string()))?,
        PlanSource::Markdown => unreachable!(),
    };
    let canonical = serde_json::to_vec(&plan).map_err(|e| WorkplanError::Json(e.to_string()))?;
    preview_workplan(&canonical)?;
    result.canonical_digest =
        Some(content_digest(&plan).map_err(|e| WorkplanError::Json(e.to_string()))?);
    result.canonical_workplan = Some(plan);
    Ok(result)
}

fn cells(line: &str) -> Vec<&str> {
    line.trim()
        .strip_prefix('|')
        .and_then(|s| s.strip_suffix('|'))
        .unwrap_or("")
        .split('|')
        .map(str::trim)
        .collect()
}

fn list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn markdown(text: &str, result: &mut SourcePreview) -> Result<(), SourceError> {
    let mut header: Option<Vec<&str>> = None;
    let mut ids = BTreeSet::new();
    let mut offset = 0;
    let mut fence: Option<(char, usize)> = None;
    for (index, raw) in text.split_inclusive('\n').enumerate() {
        let line = raw.trim();
        let line_number = index + 1;
        if raw.starts_with("    ") || raw.starts_with('\t') {
            header = None;
            offset += raw.len();
            continue;
        }
        let marker = line.chars().next().filter(|c| matches!(c, '`' | '~'));
        if let Some(marker) = marker {
            let count = line.chars().take_while(|c| *c == marker).count();
            if count >= 3 {
                match fence {
                    None => {
                        fence = Some((marker, count));
                        header = None;
                    }
                    Some((open, length))
                        if open == marker && count >= length && line[count..].trim().is_empty() =>
                    {
                        fence = None
                    }
                    _ => {}
                }
                offset += raw.len();
                continue;
            }
        }
        if fence.is_some() {
            offset += raw.len();
            continue;
        }
        if line.starts_with('|') && line.ends_with('|') {
            let values = cells(line);
            if values.contains(&"id") && values.contains(&"title") {
                let unique: BTreeSet<_> = values.iter().collect();
                if unique.len() != values.len() {
                    result.diagnostics.push(SourceDiagnostic {
                        line: line_number,
                        reason: "duplicate table columns".into(),
                    });
                    header = None;
                } else {
                    header = Some(values);
                }
            } else if values
                .iter()
                .all(|v| !v.is_empty() && v.chars().all(|c| matches!(c, '-' | ':')))
            {
                // Markdown table separator.
            } else if let Some(columns) = &header {
                if columns.len() != values.len() {
                    result.diagnostics.push(SourceDiagnostic {
                        line: line_number,
                        reason: "ambiguous table row width; escaped pipes require manual review"
                            .into(),
                    });
                } else {
                    let field = |name: &str| {
                        columns
                            .iter()
                            .position(|c| *c == name)
                            .map(|i| values[i])
                            .unwrap_or("")
                    };
                    let id = field("id");
                    if TaskId::new(id).is_err() || !ids.insert(id.to_owned()) {
                        result.diagnostics.push(SourceDiagnostic {
                            line: line_number,
                            reason: "invalid or duplicate original task ID".into(),
                        });
                    }
                    result.observed_tasks.push(ObservedTask {
                        original_id: id.into(),
                        title: field("title").into(),
                        dependencies: list(field("dependencies")),
                        owned_paths: list(field("owned_paths")),
                        criteria: list(field("criteria")),
                        legacy_status: (!field("status").is_empty())
                            .then(|| field("status").to_owned()),
                        source_span: SourceSpan {
                            start: offset as u32,
                            end: (offset + raw.trim_end_matches(['\r', '\n']).len()) as u32,
                        },
                    });
                    result.diagnostics.push(SourceDiagnostic { line: line_number, reason: "review scope and supply a complete typed contract before registration; imported status is untrusted".into() });
                }
            } else {
                result.diagnostics.push(SourceDiagnostic {
                    line: line_number,
                    reason: "table requires explicit id and title columns".into(),
                });
            }
        } else {
            header = None;
            if line.starts_with("- [") || line.starts_with("* [") {
                result.diagnostics.push(SourceDiagnostic { line: line_number, reason: "checkbox has no canonical task identity or acceptance authority; use an explicit task table".into() });
            }
        }
        if result.observed_tasks.len() > MAX_WORKPLAN_TASKS
            || result.diagnostics.len() > MAX_WORKPLAN_TASKS * 2
        {
            return Err(SourceError::MarkdownBound);
        }
        offset += raw.len();
    }
    if result.observed_tasks.is_empty() && result.diagnostics.is_empty() {
        result.diagnostics.push(SourceDiagnostic {
            line: 1,
            reason: "no explicit task table found".into(),
        });
    }
    Ok(())
}
