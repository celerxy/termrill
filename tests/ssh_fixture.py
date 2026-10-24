"""Loopback-only SSH/SFTP fixture. All data stays below the supplied test root."""
import argparse
import json
import os
from pathlib import Path
import posixpath
import select
import socket
import struct
import threading
import time

import paramiko


class Files(paramiko.SFTPServerInterface):
    # Windows 的 chmod 只能切换只读位；fixture 单独模拟 POSIX 权限，
    # 让客户端的 SETSTAT/LSTAT 核验可以在本机回归，不能当作 Linux 权限验收。
    overrides = {}
    overrides_lock = threading.Lock()
    audit_lock = threading.Lock()
    stalled_read_paths = set()

    def audit(self, kind, path=''):
        if self.faults().get('auditDelete', False):
            with self.audit_lock:
                with (self.root.parent / 'delete-events.jsonl').open('a', encoding='utf8') as stream:
                    stream.write(json.dumps({'kind': kind, 'path': path}) + '\n')

    def attributes(self, path, value):
        result = paramiko.SFTPAttributes.from_stat(value)
        if 'reportedMode' in self.faults():
            result.st_mode = (result.st_mode & 0o170000) | self.faults()['reportedMode']
        with self.overrides_lock:
            changed = self.overrides.get(str(path), {})
        if 'mode' in changed:
            result.st_mode = (result.st_mode & 0o170000) | changed['mode']
        if 'uid' in changed:
            result.st_uid = changed['uid']
        if 'gid' in changed:
            result.st_gid = changed['gid']
        return result

    def __init__(self, server, *, root):
        super().__init__(server)
        self.root = Path(root).resolve()
        self.server = server

    def faults(self):
        """故障配置仅由本机测试写入，缺省不注入任何错误。"""
        try:
            return json.loads((self.root.parent / 'faults.json').read_text(encoding='utf8'))
        except FileNotFoundError:
            return {}

    def path(self, remote):
        normalized = posixpath.normpath('/' + remote.lstrip('/'))
        result = (self.root / normalized.lstrip('/')).resolve()
        result.relative_to(self.root)
        return result

    def canonicalize(self, path):
        return posixpath.normpath('/' + path.lstrip('/'))

    def stat(self, path):
        try:
            return self.attributes(self.path(path), self.path(path).stat())
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)

    def lstat(self, path):
        try:
            return self.attributes(self.path(path), self.path(path).lstat())
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)

    def list_folder(self, path):
        if self.faults().get('denyList', False):
            return paramiko.SFTP_PERMISSION_DENIED
        try:
            result = []
            for file in self.path(path).iterdir():
                attributes = self.attributes(file, file.stat())
                attributes.filename = file.name
                result.append(attributes)
            return result
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)

    def open(self, path, flags, attr):
        try:
            fd = os.open(self.path(path), flags | getattr(os, 'O_BINARY', 0), 0o600)
            mode = 'r+b' if flags & os.O_RDWR else 'wb' if flags & os.O_WRONLY else 'rb'
            stream = os.fdopen(fd, mode)
            handle = paramiko.SFTPHandle(flags)
            handle.stat = lambda: self.attributes(self.path(path), os.fstat(stream.fileno()))
            startup_faults = self.faults()
            if '.striped-' in path and startup_faults.get('failStripedHandleStat', False):
                def audit_handle(action):
                    with self.audit_lock:
                        with (self.root.parent / 'handle-events.jsonl').open('a', encoding='utf8') as audit:
                            audit.write(json.dumps({'action': action, 'connection': id(self.server)}) + '\n')
                audit_handle('open')
                handle.stat = lambda: paramiko.SFTP_FAILURE
                original_close = handle.close
                def close_failed_handle():
                    try:
                        return original_close()
                    finally:
                        audit_handle('close')
                handle.close = close_failed_handle

            if mode != 'wb':
                handle.readfile = stream
            if mode != 'rb':
                handle.writefile = stream
            original_write = handle.write
            # 每个句柄固定故障配置。重连清除配置时，旧连接已排队的请求
            # 不能突然恢复写入，否则会跨过失败块产生测试服务自身的空洞。
            faults = self.faults()
            def paced_write(offset, data):
                if faults.get('auditWrite', False):
                    with self.audit_lock:
                        with (self.root.parent / 'write-events.jsonl').open('a', encoding='utf8') as stream:
                            stream.write(json.dumps({'path': path, 'offset': offset, 'length': len(data), 'connection': id(self.server)}) + '\n')
                if faults.get('writePathContains', '') in path:
                    if offset == faults.get('failWriteAtOffset'):
                        time.sleep(0.12)
                        return paramiko.SFTP_FAILURE
                    if 'disconnectAfterBytes' in faults and offset >= faults['disconnectAfterBytes']:
                        self.server.transport.close()
                        return paramiko.SFTP_FAILURE
                    if 'failWriteAfterBytes' in faults and offset >= faults['failWriteAfterBytes']:
                        return paramiko.SFTP_FAILURE
                time.sleep(faults.get('writeDelayMs', 4) / 1000)
                return original_write(offset, data)
            handle.write = paced_write
            original_read = handle.read
            def paced_read(offset, length):
                # 仅第一条读取此来源的连接停收，其他独立连接保持可用。
                # 故障绑定句柄，模拟 TUN 隧道仍连接但不返回 DATA 的场景。
                stall = False
                if faults.get('stallReadOnceMs', 0):
                    with self.audit_lock:
                        if path not in self.stalled_read_paths:
                            self.stalled_read_paths.add(path)
                            stall = True
                if stall:
                    time.sleep(faults['stallReadOnceMs'] / 1000)
                if faults.get('auditRead', False):
                    with self.audit_lock:
                        with (self.root.parent / 'read-events.jsonl').open('a', encoding='utf8') as stream:
                            stream.write(json.dumps({'offset': offset, 'length': length}) + '\n')
                time.sleep(faults.get('readDelayMs', 4) / 1000)
                return original_read(offset, length)
            handle.read = paced_read
            return handle
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)

    def mkdir(self, path, attr):
        try:
            self.path(path).mkdir()
            return paramiko.SFTP_OK
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)

    def rename(self, source, destination):
        try:
            faults = self.faults()
            if destination == faults.get('renameDestination') and (
                faults.get('renameSourceContains', '\0') in source or
                (faults.get('failRestore', False) and '.termrill-backup-' in source)
            ):
                return paramiko.SFTP_FAILURE
            if self.path(destination).exists():
                return paramiko.SFTP_FAILURE
            self.path(source).rename(self.path(destination))
            return paramiko.SFTP_OK
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)

    def posix_rename(self, source, destination):
        try:
            # 回滚用例模拟不支持原子替换的服务端，验证备份和恢复路径。
            if self.faults().get('renameDestination'):
                return paramiko.SFTP_OP_UNSUPPORTED
            os.replace(self.path(source), self.path(destination))
            return paramiko.SFTP_OK
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)

    def remove(self, path):
        try:
            self.audit('unlink', path)
            if self.faults().get('failBackupDelete', False) and '.termrill-backup-' in path:
                return paramiko.SFTP_FAILURE
            if path == self.faults().get('failDeletePath'):
                return paramiko.SFTP_PERMISSION_DENIED
            self.path(path).unlink()
            return paramiko.SFTP_OK
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)

    def rmdir(self, path):
        try:
            self.audit('rmdir', path)
            self.path(path).rmdir()
            return paramiko.SFTP_OK
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)

    def chattr(self, path, attr):
        try:
            if attr.st_mode is not None and self.faults().get('failPermissions', False):
                return paramiko.SFTP_FAILURE
            if os.name == 'nt':
                with self.overrides_lock:
                    changed = self.overrides.setdefault(str(self.path(path)), {})
                    if attr.st_mode is not None:
                        changed['mode'] = attr.st_mode & 0o7777
                    if attr.st_uid is not None:
                        changed['uid'] = attr.st_uid
                    if attr.st_gid is not None:
                        changed['gid'] = attr.st_gid
                if attr.st_size is not None:
                    with self.path(path).open('r+b') as stream:
                        stream.truncate(attr.st_size)
            else:
                paramiko.SFTPServer.set_file_attr(str(self.path(path)), attr)
            return paramiko.SFTP_OK
        except OSError as e:
            return paramiko.SFTPServer.convert_errno(e.errno)


