"""对生产下载与持久化执行跨进程中断验收；只操作本机临时 SSH fixture。"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time
import uuid

PROJECT = Path(__file__).resolve().parents[1]
TESTS = PROJECT / 'tests'
PHASE = 'process_recovery_tests::process_recovery_phase'
spec = importlib.util.spec_from_file_location('benchmark_support', PROJECT / 'scripts/benchmark-transfer.py')
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def cleanup(runtime):
    resolved = runtime.resolve()
    if resolved.parent != TESTS.resolve() or not re.fullmatch(r'\.runtime-restart-[a-f0-9]{32}', resolved.name):
        raise RuntimeError('Refuse unexpected cleanup directory')
    shutil.rmtree(resolved)


def run_case(binary, case):
    runtime = TESTS / ('.runtime-restart-' + uuid.uuid4().hex)
    runtime.mkdir()
    server = log = child = None
    child_log = None
    try:
        server, log, _ = support.start_fixture(runtime)
        source = runtime / 'remote/acceptance.bin'
        source.write_bytes(os.urandom(8 * 1024 * 1024))
        expected = digest(source)
        (runtime / 'faults.json').write_text(json.dumps({'readDelayMs': 10}), encoding='utf-8')
        env = {**os.environ, 'TERMRILL_TEST_MANIFEST': '1', 'TERMRILL_TEST_FIXTURE': str(runtime / 'ready.json'),
               'TERMRILL_RESTART_CASE': case, 'TERMRILL_RESTART_PHASE': 'prepare'}
        arguments = [str(binary), '--exact', PHASE, '--ignored', '--test-threads=1', '--nocapture']
        child_log = (runtime / 'child.log').open('w', encoding='utf-8')
        options = {'creationflags': subprocess.CREATE_NEW_PROCESS_GROUP} if os.name == 'nt' else {'start_new_session': True}
        child = subprocess.Popen(arguments, cwd=PROJECT, env=env, stdout=child_log, stderr=child_log, **options)
        case_root = runtime / case
        marker = 'disconnected.ready' if case == 'disconnect' else 'interrupted.ready'
        deadline = time.monotonic() + 30
        while not (case_root / marker).exists():
            if child.poll() is not None or time.monotonic() >= deadline:
                raise RuntimeError('Child did not reach the required interruption point')
            time.sleep(0.05)
        if case == 'disconnect':
            if child.wait(timeout=15) != 0:
                raise RuntimeError('Disconnected child failed')
        else:
            support.stop_process_tree(child)
            child.wait(timeout=10)
            if child.returncode == 0:
                raise RuntimeError('Child was not forcibly interrupted')
        child_log.close()
        child_log = None
        target = case_root / 'download.bin'
        if target.read_bytes() != b'original fixture destination':
            raise RuntimeError('Original destination changed before completion')
        partials = list(case_root.glob('.termrill-*'))
        partials = [path for path in partials if path.is_file()]
        if len(partials) != 1 or not 0 < partials[0].stat().st_size < source.stat().st_size:
            raise RuntimeError('Expected a non-empty, incomplete resumable file')
        partial_bytes = partials[0].stat().st_size
        snapshot = (case_root / 'transfers.json').read_text(encoding='utf-8')
        if any(field in snapshot for field in ('password', 'passphrase', 'fixture-only-password', 'PRIVATE KEY')):
            raise RuntimeError('Credentials leaked into the task snapshot')
        if case == 'changed-source':
            with source.open('r+b') as changed:
                first = changed.read(1)
                changed.seek(0)
                changed.write(bytes([first[0] ^ 255]))
        env['TERMRILL_RESTART_PHASE'] = {'kill-discard': 'discard', 'changed-source': 'reject-changed'}.get(case, 'resume')
        code, stdout, stderr = support.run_process(arguments, cwd=PROJECT, env=env, timeout=60)
        if code:
            (runtime / 'child.log').write_text(stdout + stderr, encoding='utf-8')
            raise RuntimeError('Recovery child failed; private local log retained')
        result = json.loads((case_root / 'result.json').read_text(encoding='utf-8'))
        if case in ('kill-resume', 'disconnect'):
            if digest(target) != expected or result['bytes'] != source.stat().st_size:
                raise RuntimeError('Resumed content did not match the source')
        else:
            if target.read_bytes() != b'original fixture destination':
                raise RuntimeError('Recovery replaced the protected original')
        return {'case': case, 'passed': True, 'partialBytes': partial_bytes, **result}
    except BaseException:
        failures = TESTS / '.runtime-process-recovery-results'
        failures.mkdir(exist_ok=True)
        if child_log:
            child_log.flush()
        path = runtime / 'child.log'
        if path.exists():
            (failures / 'last-failure.log').write_bytes(path.read_bytes()[-65536:])
        raise
    finally:
        if child is not None:
            support.stop_process_tree(child)
            child.wait(timeout=10)
        if child_log:
            child_log.close()
        if server is not None:
            if server.poll() is None:
                server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=10)
        if log:
            log.close()
        cleanup(runtime)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True, help='Current library test executable')
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file() or not binary.is_relative_to(PROJECT / 'src-tauri/target'):
        parser.error('Test executable must exist inside this project target directory')
    results = []
    for case in ('kill-resume', 'kill-discard', 'changed-source', 'disconnect'):
        results.append(run_case(binary, case))
        print(case + ': passed', flush=True)
    output = TESTS / '.runtime-process-recovery-results'
    output.mkdir(exist_ok=True)
    report = {'scope': 'loopback-native-process-test', 'cases': results,
              'limitations': ['Not an installer upgrade test', 'Not a real operating-system sleep or power-loss test']}
    (output / 'latest.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print('All process recovery cases passed', flush=True)


if __name__ == '__main__':
    main()
