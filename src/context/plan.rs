//! Bounded seed and delta planners over sanitized Codex records.

use std::collections::VecDeque;

#[cfg(test)]
use crate::signals::fnv1a_64;

use super::error::{ContextError, ErrorKind};
use super::reader::{PhysicalRecord, ScanSummary, SharedJsonlReader};
use super::root::ResolvedSource;
use super::sanitize::{content_chars, render_messages, VisibleMessage, VisibleMessageDecoder};
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
    reader: &mut SharedJsonlReader,
    bounds: &ContextBounds,
) -> Result<ContextPlan, ContextError> {
    let mut collector = SeedCollector::new(bounds.seed_max_chars);
    let full = reader.scan_from(0, |record| collector.push(record))?;
    let selection = collector.finish()?;
    if selection.character_count > bounds.hard_max_chars {
        return Err(ContextError::overflow(
            "seed character count exceeds hard_max_chars",
        ));
    }

    let start_offset = selection
        .messages
        .first()
        .map(|message| message.turn_start_offset)
        .or(full.first_record_start)
        .unwrap_or(0);
    let mut decoder = VisibleMessageDecoder::default();
    let mut messages = Vec::new();
    let selected = reader.scan_until(start_offset, full.complete_end, |record| {
        if let Some(message) = decoder.push(&record)? {
            messages.push(message);
        }
        Ok(())
    })?;
    if !same_seed_selection(&selection.messages, &messages) {
        return Err(ContextError::mismatch(
            "source changed while selecting the seed range",
        ));
    }
    let chars = content_chars(&messages) as usize;
    if chars > bounds.seed_max_chars {
        return Err(ContextError::overflow(
            "selected seed range exceeds seed_max_chars",
        ));
    }
    if chars > bounds.hard_max_chars {
        return Err(ContextError::overflow(
            "seed character count exceeds hard_max_chars",
        ));
    }

    Ok(build_plan(
        PlanKind::Seed,
        source,
        &messages,
        &selected,
        start_offset,
        full.complete_end,
        selection.truncated_front,
    ))
}

pub(crate) fn plan_delta(
    source: &ResolvedSource,
    reader: &mut SharedJsonlReader,
    start_offset: u64,
    bounds: &ContextBounds,
) -> Result<ContextPlan, ContextError> {
    let limit = bounds.delta_max_chars.min(bounds.hard_max_chars);
    let mut collector = MessageCollector::new(Some(limit), "delta character count exceeds limit");
    let summary = reader.scan_from(start_offset, |record| collector.push(record))?;
    let messages = collector.finish();
    let chars = content_chars(&messages) as usize;
    if chars > bounds.delta_max_chars {
        return Err(ContextError::overflow(
            "delta character count exceeds delta_max_chars",
        ));
    }
    if chars > bounds.hard_max_chars {
        return Err(ContextError::overflow(
            "delta character count exceeds hard_max_chars",
        ));
    }
    Ok(build_plan(
        PlanKind::Delta,
        source,
        &messages,
        &summary,
        start_offset,
        summary.complete_end,
        false,
    ))
}

const CATCH_UP_CUT: &str = "catch-up stopped at the rejected turn boundary";

pub(crate) fn plan_catch_up(
    source: &ResolvedSource,
    reader: &mut SharedJsonlReader,
    start_offset: u64,
    bounds: &ContextBounds,
) -> Result<ContextPlan, ContextError> {
    if !source.identity.same_file(reader.identity()) {
        return Err(ContextError::new(
            ErrorKind::Replaced,
            format!("source file identity changed for {}", source.session_id),
        ));
    }
    let mut selector = CatchUpSelector::new(start_offset, bounds);
    let selection = match reader.scan_from(start_offset, |record| selector.push(record)) {
        Ok(summary) => selector.refuse_fitting_tail(&summary)?,
        Err(error) if error.kind == ErrorKind::State && error.message == CATCH_UP_CUT => {
            selector.confirmed_cut()?
        }
        Err(error) => return Err(error),
    };
    let mut decoder = VisibleMessageDecoder::default();
    let mut messages = Vec::new();
    let selected = reader.scan_until(start_offset, selection.cut, |record| {
        if let Some(message) = decoder.push(&record)? {
            messages.push(message);
        }
        Ok(())
    })?;
    if selected.complete_end != selection.cut {
        return Err(ContextError::mismatch(
            "catch-up byte range does not match the selected cut",
        ));
    }
    if messages != selection.messages {
        return Err(ContextError::mismatch(
            "source changed while selecting the catch-up range",
        ));
    }
    let limit = bounds.delta_max_chars.min(bounds.hard_max_chars);
    if content_chars(&messages) as usize > limit {
        return Err(ContextError::overflow(
            "selected catch-up range exceeds the character limit",
        ));
    }
    Ok(build_plan(
        PlanKind::Delta,
        source,
        &messages,
        &selected,
        start_offset,
        selection.cut,
        false,
    ))
}