class Auth(paramiko.ServerInterface):
    def __init__(self, public_key, target_port, is_jump, root):
        self.root = Path(root)
        self.key = public_key
        self.target_port = target_port
        self.is_jump = is_jump
        self.destinations = {}

    def check_auth_password(self, username, password):
        return paramiko.AUTH_SUCCESSFUL if username == 'termrill-test' and password == 'fixture-only-password' else paramiko.AUTH_FAILED

    def check_auth_publickey(self, username, key):
        return paramiko.AUTH_SUCCESSFUL if username == 'termrill-test' and key == self.key else paramiko.AUTH_FAILED

    def get_allowed_auths(self, username):
        return 'password,publickey'

    def check_channel_request(self, kind, chanid):
        return paramiko.OPEN_SUCCEEDED if kind == 'session' else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

    def check_channel_direct_tcpip_request(self, chanid, origin, destination):
        if self.is_jump and destination == ('127.0.0.1', self.target_port):
            self.destinations[chanid] = destination
            return paramiko.OPEN_SUCCEEDED
        return paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

    def terminal_faults(self):
        try:
            return json.loads((self.root.parent / 'faults.json').read_text(encoding='utf8'))
        except (OSError, ValueError):
            return {}

    def terminal_window_adjust(self, channel):
        # 在请求确认之前注入合法窗口通知，复现 OpenSSH 可出现的消息交错。
        message = paramiko.Message()
        message.add_byte(paramiko.common.cMSG_CHANNEL_WINDOW_ADJUST)
        message.add_int(channel.remote_chanid)
        message.add_int(1024)
        channel.transport._send_user_message(message)

    def check_channel_pty_request(self, channel, term, width, height, pixelwidth, pixelheight, modes):
        self.terminal_window_adjust(channel)
        return not self.terminal_faults().get('denyPty')

    def check_channel_window_change_request(self, channel, width, height, pixelwidth, pixelheight):
        return True

    def check_channel_shell_request(self, channel):
        self.terminal_window_adjust(channel)
        if self.terminal_faults().get('denyShell'):
            return False
        def echo():
            try:
                channel.sendall(b'TermRill fixture shell\r\n')
                while data := channel.recv(65536):
                    channel.sendall(data)
            except (OSError, EOFError):
                pass
        threading.Thread(target=echo, daemon=True).start()
        return True

    def check_channel_exec_request(self, channel, command):
        # 测试服务只返回固定结果，绝不在宿主机执行传入命令。
        self.terminal_window_adjust(channel)
        if self.terminal_faults().get('denyExec'):
            return False
        if command not in (b'fixture-status', b'fixture-large'):
            return False
        def respond():
            try:
                time.sleep(0.03)
                if command == b'fixture-large':
                    channel.sendall(b'x' * (80 * 1024))
                else:
                    channel.sendall(b'fixture stdout\n')
                    channel.sendall_stderr(b'fixture stderr\n')
                channel.send_exit_status(0)
                channel.close()
            except (OSError, EOFError):
                pass
        threading.Thread(target=respond, daemon=True).start()
        return True


