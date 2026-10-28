import hashlib
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
JOURNAL = Path(r"C:\Users\mrdyl\AppData\Local\Parley\context\f09b6874-3b8e-875a-87ba-314c7400ae54\codex\01a0b89d-0117-7c41-86f5-569b371278f4\journal.jsonl")
MARKER = "history-check-7c9e4b2a61d8"


def rows(content):
    return [json.loads(line) for line in content.decode("utf-8-sig").splitlines() if line]


def main():
    before = (ROOT / "build/f09-journal-before.jsonl").read_bytes()
    after = JOURNAL.read_bytes()
    if not after.startswith(before):
        raise ValueError("original_journal_records_changed")
    previous = rows(before)[-1]
    appended = rows(after[len(before):])
    if len(appended) != 2 or [row["state"] for row in appended] != ["in_flight", "committed"]:
        raise ValueError("unexpected_cursor_transition")
    for row in appended:
        if row["schema_version"] != 2 or row["start_offset"] != previous["end_offset"]:
            raise ValueError("cursor_was_reset_or_skipped")
        parts = row["source_identity"]["history"]
        if len(parts) != 2 or parts[0]["physical_start"] != 0 or parts[0]["logical_start"] != 0:
            raise ValueError("unexpected_lineage_geometry")
        if parts[0]["physical_end"] != 3222474611 or parts[1]["logical_start"] != 3222474611:
            raise ValueError("retained_prefix_boundary_changed")
        if not row["start_offset"] < parts[1]["logical_start"] < row["end_offset"]:
            raise ValueError("canary_did_not_cross_retained_prefix_gap")
        if row["target_session"] != "f09b6874-3b8e-875a-87ba-314c7400ae54":
            raise ValueError("native_conversation_changed")
    if appended[0]["source_identity"] != appended[1]["source_identity"]:
        raise ValueError("lineage_changed_during_delivery")
    status = json.loads((ROOT / "build/canonical-canary-terminal.json").read_bytes())
    if (status["state"] != "succeeded" or status["handoff"]["receipt_state"] != "received"
            or MARKER not in status["handoff"]["report"]):
        raise ValueError("canonical_canary_not_verified")
    record = {"originalJournalPrefixUnchanged": True, "onlyExpectedAppendPair": True,
              "existingNativeSessionPreserved": True, "schema": 2,
              "startOffset": appended[-1]["start_offset"], "endOffset": appended[-1]["end_offset"],
              "retainedPrefixBoundary": 3222474611, "markerObserved": MARKER,
              "recordCount": appended[-1]["record_count"],
              "characterCount": appended[-1]["character_count"],
              "journalBeforeSha256": hashlib.sha256(before).hexdigest(),
              "journalAfterSha256": hashlib.sha256(after).hexdigest()}
    with (ROOT / "build/canonical-cursor-verification.json").open("x", encoding="utf-8") as destination:
        json.dump(record, destination, indent=2)
    print(json.dumps(record))


if __name__ == "__main__":
    main()
