import argparse
import hashlib
import json
from pathlib import Path

MAX_CALLER_BYTES = 16_000
DATA_SEPARATOR = '\n\nINLINE EXACT JSON DATA:\n'


def prepare(header, payload):
    if chr(64) in header:
        raise ValueError('header_contains_mention_character')
    data = json.dumps(payload, ensure_ascii=True, separators=(',', ':')).replace(chr(64), r'\u0040')
    packet = header.rstrip() + DATA_SEPARATOR + data + '\n'
    if len(packet.encode('utf-8')) > MAX_CALLER_BYTES:
        raise ValueError('caller_budget_exceeded_split_whole_files_or_evidence_groups')
    return packet


def write_packet(header_path, payload_path, output_path):
    header = header_path.read_text(encoding='utf-8')
    payload = json.loads(payload_path.read_text(encoding='utf-8'))
    packet = prepare(header, payload)
    with output_path.open('x', encoding='utf-8', newline='\n') as target:
        target.write(packet)
    encoded = packet.encode('utf-8')
    return {'bytes': len(encoded), 'sha256': hashlib.sha256(encoded).hexdigest(), 'path': str(output_path)}


def main():
    parser = argparse.ArgumentParser(description='Prepare bounded inline review data; never launches an agent.')
    parser.add_argument('--header', type=Path, required=True)
    parser.add_argument('--payload', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    options = parser.parse_args()
    try:
        result = write_packet(options.header, options.payload, options.output)
    except (OSError, ValueError) as error:
        reason = str(error) if type(error) is ValueError else 'input_or_output_error'
        print(json.dumps({'status': 'refused', 'reason': reason}))
        return 1
    print(json.dumps({'status': 'prepared_not_sent', **result}))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