pub(crate) fn plan_replay(
    source: &ResolvedSource,
    summary: &ScanSummary,
    messages: &[VisibleMessage],
    start_offset: u64,
    end_offset: u64,
) -> ContextPlan {
    build_plan(
        PlanKind::Replay,
        source,
        messages,
        summary,
        start_offset,
        end_offset,
        false,
    )
}

pub(crate) fn scan_evidence(
    reader: &mut SharedJsonlReader,
    start_offset: u64,
    end_offset: u64,
    max_chars: usize,
) -> Result<(ScanSummary, Vec<VisibleMessage>), ContextError> {
    let mut collector = MessageCollector::new(
        Some(max_chars),
        "context evidence exceeds the stored cursor bound",
    );
    let summary = reader.scan_until(start_offset, end_offset, |record| collector.push(record))?;
    Ok((summary, collector.finish()))
}

#[cfg(test)]
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

struct SeedSelection {
    messages: Vec<VisibleMessage>,
    character_count: usize,
    truncated_front: bool,
}

struct TurnGroup {
    turn_id: String,
    messages: Vec<VisibleMessage>,
    character_count: usize,
    oversized: bool,
}

struct SeedCollector {
    decoder: VisibleMessageDecoder,
    groups: VecDeque<TurnGroup>,
    character_count: usize,
    max_chars: usize,
    truncated_front: bool,
}

impl SeedCollector {
    fn new(max_chars: usize) -> Self {
        Self {
            decoder: VisibleMessageDecoder::default(),
            groups: VecDeque::new(),
            character_count: 0,
            max_chars,
            truncated_front: false,
        }
    }

    fn push(&mut self, record: PhysicalRecord) -> Result<(), ContextError> {
        let Some(message) = self.decoder.push(&record)? else {
            return Ok(());
        };
        let message_chars = message.text.chars().count();
        let same_turn = self
            .groups
            .back()
            .is_some_and(|group| group.turn_id == message.turn_id);
        if !same_turn {
            if self.groups.back().is_some_and(|group| group.oversized) {
                self.groups.clear();
                self.character_count = 0;
                self.truncated_front = true;
            }
            self.groups.push_back(TurnGroup {
                turn_id: message.turn_id.clone(),
                messages: Vec::new(),
                character_count: 0,
                oversized: false,
            });
        }

        let group = self
            .groups
            .back_mut()
            .expect("message creates a turn group");
        if group.oversized {
            return Ok(());
        }
        if group.character_count.saturating_add(message_chars) > self.max_chars {
            self.character_count = self.character_count.saturating_sub(group.character_count);
            group.messages.clear();
            group.character_count = 0;
            group.oversized = true;
            while self.groups.len() > 1 {
                self.groups.pop_front();
            }
            self.truncated_front = true;
            return Ok(());
        }
        group.character_count += message_chars;
        group.messages.push(message);
        self.character_count += message_chars;
        while self.character_count > self.max_chars && self.groups.len() > 1 {
            let removed = self.groups.pop_front().expect("checked group length");
            self.character_count = self.character_count.saturating_sub(removed.character_count);
            self.truncated_front = true;
        }
        Ok(())
    }