class AuditedSFTP(paramiko.SFTPServer):
    def _send_server_version(self):
        if 'maxWriteLen' not in self.server.faults():
            return super()._send_server_version()
        from paramiko.sftp import CMD_INIT, CMD_VERSION
        kind, data = self._read_packet()
        if kind != CMD_INIT:
            raise paramiko.SFTPError('Expected INIT')
        message = paramiko.Message()
        message.add_int(3)
        message.add_string('limits@openssh.com')
        message.add_string('1')
        self._send_packet(CMD_VERSION, message)
        return struct.unpack('>I', data[:4])[0]

    """记录真实 READDIR 请求顺序，验证分批扫描是否与删除重叠。"""
    def _process(self, kind, request_number, message):
        from paramiko.sftp import CMD_EXTENDED, CMD_EXTENDED_REPLY
        # 目录故障注入只检查目录相关请求，不给每个 READ/WRITE 增加同步磁盘读取。
        faults = self.server.faults() if kind in (paramiko.sftp.CMD_REALPATH, paramiko.sftp.CMD_OPENDIR, paramiko.sftp.CMD_CLOSE) else {}
        if faults.get('auditListing', False) and kind in (paramiko.sftp.CMD_REALPATH, paramiko.sftp.CMD_OPENDIR, paramiko.sftp.CMD_CLOSE):
            with self.server.audit_lock:
                with (self.server.root.parent / 'listing-events.jsonl').open('a', encoding='utf8') as stream:
                    stream.write(json.dumps({'kind': kind}) + '\n')
        if kind == paramiko.sftp.CMD_REALPATH and faults.get('denyRealpath', False):
            self._send_status(request_number, paramiko.SFTP_PERMISSION_DENIED)
            return
        if kind == CMD_EXTENDED and paramiko.Message(message.get_remainder()).get_text() == 'limits@openssh.com':
            reply = paramiko.Message()
            reply.add_int(request_number)
            for value in [262144, 131072, self.server.faults().get('maxWriteLen', 32768), 32]:
                reply.add_int64(value)
            self._send_packet(CMD_EXTENDED_REPLY, reply)
            return
        if kind == paramiko.sftp.CMD_READDIR:
            self.server.audit('readdir')
        return super()._process(kind, request_number, message)


