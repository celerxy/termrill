"""显式授权的真实服务器对照：独占随机临时目录，测完只清理本次测试文件。"""
import argparse
import base64
import hashlib
import importlib.util
import json
import logging
import os
from pathlib import Path
import posixpath
import shutil
import statistics
import time
import uuid

import paramiko

PROJECT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('benchmark_support', PROJECT / 'scripts/benchmark-transfer.py')
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)
TEST = 'server_benchmark_tests::authorized_download'


def connect(endpoint, fingerprint):
    class Pinned(paramiko.MissingHostKeyPolicy):
        def missing_host_key(self, client, hostname, key):
            actual = 'SHA256:' + base64.b64encode(hashlib.sha256(key.asbytes()).digest()).decode().rstrip('=')
            if actual != fingerprint:
                raise paramiko.SSHException('Saved fingerprint mismatch')
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(Pinned())
    try:
        client.connect(endpoint['host'], port=endpoint['port'], username=endpoint['username'],
                       key_filename=endpoint['keyPath'], look_for_keys=False, allow_agent=False,
                       timeout=10, banner_timeout=10, auth_timeout=10)
        return client
    except BaseException:
        client.close()
        raise


def verify(path, expected):
    if path.is_symlink() or path.stat().st_size != 8 * 1024 * 1024:
        raise RuntimeError('Downloaded file length or type was invalid')
    with path.open('rb') as stream:
        if hashlib.file_digest(stream, 'sha256').hexdigest() != expected:
            raise RuntimeError('Downloaded content did not match')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', type=Path, required=True)
    parser.add_argument('--profile-id', required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--allow-remote-write', action='store_true', required=True)
    parser.add_argument('--rounds', type=int, choices=(1, 2, 3), default=3)
    parser.add_argument('--concurrency', type=int, choices=(1, 2, 3, 4), default=3)
    parser.add_argument('--no-compression', action='store_true', help='Test-only native compression override')
    args = parser.parse_args()
    logging.getLogger('paramiko').handlers = [logging.NullHandler()]
    logging.getLogger('paramiko').propagate = False
    config = json.loads(args.config.read_text(encoding='utf-8-sig'))
    profiles = [item for item in config['profiles'] if item['id'] == args.profile_id]
    if len(profiles) != 1 or profiles[0].get('jump') or profiles[0]['target']['auth'] != 'key':
        parser.error('Select exactly one directly connected key-authenticated profile')
    profile = {'id': 'authorized-benchmark', 'name': 'Authorized benchmark', 'jump': None,
               'target': {key: profiles[0]['target'][key] for key in ('host', 'port', 'username', 'auth', 'keyPath')}}
    endpoint = profile['target']
    expected_host = config['knownHosts'].get(f"[{endpoint['host']}]:{endpoint['port']}")
    if not expected_host:
        parser.error('An existing trusted host fingerprint is required')
    binary = args.binary.resolve()
    if not binary.is_file() or not binary.is_relative_to(PROJECT / 'src-tauri/target'):
        parser.error('Current library test executable must be inside this project target directory')
    token = uuid.uuid4().hex
    base = PROJECT / 'tests/.runtime-server-benchmark'
    base.mkdir(exist_ok=True)
    runtime = base / ('run-' + token)
    runtime.mkdir()
    samples = []
    remote_directory = None
    remote_file = None
    created = False
    cleanup_complete = False
    cleanup_record = base / ('cleanup-' + token + '.json')
    try:
        payload = os.urandom(8 * 1024 * 1024)
        expected = hashlib.sha256(payload).hexdigest()
        client = connect(endpoint, expected_host)
        try:
            with client.open_sftp() as sftp:
                sftp.get_channel().settimeout(15)
                home = sftp.normalize('.')
                if not home.startswith('/') or any(value in home for value in ('\r', '\n', '"', '\\')):
                    raise RuntimeError('Remote home cannot be safely represented in the SFTP batch')
                remote_directory = posixpath.join(home, '.termrill-speed-' + token)
                remote_file = posixpath.join(remote_directory, 'payload.bin')
                sftp.mkdir(remote_directory, mode=0o700)
                created = True
                cleanup_record.write_text(json.dumps({'profileId': args.profile_id, 'remoteDirectory': remote_directory,
                                                       'remoteFile': remote_file, 'ownerToken': token}), encoding='utf-8')
                deadline = time.monotonic() + 60
                with sftp.open(remote_file, 'wx') as target:
                    target.set_pipelined(True)
                    for offset in range(0, len(payload), 128 * 1024):
                        if time.monotonic() >= deadline:
                            raise RuntimeError('Test upload preparation timed out')
                        target.write(payload[offset:offset + 128 * 1024])
                if sftp.lstat(remote_file).st_size != len(payload):
                    raise RuntimeError('Test upload preparation failed')
                print('Random 8 MiB remote test file prepared', flush=True)
            key = client.get_transport().get_remote_server_key()
            text = key.get_name() + ' ' + key.get_base64()
            host = endpoint['host']
            (runtime / 'known_hosts').write_text(f'{host} {text}\n[{host}]:{endpoint["port"]} {text}\n', encoding='utf-8')
        finally:
            client.close()
        (runtime / 'empty_config').write_text('', encoding='utf-8')
        for index in range(args.rounds):
            order = ('native', 'openssh') if index % 2 == 0 else ('openssh', 'native')
            for name in order:
                if name == 'native':
                    manifest = {'profile': profile, 'fingerprint': expected_host, 'remote': remote_file,
                                'runtime': str(runtime), 'sample': index, 'concurrency': args.concurrency}
                    manifest_path = runtime / 'manifest.json'
                    manifest_path.write_text(json.dumps(manifest), encoding='utf-8')
                    env = {**os.environ, 'TERMRILL_SERVER_BENCHMARK_MANIFEST': str(manifest_path), 'TERMRILL_TEST_MANIFEST': '1'}
                    if args.no_compression:
                        env['TERMRILL_KVM_NO_COMPRESSION'] = '1'
                    arguments = [str(binary), '--exact', TEST, '--ignored', '--test-threads=1']
                    batch = None
                    output = runtime / f'native-{index}.bin'
                else:
                    env = None
                    arguments = ['sftp', '-F', str(runtime / 'empty_config'), '-b', '-', '-P', str(endpoint['port']),
                                 '-i', endpoint['keyPath'], '-B32768', '-R64', '-oBatchMode=yes', '-oIdentitiesOnly=yes',
                                 '-oIdentityAgent=none', '-oStrictHostKeyChecking=yes', '-oUser=' + endpoint['username'],
                                 '-oUserKnownHostsFile=' + str(runtime / 'known_hosts'),
                                 '-oGlobalKnownHostsFile=' + str(runtime / 'known_hosts'), '-oCompression=no',
                                 '-oControlMaster=no', '-oControlPath=none', endpoint['host']]
                    batch = f'get "{remote_file}" "openssh-{index}.bin"\n'
                    output = runtime / f'openssh-{index}.bin'
                started = time.monotonic()
                code, stdout, stderr = support.run_process(arguments, cwd=runtime, env=env, input_text=batch, timeout=180)
                elapsed = time.monotonic() - started
                if code:
                    (base / 'last-failure.log').write_text(stdout + stderr, encoding='utf-8')
                    raise RuntimeError('Comparison client failed; local private log retained')
                verify(output, expected)
                result = {'client': name, 'round': index + 1, 'elapsedSeconds': elapsed,
                          'mibPerSecond': 8 / elapsed, 'bytes': len(payload), 'verified': True}
                if name == 'native':
                    phases = json.loads((runtime / f'native-{index}.json').read_text())
                    result.update({key: phases[key] for key in ('connectMs', 'transferMs')})
                    log_path = runtime / f'performance-{index}/performance.log'
                    if log_path.exists():
                        shutil.copyfile(log_path, base / f'phases-{token}-{index}.log')
                samples.append(result)
                print(f'{name} round {index + 1}: {8 / elapsed:.2f} MiB/s; SHA256 verified', flush=True)
    finally:
        try:
            if created:
                # 仅删除本次成功创建的固定文件与空目录，不递归删除、不清理别人的文件。
                client = connect(endpoint, expected_host)
                try:
                    with client.open_sftp() as sftp:
                        sftp.get_channel().settimeout(15)
                        try:
                            sftp.remove(remote_file)
                        except FileNotFoundError:
                            pass
                        sftp.rmdir(remote_directory)
                        cleanup_complete = True
                        cleanup_record.unlink(missing_ok=True)
                        print('Owned remote test file and directory removed', flush=True)
                finally:
                    client.close()
        finally:
            if runtime.resolve().parent != base.resolve() or runtime.name != 'run-' + token:
                raise RuntimeError('Refuse unexpected local cleanup target')
            # 失败或超时也保留数值阶段日志；正文、配置与公钥缓存仍随本次目录清理。
            for log_path in runtime.glob('performance-*/performance.log'):
                try:
                    shutil.copyfile(log_path, base / f'phases-{token}-{log_path.parent.name}.log')
                except OSError:
                    pass
            shutil.rmtree(runtime)
    summary = {name: statistics.median(row['mibPerSecond'] for row in samples if row['client'] == name)
               for name in ('native', 'openssh')}
    report = {'scope': 'authorized-server-current-route', 'routeVerified': False, 'remoteCleanupComplete': cleanup_complete,
              'nativeConcurrency': args.concurrency, 'nativeCompressionDisabled': args.no_compression,
              'sizeMiB': 8, 'buildProfile': 'debug-application-test', 'samples': samples, 'medianMiBPerSecond': summary,
              'limitations': ['Current operating-system route only', 'No automatic TUN or proxy changes',
                              'Application debug test build; not published release performance']}
    (base / ('result-' + token + '.json')).write_text(json.dumps(report, indent=2), encoding='utf-8')
    (base / 'latest.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(summary), flush=True)


if __name__ == '__main__':
    main()
