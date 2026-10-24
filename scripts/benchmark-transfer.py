"""回环下载对照：生产下载流程与 OpenSSH SFTP；不修改系统代理，不连接真实服务器。"""
import argparse
import base64
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import sys
import time
import uuid

PROJECT = Path(__file__).resolve().parents[1]
TESTS = PROJECT / 'tests'
BENCHMARK = 'reliability_tests::benchmark::loopback_download_benchmark'


def stop_process_tree(process):
    if process.poll() is not None:
        return
    if os.name == 'nt':
        subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'], capture_output=True, check=False)
    else:
        import signal
        os.killpg(process.pid, signal.SIGKILL)


def run_process(arguments, *, cwd, env=None, input_text=None, timeout=180):
    """独占子进程；超时关闭本次创建的进程树，不遗留下载或编译子进程。"""
    options = {'creationflags': subprocess.CREATE_NEW_PROCESS_GROUP} if os.name == 'nt' else {'start_new_session': True}
    with subprocess.Popen(arguments, cwd=cwd, env=env, stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                          encoding='utf-8', errors='replace', **options) as process:
        try:
            stdout, stderr = process.communicate(input_text, timeout=timeout)
        except subprocess.TimeoutExpired:
            stop_process_tree(process)
            process.communicate()
            raise RuntimeError('Benchmark subprocess timed out') from None
        except BaseException:
            stop_process_tree(process)
            process.communicate()
            raise
        return process.returncode, stdout, stderr


def build_runner(profile, runtime):
    arguments = ['cargo', 'test', '--locked', '--offline', '--manifest-path',
                 str(PROJECT / 'src-tauri/Cargo.toml'), '--lib', '--no-run', '--message-format=json']
    if profile == 'release':
        arguments.append('--release')
    # 基准入口不需要开发期增量编译；避免单个测试二进制产生多 GiB 增量缓存。
    env = dict(os.environ, TERMRILL_TEST_MANIFEST='1', CARGO_INCREMENTAL='0')
    code, stdout, stderr = run_process(arguments, cwd=PROJECT, env=env, timeout=1800)
    (runtime / 'build.log').write_text(stderr, encoding='utf-8')
    if code:
        raise RuntimeError('Benchmark test binary build failed')
    executables = []
    for line in stdout.splitlines():
        event = json.loads(line)
        if event.get('reason') == 'compiler-artifact' and event.get('executable'):
            executables.append(event['executable'])
    if len(executables) != 1:
        raise RuntimeError('Expected exactly one library test executable')
    return executables[0]


def start_fixture(runtime):
    log = (runtime / 'fixture.log').open('w', encoding='utf-8')
    process = None
    try:
        process = subprocess.Popen([sys.executable, str(TESTS / 'ssh_fixture.py'), '--root', str(runtime)],
                                   stdout=log, stderr=log, cwd=PROJECT)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            ready = runtime / 'ready.json'
            if ready.exists():
                return process, log, json.loads(ready.read_text(encoding='utf-8'))
            if process.poll() is not None:
                break
            time.sleep(0.1)
        raise RuntimeError('Loopback fixture did not become ready')
    except BaseException:
        try:
            if process and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        finally:
            log.close()
        raise


def prepare_source(runtime, info, size_mib, read_delay_ms):
    if shutil.disk_usage(runtime).free < (size_mib * 3 + 32) * 1024 * 1024:
        raise RuntimeError('Not enough free space for source and verified download copies')
    protect_test_key(runtime)
    (runtime / 'faults.json').write_text(json.dumps({'readDelayMs': read_delay_ms}), encoding='utf-8')
    path = runtime / 'remote/benchmark.bin'
    digest = hashlib.sha256()
    with path.open('xb') as output:
        for _ in range(size_mib):
            # 随机正文避免可压缩的全零样本；两客户端始终读取同一份文件。
            block = os.urandom(1024 * 1024)
            output.write(block)
            digest.update(block)
    known_host = f'[127.0.0.1]:{info["targetPort"]} {info["targetHostKey"]}\n'
    (runtime / 'known_hosts').write_text(known_host, encoding='ascii')
    (runtime / 'empty_config').write_text('', encoding='ascii')
    return digest.hexdigest()


