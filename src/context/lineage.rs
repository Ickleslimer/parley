use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::error::ContextError;
use super::parse::{get_str, parse_json_strict, required_object, required_str, required_u64};
use super::reader::{read_first_complete_record, MAX_PHYSICAL_RECORD};
use super::root::ResolvedSource;
use super::winfile::{open_shared_read, source_identity, HistoryIdentity, SourceIdentity};

#[derive(Clone, Debug)]
pub(crate) struct SourcePart {
    pub path: PathBuf,
    pub identity: SourceIdentity,
    pub extent: HistoryIdentity,
}

struct Metadata {
    header_end: u64,
    fingerprint: String,
    base: Option<(String, u64, u64)>,
    ordinal: Option<u64>,
}

fn metadata(path: &Path, session: &str) -> Result<Metadata, ContextError> {
    let record = read_first_complete_record(path)?
        .ok_or_else(|| ContextError::malformed("missing rollout header"))?;
    let json = parse_json_strict(&record.text)?;
    let payload = required_object(&json, "payload")?;
    if get_str(&json, "type") != Some("session_meta") || get_str(payload, "id") != Some(session) {
        return Err(ContextError::mismatch(
            "rollout header does not match source session",
        ));
    }
    let base = match payload.get("history_base") {
        None | Some(crate::json::Json::Null) => None,
        Some(value) => {
            if get_str(payload, "history_mode") != Some("paginated") {
                return Err(ContextError::malformed(
                    "history_base requires paginated history",
                ));
            }
            Some((
                required_str(value, "thread_id")?,
                required_u64(value, "end_ordinal_exclusive")?,
                required_u64(value, "end_byte_offset")?,
            ))
        }
    };
    let ordinal = json
        .get("ordinal")
        .map(|_| required_u64(&json, "ordinal"))
        .transpose()?;
    if let Some((_, cut, _)) = &base {
        if ordinal != Some(*cut) {
            return Err(ContextError::mismatch(
                "paginated header ordinal does not match its history base",
            ));
        }
    }
    Ok(Metadata {
        header_end: record.end_offset,
        fingerprint: format!("{:016x}", crate::signals::fnv1a_64(&record.text)),
        base,
        ordinal,
    })
}

fn physical_id(path: &Path) -> Option<&str> {
    let stem = path.file_stem()?.to_str()?;
    let identity = stem.get(28..)?;
    Some(
        identity
            .rsplit_once('_')
            .map_or(identity, |(_, suffix)| suffix),
    )
}

fn validate_boundary(
    path: &Path,
    end: u64,
    ordinal: u64,
    start_ordinal: u64,
) -> Result<(), ContextError> {
    let mut file = open_shared_read(path)?;
    if end == 0 || end > file.metadata()?.len() || ordinal <= start_ordinal {
        return Err(ContextError::mismatch(
            "history prefix is truncated or has invalid ordinal bounds",
        ));
    }
    let start = end.saturating_sub(MAX_PHYSICAL_RECORD as u64 + 2);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; (end - start) as usize];
    file.read_exact(&mut bytes)?;
    if bytes.last() != Some(&b'\n') {
        return Err(ContextError::mismatch(
            "history cutoff is not a complete JSONL boundary",
        ));
    }
    bytes.pop();
    let offset = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);
    if start != 0 && offset == 0 {
        return Err(ContextError::malformed(
            "history boundary record exceeds bounded validation limit",
        ));
    }
    let text = std::str::from_utf8(&bytes[offset..])
        .map_err(|_| ContextError::malformed("history boundary is not UTF-8"))?;
    let record = parse_json_strict(text)?;
    if required_u64(&record, "ordinal")?.checked_add(1) != Some(ordinal) {
        return Err(ContextError::mismatch(
            "history cutoff byte and ordinal disagree",
        ));
    }
    Ok(())
}

pub(super) fn resolve(
    root: &Path,
    session: &str,
    selected: PathBuf,
    rollouts: &[PathBuf],
) -> Result<ResolvedSource, ContextError> {
    let mut path = selected;
    let mut end = None;
    let mut seen = HashSet::new();
    let mut reversed = Vec::new();
    loop {
        path = std::fs::canonicalize(&path)?;
        if !path.starts_with(root) {
            return Err(ContextError::mismatch(
                "history source escapes sessions root",
            ));
        }
        let file = open_shared_read(&path)?;
        let identity = source_identity(&file, &path)?;
        if !seen.insert((identity.volume_serial, identity.file_index)) || reversed.len() >= 8 {
            return Err(ContextError::malformed(
                "history lineage cycle or depth limit",
            ));
        }
        let meta = metadata(&path, session)?;
        if (meta.base.is_some() || end.is_some())
            && path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.get(28..))
                .map(|name| name.split_once('_').map_or(name, |(owner, _)| owner))
                != Some(session)
        {
            return Err(ContextError::mismatch(
                "history filename owner does not match metadata",
            ));
        }
        if let Some((limit, ordinal)) = end {
            validate_boundary(&path, limit, ordinal, meta.ordinal.unwrap_or(0))?;
        }
        let physical_start = if meta.base.is_some() {
            meta.header_end
        } else {
            0
        };
        if end.is_some_and(|(limit, _)| limit < physical_start) {
            return Err(ContextError::mismatch(
                "history cutoff precedes inherited prefix",
            ));
        }
        reversed.push(SourcePart {
            path: path.clone(),
            identity: identity.clone(),
            extent: HistoryIdentity {
                volume_serial: identity.volume_serial,
                file_index: identity.file_index,
                logical_start: 0,
                physical_start,
                physical_end: end.map(|(limit, _)| limit),
                header_fingerprint: meta.fingerprint,
            },
        });
        let Some((base_id, ordinal, offset)) = meta.base else {
            break;
        };
        let bases: Vec<_> = rollouts
            .iter()
            .filter(|candidate| physical_id(candidate) == Some(&base_id))
            .collect();
        if bases.len() != 1 {
            return Err(ContextError::mismatch(
                "history base physical rollout is missing or ambiguous",
            ));
        }
        path = bases[0].clone();
        end = Some((offset, ordinal));
    }
    reversed.reverse();
    let mut position = 0;
    for part in &mut reversed {
        part.extent.logical_start = position;
        if let Some(end) = part.extent.physical_end {
            position = position
                .checked_add(end - part.extent.physical_start)
                .ok_or_else(|| ContextError::overflow("logical history offset overflow"))?;
        }
    }
    let first = &reversed[0];
    let path = first.path.clone();
    let mut identity = first.identity.clone();
    if reversed.len() > 1 {
        identity.history = reversed.iter().map(|part| part.extent.clone()).collect();
    }
    Ok(ResolvedSource {
        session_id: session.to_string(),
        path,
        identity,
        parts: reversed,
        selection: None,
    })
}