def forward(channel, destination):
    try:
        with socket.create_connection(destination) as stream:
            while True:
                readable, _, _ = select.select([channel, stream], [], [], 1)
                for source in readable:
                    data = source.recv(65536)
                    if not data:
                        return
                    (stream if source is channel else channel).sendall(data)
    finally:
        channel.close()


def serve(listener, host_key, public_key, root, target_port, is_jump):
    def handle(stream):
        transport = paramiko.Transport(stream)
        transport.get_security_options().kex = ('diffie-hellman-group14-sha256',)
        server = Auth(public_key, target_port, is_jump, root)
        server.transport = transport
        transport.add_server_key(host_key)
        transport.set_subsystem_handler('sftp', AuditedSFTP, Files, root=root)
        try:
            transport.start_server(server=server)
            while transport.is_active():
                channel = transport.accept(1)
                if channel and channel.chanid in server.destinations:
                    threading.Thread(target=forward, args=(channel, server.destinations[channel.chanid]), daemon=True).start()
        except (paramiko.SSHException, EOFError, OSError):
            pass
        finally:
            transport.close()
    while True:
        stream, _ = listener.accept()
        threading.Thread(target=handle, args=(stream,), daemon=True).start()


def listener():
    result = socket.socket()
    result.bind(('127.0.0.1', 0))
    result.listen(16)
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', required=True)
    args = parser.parse_args()
    root = Path(args.root).resolve()
    root.mkdir(parents=True, exist_ok=True)
    remote = root / 'remote'
    remote.mkdir(exist_ok=True)
    (remote / 'welcome.txt').write_text('hello from fixture', encoding='utf8')
    client_key = paramiko.RSAKey.generate(2048)
    key_path = root / 'client-key'
    client_key.write_private_key_file(str(key_path))
    target, jump = listener(), listener()
    target_port = target.getsockname()[1]
    target_key = paramiko.RSAKey.generate(2048)
    for sock, host_key, is_jump in [(target, target_key, False), (jump, paramiko.RSAKey.generate(2048), True)]:
        threading.Thread(target=serve, args=(sock,host_key,client_key,remote,target_port,is_jump), daemon=True).start()
    # 本机启动器直接提供预期公钥，基线工具据此严格校验；不扫描并静默信任网络公钥。
    info = {'targetPort': target_port, 'jumpPort': jump.getsockname()[1], 'keyPath': str(key_path), 'root': str(root),
            'targetHostKey': target_key.get_name() + ' ' + target_key.get_base64()}
    ready = root / 'ready.tmp'
    ready.write_text(json.dumps(info), encoding='utf8')
    ready.replace(root / 'ready.json')
    print('Loopback SSH fixture ready', flush=True)
    while True:
        time.sleep(1)
