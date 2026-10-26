// SPDX-License-Identifier: GPL-3.0-only
//! Conformance tests for the fabric request/tag model.
//!
//! These are model tests, not a live-module capacity claim. The MCP module's
//! pending table is deliberately much smaller (`PENDING_CAPACITY` in
//! `modules/mcp/src/mesh.rs`); see `docs/plan/testing.md` §0.

use std::collections::BTreeSet;

use fabric::{CancelError, CompleteOutcome, RequestLedger, TagAllocator, TagError};
use sys_primitives::{completion_flags, Completion, CompletionId, CompletionKind, Status, Tag};

fn completion(event: u64, tag: Tag, status: Status) -> Completion {
    Completion {
        event: CompletionId(event),
        tag,
        status,
        kind: CompletionKind::None,
        flags: completion_flags::FINAL,
        reserved: 0,
        value0: 0,
        value1: 0,
    }
}

#[test]
fn cancel_before_completion_has_one_terminal_record() {
    let mut ledger = RequestLedger::new(4, 4);
    let tag = ledger.begin(CompletionId(1)).expect("tag");
    assert_eq!(ledger.in_flight(), 1);

    ledger.cancel(tag).expect("cancel");
    assert_eq!(ledger.in_flight(), 0);
    assert_eq!(ledger.cancelled(), 1);

    // The operation reaches the home after the cancellation. It must not
    // overwrite the cancelled completion or create a second terminal record.
    assert_eq!(
        ledger.complete(completion(1, tag, Status::Ok)),
        CompleteOutcome::Discarded
    );
    assert_eq!(
        ledger.complete(completion(1, tag, Status::Ok)),
        CompleteOutcome::Unknown
    );

    let terminal = ledger.pop().expect("cancelled completion");
    assert_eq!(terminal.tag, tag);
    assert_eq!(terminal.status, Status::Cancelled);
    assert_ne!(terminal.flags & completion_flags::CANCELLED, 0);
    assert!(ledger.pop().is_none());
    assert_eq!(ledger.in_flight(), 0);
    assert_eq!(ledger.cancelled(), 0);
}

#[test]
fn completion_before_cancel_has_one_terminal_record() {
    let mut ledger = RequestLedger::new(4, 4);
    let tag = ledger.begin(CompletionId(2)).expect("tag");

    assert_eq!(
        ledger.complete(completion(2, tag, Status::Ok)),
        CompleteOutcome::Queued
    );
    assert_eq!(ledger.in_flight(), 0);

    // The completion already won the race; the late cancellation is refused
    // and cannot replace the queued result.
    assert_eq!(ledger.cancel(tag), Err(CancelError::Unknown));

    let terminal = ledger.pop().expect("completion");
    assert_eq!(terminal.tag, tag);
    assert_eq!(terminal.status, Status::Ok);
    assert_eq!(terminal.flags & completion_flags::CANCELLED, 0);
    assert!(ledger.pop().is_none());
}

#[test]
fn interleaved_cancellation_and_completion_has_one_terminal_per_tag() {
    const COUNT: u32 = 256;
    let mut ledger = RequestLedger::new(COUNT, COUNT as usize);
    let mut tags = Vec::with_capacity(COUNT as usize);
    for index in 0..COUNT {
        let tag = ledger
            .begin(CompletionId(u64::from(index) + 100))
            .expect("tag");
        tags.push((tag, index));
    }

    for (tag, index) in &tags {
        let event = u64::from(*index) + 100;
        if index % 2 == 0 {
            ledger.cancel(*tag).expect("cancel");
            assert_eq!(
                ledger.complete(completion(event, *tag, Status::Ok)),
                CompleteOutcome::Discarded
            );
        } else {
            assert_eq!(
                ledger.complete(completion(event, *tag, Status::Ok)),
                CompleteOutcome::Queued
            );
            assert_eq!(ledger.cancel(*tag), Err(CancelError::Unknown));
        }
    }

    let mut terminals = Vec::new();
    while let Some(terminal) = ledger.pop() {
        terminals.push(terminal);
    }
    assert_eq!(terminals.len(), COUNT as usize);
    let terminal_tags: BTreeSet<u32> = terminals.iter().map(|entry| entry.tag.0).collect();
    assert_eq!(terminal_tags.len(), COUNT as usize);
    for terminal in terminals {
        let expected_cancelled = (terminal.event.0 - 100) % 2 == 0;
        assert_eq!(
            terminal.status,
            if expected_cancelled {
                Status::Cancelled
            } else {
                Status::Ok
            }
        );
        assert_eq!(
            terminal.flags & completion_flags::CANCELLED != 0,
            expected_cancelled
        );
    }
    assert_eq!(ledger.in_flight(), 0);
    assert_eq!(ledger.cancelled(), 0);
    assert_eq!(ledger.pending_completions(), 0);
}

