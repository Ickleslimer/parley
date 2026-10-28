import copy
import ctypes
from ctypes import wintypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
CONFIG = Path(r'D:\AppData\Codex\UserProfile\.codex\config.toml')
PROFILE = 'parley-widget-dev'
HELPER = ROOT.parent / 'grok-image-lifecycle-repair/build/activate_candidate.py'
HELPER_HASH = 'b98f6d7782e59756b703e5b1238ca17a669730a4875c9bbd79118eabfaba81b8'
OLD_HASH = '08dbca8c5eb5c4e718b5a606af5a14210c45237d4796ed8a239d7643ed47f769'
SEED = ROOT.parents[2] / '.parley-evidence/grok-working-brief-20261023/brief-v2.json'
SEED_HASH = '3dae5a6b8b20bf9b327ebad8c0766948ad2abcba51dddd7b2056561e9373ef01'
SESSION = 'f09b6874-3b8e-875a-87ba-314c7400ae54'
CWD = ROOT.parent / 'grok-image-lifecycle-repair'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def create(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('xb') as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def record(path, value):
    create(path, json.dumps(value, indent=2).encode())


def source_hashes():
    paths = list((ROOT / 'src').rglob('*.rs')) + [ROOT / 'Cargo.toml', ROOT / 'Cargo.lock']
    return {str(path.relative_to(ROOT)).replace('\\', '/'): digest(path.read_bytes()) for path in paths}


def update_config(content, previous, destination):
    if digest(HELPER.read_bytes()) != HELPER_HASH:
        raise ValueError('reviewed_helper_changed')
    spec = importlib.util.spec_from_file_location('reviewed_activation', HELPER)
    helper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helper)
    before = tomllib.loads(content.decode())
    if 'PARLEY_WORKING_BRIEF_ENABLED' in before['mcp_servers'][PROFILE]['env']:
        raise ValueError('flag_already_present')
    updated = helper.updated_configuration(content, {PROFILE: (previous, destination)})
    pattern = rb'(?m)^\[mcp_servers\.parley-widget-dev\.env\][ \t]*\r?$'
    matches = list(re.finditer(pattern, updated))
    if len(matches) != 1:
        raise ValueError('env_header_ambiguous')
    position = matches[0].end()
    updated = updated[:position] + b'\nPARLEY_WORKING_BRIEF_ENABLED = "true"' + updated[position:]
    expected = copy.deepcopy(before)
    expected['mcp_servers'][PROFILE]['command'] = destination
    expected['mcp_servers'][PROFILE]['env']['PARLEY_WORKING_BRIEF_ENABLED'] = 'true'
    if tomllib.loads(updated.decode()) != expected:
        raise ValueError('unexpected_semantic_change')
    return updated


def normalized(path):
    value = str(path).replace('\\', '/')
    if value.startswith('//?/UNC/'):
        value = '//' + value[8:]
    elif value.startswith('//?/'):
        value = value[4:]
    return value.rstrip('/').lower()


def prepare_seed(environment):
    raw = SEED.read_bytes()
    if digest(raw) != SEED_HASH:
        raise ValueError('seed_changed')
    seed = json.loads(raw)
    if seed['session_id'] != SESSION or normalized(seed['cwd']) != normalized(CWD.resolve()):
        raise ValueError('seed_binding_mismatch')
    allowed = str(Path(environment['PARLEY_ALLOWED_CWD_ROOT']).resolve())
    if not allowed.startswith('\\\\?\\'):
        allowed = '\\\\?\\' + allowed
    identity = environment['PARLEY_CALLER'] + '\0' + allowed.rstrip('\\/').lower()
    hashed = 0xcbf29ce484222325
    for byte in identity.encode():
        hashed = ((hashed ^ byte) * 0x100000001b3) & ((1 << 64) - 1)
    binding = {'cwd': normalized(CWD.resolve()), 'profile': f'profile-{hashed:016x}', 'session_id': SESSION}
    key = digest(json.dumps(binding, separators=(',', ':'), sort_keys=True).encode())
    root = Path(environment['PARLEY_JOB_STATE_DIR']).resolve()
    if root != Path(r'C:\Users\mrdyl\AppData\Local\Parley\jobs').resolve():
        raise ValueError('job_root_changed')
    destination = root / 'working-brief' / key / 'seed.json'
    for parent in [destination, *destination.parents]:
        if parent.exists() and parent.lstat().st_file_attributes & 0x400:
            raise ValueError('redirected_seed_destination')
    return destination, json.dumps({'sha256': SEED_HASH, 'body': raw.decode()}, separators=(',', ':')).encode()