def protect_test_key(runtime):
    """Windows OpenSSH 拒绝继承的其他用户权限；只重设本次生成密钥的 ACL。"""
    key = runtime / 'client-key'
    if key.is_symlink() or key.resolve().parent != runtime.resolve():
        raise ValueError('Test key is outside benchmark runtime')
    if os.name != 'nt':
        key.chmod(0o600)
        return
    script = '''$ErrorActionPreference = 'Stop'
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = [System.Security.AccessControl.FileSecurity]::new()
$acl.SetOwner($sid)
$acl.SetAccessRuleProtection($true, $false)
$rule = [System.Security.AccessControl.FileSystemAccessRule]::new($sid, 'FullControl', 'Allow')
$acl.AddAccessRule($rule)
[System.IO.File]::SetAccessControl($env:TERMRILL_BENCHMARK_KEY, $acl)
'''
    encoded = base64.b64encode(script.encode('utf-16le')).decode('ascii')
    code, stdout, stderr = run_process(['powershell.exe', '-NoProfile', '-NonInteractive', '-EncodedCommand', encoded],
                            cwd=runtime, env=dict(os.environ, TERMRILL_BENCHMARK_KEY=str(key)), timeout=20)
    (runtime / 'key-acl.log').write_text(stdout + stderr, encoding='utf-8')
    if code:
        raise RuntimeError('Could not restrict temporary test-key permissions')


def checked_download(path, expected_bytes, expected_sha):
    if path.is_symlink() or not path.is_file() or path.stat().st_size != expected_bytes:
        raise ValueError('Downloaded file type or length mismatch')
    with path.open('rb') as stream:
        actual = hashlib.file_digest(stream, 'sha256').hexdigest()
    if actual != expected_sha:
        raise ValueError('Downloaded SHA256 mismatch')
    return actual


def checked_phases(details, size):
    if details.get('bytes') != size:
        raise ValueError('TermRill progress length mismatch')
    result = {}
    for field in ['connectMs', 'transferMs', 'maxProgressGapMs']:
        value = details.get(field)
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
            raise ValueError('Invalid phase timing')
        result[field] = value
    return result


def sample(client, case, concurrency, runner, sftp, runtime, info, args, expected_sha):
    # Rust fixture 在退出时清理故障设置；每个客户端开始前都恢复相同延迟，避免基线不对等。
    (runtime / 'faults.json').write_text(json.dumps({'readDelayMs': args.read_delay_ms}), encoding='utf-8')
    env = dict(os.environ, TERMRILL_TEST_MANIFEST='1', TERMRILL_TEST_FIXTURE=str(runtime / 'ready.json'),
               TERMRILL_BENCHMARK_CASE=str(case), TERMRILL_BENCHMARK_CONCURRENCY=str(concurrency))
    name = f'{client}-{case}.bin'
    if client == 'termrill':
        command = [runner, '--exact', BENCHMARK, '--ignored', '--test-threads=1']
        input_text = None
    else:
        command = [sftp, '-F', 'empty_config', '-b', '-', '-P', str(info['targetPort']),
                   '-i', info['keyPath'], '-B', '32768', '-R', '64',
                   '-oBatchMode=yes', '-oIdentitiesOnly=yes', '-oIdentityAgent=none',
                   '-oStrictHostKeyChecking=yes', '-oUserKnownHostsFile=known_hosts',
                   '-oGlobalKnownHostsFile=known_hosts', '-oCompression=no',
                   '-oControlMaster=no', '-oControlPath=none', 'termrill-test@127.0.0.1']
        # 批处理只使用固定远端名和程序生成的本地 basename，不插入用户路径或 shell。
        input_text = f'get "/benchmark.bin" "{name}"\n'
    started = time.perf_counter()
    code, stdout, stderr = run_process(command, cwd=runtime, env=env, input_text=input_text, timeout=args.timeout)
    elapsed = time.perf_counter() - started
    (runtime / f'{client}-{case}.log').write_text(stdout + stderr, encoding='utf-8')
    if code:
        raise RuntimeError(f'{client} sample failed')
    size = args.size_mib * 1024 * 1024
    digest = checked_download(runtime / name, size, expected_sha)
    result = {'client': client, 'round': case, 'termrillConcurrency': concurrency,
              'elapsedSeconds': elapsed, 'mibPerSecond': size / elapsed / (1024 * 1024),
              'bytes': size, 'sha256': digest, 'verified': True}
    if client == 'termrill':
        details = json.loads((runtime / f'termrill-{case}.json').read_text(encoding='utf-8'))
        result['phases'] = checked_phases(details, size)
    # 仅删除已逐字节摘要校验的本次下载副本；源文件保留给下一客户端。
    (runtime / name).unlink()
    return result


def summarize(samples):
    groups = sorted({(sample['client'], sample['termrillConcurrency']) for sample in samples})
    result = []
    for client, concurrency in groups:
        values = [sample['mibPerSecond'] for sample in samples
                  if sample['client'] == client and sample['termrillConcurrency'] == concurrency]
        result.append({'client': client, 'termrillConcurrency': concurrency,
                       'samples': len(values), 'medianMiBPerSecond': statistics.median(values)})
    return result