    fn finish(self) -> Result<SeedSelection, ContextError> {
        if self.groups.back().is_some_and(|group| group.oversized) {
            return Err(ContextError::overflow(
                "newest turn group exceeds seed_max_chars",
            ));
        }
        let messages = self
            .groups
            .into_iter()
            .flat_map(|group| group.messages)
            .collect();
        Ok(SeedSelection {
            messages,
            character_count: self.character_count,
            truncated_front: self.truncated_front,
        })
    }
}

struct MessageCollector {
    decoder: VisibleMessageDecoder,
    messages: Vec<VisibleMessage>,
    character_count: usize,
    limit: Option<usize>,
    overflow_message: &'static str,
}

impl MessageCollector {
    fn new(limit: Option<usize>, overflow_message: &'static str) -> Self {
        Self {
            decoder: VisibleMessageDecoder::default(),
            messages: Vec::new(),
            character_count: 0,
            limit,
            overflow_message,
        }
    }

    fn push(&mut self, record: PhysicalRecord) -> Result<(), ContextError> {
        let Some(message) = self.decoder.push(&record)? else {
            return Ok(());
        };
        self.character_count = self
            .character_count
            .saturating_add(message.text.chars().count());
        if self.limit.is_some_and(|limit| self.character_count > limit) {
            return Err(ContextError::overflow(self.overflow_message));
        }
        self.messages.push(message);
        Ok(())
    }

    fn finish(self) -> Vec<VisibleMessage> {
        self.messages
    }
}

struct CatchUpSelection {
    messages: Vec<VisibleMessage>,
    cut: u64,
}

struct OpenTurn {
    turn_id: String,
    start_offset: u64,
    messages: Vec<VisibleMessage>,
    chars: usize,
}

struct CatchUpSelector {
    decoder: VisibleMessageDecoder,
    cursor: u64,
    limit: usize,
    hard_is_tighter: bool,
    accepted: Vec<VisibleMessage>,
    accepted_chars: usize,
    closed_ids: Vec<String>,
    open: Option<OpenTurn>,
    cut: Option<u64>,
}

impl CatchUpSelector {
    fn new(cursor: u64, bounds: &ContextBounds) -> Self {
        Self {
            decoder: VisibleMessageDecoder::default(),
            cursor,
            limit: bounds.delta_max_chars.min(bounds.hard_max_chars),
            hard_is_tighter: bounds.hard_max_chars < bounds.delta_max_chars,
            accepted: Vec::new(),
            accepted_chars: 0,
            closed_ids: Vec::new(),
            open: None,
            cut: None,
        }
    }

    fn push(&mut self, record: PhysicalRecord) -> Result<(), ContextError> {
        if self.cut.is_some() {
            return Ok(());
        }
        let Some(message) = self.decoder.push(&record)? else {
            return Ok(());
        };
        if message.start_offset < self.cursor || message.turn_start_offset < self.cursor {
            return Err(ContextError::mismatch(
                "catch-up cut does not advance the cursor",
            ));
        }
        let message_chars = message.text.chars().count();
        let same_turn = self
            .open
            .as_ref()
            .is_some_and(|open| open.turn_id == message.turn_id);
        if !same_turn {
            if self
                .closed_ids
                .iter()
                .any(|turn_id| turn_id == &message.turn_id)
            {
                return Err(ContextError::mismatch(
                    "catch-up turn boundary is not contiguous",
                ));
            }
            if let Some(open) = self.open.take() {
                self.commit_open(open);
            }
            if let Some(previous) = self.accepted.last() {
                if message.turn_start_offset < previous.end_offset {
                    return Err(ContextError::mismatch(
                        "catch-up turn boundary is not contiguous",
                    ));
                }
            }
            self.open = Some(OpenTurn {
                turn_id: message.turn_id.clone(),
                start_offset: message.turn_start_offset,
                messages: Vec::new(),
                chars: 0,
            });
        }
        let accepted_chars = self.accepted_chars;
        let limit = self.limit;
        let decision = {
            let Some(open) = self.open.as_mut() else {
                return Err(ContextError::mismatch(
                    "catch-up turn boundary is not contiguous",
                ));
            };
            if message.start_offset < open.start_offset {
                return Err(ContextError::mismatch(
                    "catch-up turn boundary is not contiguous",
                ));
            }
            let fits = open
                .chars
                .checked_add(message_chars)
                .and_then(|chars| accepted_chars.checked_add(chars))
                .is_some_and(|count| count <= limit);
            (fits, open.start_offset)
        };
        let (fits, cut) = decision;
        if !fits {
            self.open = None;
            if self.accepted.is_empty() {
                return Err(self.first_turn_error());
            }
            if cut <= self.cursor {
                return Err(ContextError::mismatch(
                    "catch-up cut does not advance the cursor",
                ));
            }
            self.cut = Some(cut);
            return Err(ContextError::state(CATCH_UP_CUT));
        }
        let Some(open) = self.open.as_mut() else {
            return Err(ContextError::mismatch(
                "catch-up turn boundary is not contiguous",
            ));
        };
        open.chars += message_chars;
        open.messages.push(message);
        Ok(())
    }

