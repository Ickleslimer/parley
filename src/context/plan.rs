//! Bounded seed and delta planners over sanitized Codex records.

use crate::signals::fnv1a_64;

use super::error::ContextError;
use super::reader::PhysicalRecord;
use super::root::ResolvedSource;
use super::sanitize::{content_chars, render_messages, visible_messages, VisibleMessage};
use super::winfile::SourceIdentity;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlanKind {
    Seed,
    Delta,
    Replay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ContextBounds {
    pub seed_max_chars: usize,
    pub delta_max_chars: usize,
    pub hard_max_chars: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MessageAnchor {
    pub byte_offset: u64,
    pub message_id: String,
    pub turn_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ContextPlan {
    pub kind: PlanKind,
    pub text: String,
    pub truncated_front: bool,
    pub source_identity: SourceIdentity,
    pub source_session: String,
    pub start_offset: u64,
    pub end_offset: u64,
    pub first_message: Option<MessageAnchor>,
    pub last_message: Option<MessageAnchor>,
    pub first_turn_id: Option<String>,
    pub last_turn_id: Option<String>,
    pub record_count: u64,
    pub character_count: u64,
    pub fingerprint: String,
}

pub(crate) fn plan_seed(
    source: &ResolvedSource,
    records: &[PhysicalRecord],
    complete_end: u64,
    bounds: &ContextBounds,
) -> Result<ContextPlan, ContextError> {
    let messages = visible_messages(records)?;
    let groups = group_turns(&messages);
    if let Some(newest) = groups.last() {
        let newest_chars = content_chars(newest);
        if newest_chars as usize > bounds.seed_max_chars {
            return Err(ContextError::overflow(
                "newest turn group exceeds seed_max_chars",
            ));
        }
    }

    let mut selected_from = groups.len();
    let mut chars = 0usize;
    for index in (0..groups.len()).rev() {
        let group_chars = content_chars(&groups[index]) as usize;
        if chars.saturating_add(group_chars) > bounds.seed_max_chars {
            break;
        }
        chars += group_chars;
        selected_from = index;
    }
    let selected: Vec<VisibleMessage> = groups[selected_from..].iter().flatten().cloned().collect();
    if chars > bounds.hard_max_chars {
        return Err(ContextError::overflow(
            "seed character count exceeds hard_max_chars",
        ));
    }

    let truncated_front = selected_from > 0;
    let start_offset = selected
        .first()
        .map(|message| message.turn_start_offset)
        .or_else(|| records.first().map(|record| record.start_offset))
        .unwrap_or(0);
    let range: Vec<&PhysicalRecord> = records
        .iter()
        .filter(|record| record.start_offset >= start_offset)
        .collect();
    Ok(build_plan(
        PlanKind::Seed,
        source,
        &selected,
        &range,
        start_offset,
        complete_end,
        truncated_front,
    ))
}

pub(crate) fn plan_delta(
    source: &ResolvedSource,
    records: &[PhysicalRecord],
    start_offset: u64,
    complete_end: u64,
    bounds: &ContextBounds,
) -> Result<ContextPlan, ContextError> {
    let messages = visible_messages(records)?;
    let chars = content_chars(&messages);
    if chars as usize > bounds.delta_max_chars {
        return Err(ContextError::overflow(
            "delta character count exceeds delta_max_chars",
        ));
    }
    if chars as usize > bounds.hard_max_chars {
        return Err(ContextError::overflow(
            "delta character count exceeds hard_max_chars",
        ));
    }
    let range: Vec<&PhysicalRecord> = records.iter().collect();
    Ok(build_plan(
        PlanKind::Delta,
        source,
        &messages,
        &range,
        start_offset,
        complete_end,
        false,
    ))
}

pub(crate) fn plan_replay(
    source: &ResolvedSource,
    records: &[PhysicalRecord],
    start_offset: u64,
    end_offset: u64,
) -> Result<ContextPlan, ContextError> {
    let messages = visible_messages(records)?;
    let range: Vec<&PhysicalRecord> = records.iter().collect();
    Ok(build_plan(
        PlanKind::Replay,
        source,
        &messages,
        &range,
        start_offset,
        end_offset,
        false,
    ))
}

pub(crate) fn fingerprint_records<'a, I>(records: I) -> String
where
    I: IntoIterator<Item = &'a PhysicalRecord>,
{
    let mut joined = String::new();
    for (index, record) in records.into_iter().enumerate() {
        if index > 0 {
            joined.push('\n');
        }
        joined.push_str(&record.text);
    }
    format!("{:016x}", fnv1a_64(&joined))
}

fn group_turns(messages: &[VisibleMessage]) -> Vec<Vec<VisibleMessage>> {
    let mut groups: Vec<Vec<VisibleMessage>> = Vec::new();
    for message in messages {
        match groups.last_mut() {
            Some(group)
                if group
                    .first()
                    .is_some_and(|first| first.turn_id == message.turn_id) =>
            {
                group.push(message.clone());
            }
            _ => groups.push(vec![message.clone()]),
        }
    }
    groups
}

fn build_plan(
    kind: PlanKind,
    source: &ResolvedSource,
    messages: &[VisibleMessage],
    range: &[&PhysicalRecord],
    start_offset: u64,
    end_offset: u64,
    truncated_front: bool,
) -> ContextPlan {
    ContextPlan {
        kind,
        text: render_messages(messages),
        truncated_front,
        source_identity: source.identity.clone(),
        source_session: source.session_id.clone(),
        start_offset,
        end_offset,
        first_message: messages.first().map(anchor),
        last_message: messages.last().map(anchor),
        first_turn_id: messages.first().map(|message| message.turn_id.clone()),
        last_turn_id: messages.last().map(|message| message.turn_id.clone()),
        record_count: range.len() as u64,
        character_count: content_chars(messages),
        fingerprint: fingerprint_records(range.iter().copied()),
    }
}

fn anchor(message: &VisibleMessage) -> MessageAnchor {
    MessageAnchor {
        byte_offset: message.start_offset,
        message_id: message.message_id.clone(),
        turn_id: message.turn_id.clone(),
    }
}