#[test]
fn full_completion_queue_applies_backpressure_without_losing_the_race() {
    let mut ledger = RequestLedger::new(4, 1);
    let first = ledger.begin(CompletionId(10)).expect("first");
    let second = ledger.begin(CompletionId(11)).expect("second");

    assert_eq!(
        ledger.complete(completion(10, first, Status::Ok)),
        CompleteOutcome::Queued
    );
    assert_eq!(
        ledger.complete(completion(11, second, Status::Ok)),
        CompleteOutcome::Backpressure
    );
    assert_eq!(
        ledger.cancel(second),
        Err(CancelError::Backpressure),
        "cancellation must not drop an in-flight request"
    );
    assert_eq!(ledger.in_flight(), 1);
    assert_eq!(ledger.cancelled(), 0);

    assert_eq!(ledger.pop().expect("first").tag, first);
    assert_eq!(
        ledger.complete(completion(11, second, Status::Ok)),
        CompleteOutcome::Queued
    );
    assert_eq!(ledger.pop().expect("second").tag, second);
    assert_eq!(ledger.in_flight(), 0);
    assert_eq!(ledger.pending_completions(), 0);
}

#[test]
fn late_completion_for_a_reused_tag_is_rejected() {
    let mut ledger = RequestLedger::new(2, 4);
    let first = ledger.begin(CompletionId(20)).expect("first");
    ledger.cancel(first).expect("cancel");
    assert_eq!(ledger.pop().expect("cancelled").tag, first);

    // The freed tag is the lowest free value, so the next request reuses it.
    let second = ledger.begin(CompletionId(21)).expect("second");
    assert_eq!(second, first);
    assert_eq!(
        ledger.complete(completion(20, first, Status::Ok)),
        CompleteOutcome::Unknown,
        "a stale generation must not terminate the new request"
    );
    assert_eq!(ledger.in_flight(), 1);
    assert_eq!(
        ledger.complete(completion(21, second, Status::Ok)),
        CompleteOutcome::Queued
    );
    assert_eq!(ledger.pop().expect("second").event, CompletionId(21));
}

#[test]
fn ten_thousand_tags_are_distinct_and_exhaustible() {
    const COUNT: u32 = 10_000;
    let mut tags = TagAllocator::new(COUNT);
    let mut seen = BTreeSet::new();
    for _ in 0..COUNT {
        let tag = tags.alloc().expect("tag below capacity");
        assert!(seen.insert(tag.0), "tag {tag:?} was allocated twice");
    }
    assert_eq!(seen.len(), COUNT as usize);
    assert_eq!(tags.in_flight(), COUNT as usize);
    assert_eq!(tags.alloc(), Err(TagError::Exhausted));

    // Free the even tags, then hand them out again. The allocator must reuse
    // exactly those values and stay exhausted once they are gone.
    for value in (2..=COUNT).step_by(2) {
        assert!(tags.free(Tag(value)));
    }
    assert_eq!(tags.in_flight(), (COUNT / 2) as usize);
    for _ in 0..COUNT / 2 {
        let tag = tags.alloc().expect("reused tag");
        assert_eq!(tag.0 % 2, 0);
    }
    assert_eq!(tags.alloc(), Err(TagError::Exhausted));
    assert_eq!(tags.in_flight(), COUNT as usize);
}

#[test]
fn ten_thousand_outstanding_requests_retire_once() {
    const COUNT: u32 = 10_000;
    let mut ledger = RequestLedger::new(COUNT, COUNT as usize);
    for index in 0..COUNT {
        ledger
            .begin(CompletionId(u64::from(index) + 1))
            .expect("tag below capacity");
    }
    assert_eq!(ledger.in_flight(), COUNT as usize);
    assert_eq!(ledger.capacity(), COUNT);

    // Complete in reverse order to prove the table is not a stack.
    for index in (0..COUNT).rev() {
        let tag = Tag(index + 1);
        assert_eq!(
            ledger.complete(completion(u64::from(index) + 1, tag, Status::Ok)),
            CompleteOutcome::Queued
        );
    }
    assert_eq!(ledger.in_flight(), 0);
    assert_eq!(ledger.pending_completions(), COUNT as usize);

    let mut popped = 0usize;
    while ledger.pop().is_some() {
        popped += 1;
    }
    assert_eq!(popped, COUNT as usize);
}