def cleanup(runtime):
    # Windows 上递归删除前核对最终绝对路径；只清理本次创建的独立目录。
    resolved = runtime.resolve()
    if resolved.parent != TESTS.resolve() or not re.fullmatch(r'\.runtime-benchmark-[a-f0-9]{32}', resolved.name):
        raise ValueError('Refusing cleanup outside benchmark runtime')
    shutil.rmtree(resolved)


def preserve_failure_logs(runtime, session):
    """只保留本次进程输出，不复制密钥、配置、正文和已下载文件。"""
    destination = TESTS / '.runtime-benchmark-results'
    destination.mkdir(exist_ok=True)
    with (destination / f'failure-{session}.log').open('x', encoding='utf-8') as output:
        for path in sorted(runtime.glob('*.log')):
            output.write(f'[{path.name}]\n')
            with path.open(encoding='utf-8', errors='replace') as source:
                output.write(source.read(1024 * 1024))
            output.write('\n')


def benchmark(args):
    sftp = shutil.which('sftp')
    if not sftp:
        raise RuntimeError('OpenSSH sftp is required')
    session = uuid.uuid4().hex
    runtime = TESTS / ('.runtime-benchmark-' + session)
    runtime.mkdir()
    output = args.output or TESTS / '.runtime-benchmark-results' / f'{session}.json'
    output = output.resolve()
    if not output.is_relative_to(PROJECT) or output.exists():
        cleanup(runtime)
        raise ValueError('Report must be a new file inside this repository')
    fixture = log = None
    try:
        runner = build_runner(args.build_profile, runtime)
        fixture, log, info = start_fixture(runtime)
        expected = prepare_source(runtime, info, args.size_mib, args.read_delay_ms)
        samples = []
        for concurrency in args.concurrency:
            for index in range(args.rounds):
                case = len(samples) // 2
                order = ['termrill', 'openssh'] if index % 2 == 0 else ['openssh', 'termrill']
                for client in order:
                    result = sample(client, case, concurrency, runner, sftp, runtime, info, args, expected)
                    samples.append(result)
                    print(f'{client} termrill-config-concurrency={concurrency}: {result["mibPerSecond"]:.2f} MiB/s (SHA256 verified)', flush=True)
        report = {'schemaVersion': 1, 'scope': 'loopback-only', 'termrillEngine': 'russh-native-download',
                  'scenarioLabel': args.scenario,
                  'scenarioVerified': False, 'buildProfile': args.build_profile,
                  'appVersion': json.loads((PROJECT / 'package.json').read_text(encoding='utf-8'))['version'],
                  'platform': sys.platform, 'openSshConnections': 1, 'openSshReadRequests': 64,
                  'sizeMiB': args.size_mib, 'readDelayMs': args.read_delay_ms,
                  'timingScope': 'process start through download completion; excludes build, source generation and final SHA256',
                  'limitations': ['Not a WAN or VPN throughput measurement', 'Source and OS caches are not cleared',
                                  'OpenSSH uses one connection and 64 requests; TermRill concurrency varies',
                                  'Client process overhead, hashing during transfer and fsync policy may differ'],
                  'samples': samples, 'summary': summarize(samples)}
        output.parent.mkdir(parents=True, exist_ok=True)
        created = False
        try:
            with output.open('x', encoding='utf-8') as stream:
                created = True
                json.dump(report, stream, ensure_ascii=False, indent=2)
        except BaseException:
            if created:
                output.unlink()
            raise
        print('Report: ' + output.relative_to(PROJECT).as_posix(), flush=True)
        print('No addresses, paths or credentials in report content.', flush=True)
    except BaseException:
        preserve_failure_logs(runtime, session)
        raise
    finally:
        try:
            if fixture and fixture.poll() is None:
                fixture.terminate()
                try:
                    fixture.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    fixture.kill()
                    fixture.wait()
        finally:
            if log:
                log.close()
            cleanup(runtime)


def parse_arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--size-mib', type=int, choices=range(1, 257), default=32)
    parser.add_argument('--rounds', type=int, choices=range(1, 6), default=3)
    parser.add_argument('--concurrency', type=int, nargs='+', choices=range(1, 5), default=[1, 3])
    parser.add_argument('--build-profile', choices=['release', 'debug'], default='release')
    parser.add_argument('--read-delay-ms', type=int, choices=range(101), default=0)
    parser.add_argument('--scenario', choices=['loopback', 'direct', 'tun'], default='loopback')
    parser.add_argument('--timeout', type=int, choices=range(10, 601), default=180)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    args.concurrency = list(dict.fromkeys(args.concurrency))
    return args


if __name__ == '__main__':
    try:
        benchmark(parse_arguments())
    except (OSError, RuntimeError, ValueError) as error:
        print(f'Benchmark failed: {error}', file=sys.stderr)
        sys.exit(1)