def stage():
    binary = (ROOT / 'target/release/par.exe').read_bytes()
    hashed = digest(binary)
    destination = ROOT / 'build' / f'par-automatic-brief-{hashed[:16]}.exe'
    create(destination, binary)
    value = {'candidate': str(destination), 'sha256': hashed, 'bytes': len(binary), 'sources': source_hashes()}
    record(ROOT / 'build/candidate.json', value)
    return {'candidate': str(destination), 'sha256': hashed, 'bytes': len(binary)}


def plan():
    candidate = json.loads((ROOT / 'build/candidate.json').read_bytes())
    if source_hashes() != candidate['sources'] or digest(Path(candidate['candidate']).read_bytes()) != candidate['sha256']:
        raise ValueError('candidate_or_sources_changed')
    content = CONFIG.read_bytes()
    parsed = tomllib.loads(content.decode())
    previous = Path(parsed['mcp_servers'][PROFILE]['command'])
    if digest(previous.read_bytes()) != OLD_HASH:
        raise ValueError('baseline_changed')
    destination = previous.with_name(Path(candidate['candidate']).name)
    updated = update_config(content, str(previous), str(destination))
    seed, _ = prepare_seed(parsed['mcp_servers'][PROFILE]['env'])
    value = {'before_sha256': digest(content), 'after_sha256': digest(updated), 'previous': str(previous),
             'destination': str(destination), 'candidate_sha256': candidate['sha256'], 'seed': str(seed),
             'backup': str(CONFIG.parent / 'config-backups/parley-automatic-brief-20261023.toml')}
    record(ROOT / 'build/activation-plan.json', value)
    return value


def apply():
    value = json.loads((ROOT / 'build/activation-plan.json').read_bytes())
    candidate = json.loads((ROOT / 'build/candidate.json').read_bytes())
    content = CONFIG.read_bytes()
    if digest(content) != value['before_sha256'] or source_hashes() != candidate['sources']:
        raise ValueError('configuration_or_source_drift')
    binary = Path(candidate['candidate']).read_bytes()
    if digest(binary) != value['candidate_sha256'] or digest(Path(value['previous']).read_bytes()) != OLD_HASH:
        raise ValueError('binary_drift')
    updated = update_config(content, value['previous'], value['destination'])
    if digest(updated) != value['after_sha256']:
        raise ValueError('plan_drift')
    temporary = CONFIG.with_name('config.automatic-brief-20261023.pending.toml')
    backup = Path(value['backup'])
    if temporary.exists() or backup.exists():
        raise ValueError('pending_or_backup_exists')
    destination = Path(value['destination'])
    if destination.exists():
        if digest(destination.read_bytes()) != value['candidate_sha256']:
            raise ValueError('destination_collision')
    else:
        create(destination, binary)
    parsed = tomllib.loads(content.decode())
    seed, seed_bytes = prepare_seed(parsed['mcp_servers'][PROFILE]['env'])
    if str(seed) != value['seed']:
        raise ValueError('seed_plan_drift')
    if seed.exists():
        if seed.read_bytes() != seed_bytes:
            raise ValueError('seed_collision')
    else:
        create(seed, seed_bytes)
    create(temporary, updated)
    replace = ctypes.WinDLL('kernel32', use_last_error=True).ReplaceFileW
    replace.argtypes = [wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.LPCWSTR, wintypes.DWORD, wintypes.LPVOID, wintypes.LPVOID]
    replace.restype = wintypes.BOOL
    if not replace(str(CONFIG), str(temporary), str(backup), 0, None, None):
        raise ctypes.WinError(ctypes.get_last_error())
    if CONFIG.read_bytes() != updated or digest(backup.read_bytes()) != value['before_sha256']:
        raise ValueError('readback_mismatch')
    record(ROOT / 'build/activation-applied.json', {**value, 'status': 'host_reload_pending'})
    return {'status': 'host_reload_pending', 'candidate_sha256': value['candidate_sha256'], 'only_command_and_feature_flag_changed': True}


if __name__ == '__main__':
    if len(sys.argv) != 2 or sys.argv[1] not in ('stage', 'plan', 'apply'):
        raise SystemExit('use stage, plan or apply')
    print(json.dumps(globals()[sys.argv[1]]()))