    fn commit_open(&mut self, open: OpenTurn) {
        self.accepted_chars = self.accepted_chars.saturating_add(open.chars);
        self.closed_ids.push(open.turn_id);
        self.accepted.extend(open.messages);
    }

    fn first_turn_error(&self) -> ContextError {
        if self.hard_is_tighter {
            ContextError::overflow("first unread turn exceeds hard_max_chars")
        } else {
            ContextError::overflow("first unread turn exceeds delta_max_chars")
        }
    }

    fn confirmed_cut(&self) -> Result<CatchUpSelection, ContextError> {
        let Some(cut) = self.cut else {
            return Err(ContextError::mismatch(
                "catch-up cut does not advance the cursor",
            ));
        };
        if self.accepted.is_empty() || cut <= self.cursor {
            return Err(ContextError::mismatch(
                "catch-up cut does not advance the cursor",
            ));
        }
        let unsafe_range = self.accepted.iter().any(|message| {
            message.start_offset < self.cursor
                || message.end_offset > cut
                || message.turn_start_offset < self.cursor
                || message.turn_start_offset >= cut
        });
        if unsafe_range {
            return Err(ContextError::mismatch(
                "catch-up cut does not advance the cursor",
            ));
        }
        Ok(CatchUpSelection {
            messages: self.accepted.clone(),
            cut,
        })
    }

    fn refuse_fitting_tail(&self, summary: &ScanSummary) -> Result<CatchUpSelection, ContextError> {
        if self.cut.is_some() {
            return Err(ContextError::mismatch(
                "catch-up cut does not advance the cursor",
            ));
        }
        if summary.record_count == 0 && self.accepted.is_empty() && self.open.is_none() {
            return Err(ContextError::unsupported(
                "catch-up requires unread history",
            ));
        }
        Err(ContextError::unsupported(
            "unread tail fits in an ordinary delta",
        ))
    }
}

fn same_seed_selection(expected: &[VisibleMessage], actual: &[VisibleMessage]) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let expected_boundaries = turn_boundaries(expected);
    let actual_boundaries = turn_boundaries(actual);
    if expected_boundaries != actual_boundaries {
        return false;
    }
    expected.iter().zip(actual).all(|(left, right)| {
        left.role == right.role
            && left.phase == right.phase
            && left.text == right.text
            && left.message_id == right.message_id
            && left.turn_start_offset == right.turn_start_offset
            && left.start_offset == right.start_offset
            && left.end_offset == right.end_offset
            && (left.turn_id == right.turn_id
                || (left.turn_id.starts_with("synthetic:")
                    && right.turn_id.starts_with("synthetic:")))
    })
}

fn turn_boundaries(messages: &[VisibleMessage]) -> Vec<bool> {
    messages
        .windows(2)
        .map(|pair| pair[0].turn_id != pair[1].turn_id)
        .collect()
}

fn build_plan(
    kind: PlanKind,
    source: &ResolvedSource,
    messages: &[VisibleMessage],
    summary: &ScanSummary,
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
        record_count: summary.record_count,
        character_count: content_chars(messages),
        fingerprint: summary.fingerprint.clone(),
    }
}

fn anchor(message: &VisibleMessage) -> MessageAnchor {
    MessageAnchor {
        byte_offset: message.start_offset,
        message_id: message.message_id.clone(),
        turn_id: message.turn_id.clone(),
    }
}
