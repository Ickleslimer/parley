import copy
import hashlib
import json
from pathlib import Path
import sys
import tomllib

from promote_working_brief import CONFIG, PROFILE, ROOT, SEED_HASH, source_hashes


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def verify(evidence):
    plan = json.loads((ROOT / 'build/activation-plan.json').read_bytes())
    candidate = json.loads((ROOT / 'build/candidate.json').read_bytes())
    before = tomllib.loads(Path(plan['backup']).read_text())
    after = tomllib.loads(CONFIG.read_text())
    expected = copy.deepcopy(before)
    expected['mcp_servers'][PROFILE]['command'] = plan['destination']
    expected['mcp_servers'][PROFILE]['env']['PARLEY_WORKING_BRIEF_ENABLED'] = 'true'
    assert after == expected, 'unexpected_profile_change'
    assert source_hashes() == candidate['sources'], 'source_drift'
    assert digest(Path(plan['destination']).read_bytes()) == candidate['sha256'], 'binary_drift'
    directory = Path(plan['seed']).parent
    seed = json.loads(Path(plan['seed']).read_bytes())
    assert seed['sha256'] == SEED_HASH == digest(seed['body'].encode()), 'seed_drift'
    environment = after['mcp_servers'][PROFILE]['env']
    with Path(environment['PARLEY_EVENT_LOG']).open('rb') as stream:
        length = stream.seek(0, 2)
        offset = max(0, length - 4 * 1024 * 1024)
        stream.seek(offset)
        if offset:
            stream.readline()
        records = [json.loads(line) for line in stream if line.strip()]
    events = {item['event_id']: item for item in records}
    head = json.loads((directory / 'head.json').read_bytes())
    assert head['status'] == 'updated', head['status']
    previous = ''
    checks = []
    for number in (1, 2):
        job = json.loads((evidence / f'LIVE-CANARY-{number}-TERMINAL.json').read_bytes())
        original = json.loads((evidence / f'LIVE-CANARY-{number}-REQUEST.json').read_bytes())
        assert job['state'] == 'succeeded' and job['handoff']['receipt_state'] == 'received', 'job_not_successful'
        request = events[job['request_event_id']]
        reply = events[job['completion_event_id']]
        assert request['content'].startswith(original['prompt']), 'current_prompt_changed'
        assert 'MODEL-GENERATED HISTORICAL CONTEXT:' not in original['prompt'], 'manual_injection'
        assert 'PARLEY_WORKING_BRIEF_V1 ' not in original['prompt'], 'manual_template'
        injected = request['content'][len(original['prompt']):]
        assert 'MODEL-GENERATED HISTORICAL CONTEXT:' in injected and SEED_HASH in injected, 'not_injected'
        assert reply['event_type'] == 'response' and reply['status'] == 'ok', 'reply_not_saved'
        matches = []
        for path in directory.glob('report-*.json'):
            raw = path.read_bytes()
            record = json.loads(raw)
            if record['exchange_id'] == job['exchange_id']:
                assert path.stem == 'report-' + digest(raw), 'report_hash_mismatch'
                matches.append((digest(raw), record))
        assert len(matches) == 1, 'report_identity_ambiguous'
        hashed, report = matches[0]
        assert report['request_event_id'] == request['event_id'], 'request_id_mismatch'
        assert report['completion_event_id'] == reply['event_id'], 'reply_id_mismatch'
        assert report['reply_sha256'] == digest(reply['content'].encode()), 'reply_hash_mismatch'
        assert report['based_on_sha256'] == previous, 'chain_mismatch'
        assert report['authority'] == 'model_generated_historical_context', 'authority_changed'
        proposal = json.loads(report['proposal'])
        assert proposal['prior_sha256'] == previous, 'proposal_chain_mismatch'
        if number == 1:
            assert 'ceramic comet' in report['proposal'].lower() and 'teal' in report['proposal'].lower(), 'synthetic_reminder_not_carried'
        else:
            assert previous in injected and 'ceramic comet' in injected.lower() and 'teal' in injected.lower(), 'prior_not_injected'
            assert 'ceramic comet' not in original['prompt'].lower() and 'teal' not in original['prompt'].lower(), 'manual_fact_replay'
        checks.append({'job_id': job['job_id'], 'exchange_id': job['exchange_id'], 'request_event_id': request['event_id'], 'completion_event_id': reply['event_id'], 'report_sha256': hashed, 'reply_sha256': report['reply_sha256'], 'current_prompt_unchanged': True, 'automatically_injected': True})
        previous = hashed
    assert head['report_sha256'] == previous, 'head_not_latest'
    return {'result': 'PASS', 'candidate_sha256': candidate['sha256'], 'seed_sha256': SEED_HASH, 'source_manifest_matches': True, 'only_selected_profile_changed': True, 'binding': head['binding'], 'head_sha256': digest((directory / 'head.json').read_bytes()), 'checks': checks}


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit('usage: verify_working_brief_live.py EVIDENCE_DIRECTORY')
    print(json.dumps(verify(Path(sys.argv[1])), indent=2))
