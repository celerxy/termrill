"""从当前成功的四平台任务收集校验后的安装包，完整上传草稿后才公开预发布。"""

import hashlib
import io
import json
import os
from pathlib import Path
import re
import urllib.error
import urllib.parse
import urllib.request
import zipfile


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def installer_names(version):
    prefix = f'TermRill_{version}_'
    names = [prefix + suffix for suffix in
             ['aarch64.dmg', 'x64.dmg', 'x64-setup.exe', 'amd64.deb', 'amd64.AppImage']]
    return set(names + [name + '.sha256' for name in names])


def verified_archive(data, digest, expected):
    if 'sha256:' + hashlib.sha256(data).hexdigest() != digest:
        raise ValueError('Artifact archive SHA256 mismatch')
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        entries = archive.infolist()
        names = [entry.filename for entry in entries]
        if len(names) != len(set(names)) or not set(names) <= expected:
            raise ValueError('Duplicate or unexpected artifact entry')
        if sum(entry.file_size for entry in entries) > 200 * 1024 * 1024:
            raise ValueError('Installer exceeds artifact budget')
        assets = {entry.filename: archive.read(entry) for entry in entries}
    for name, content in assets.items():
        if name.endswith('.sha256'):
            continue
        checksum = assets.get(name + '.sha256', b'').decode('utf-8-sig').strip()
        if checksum != hashlib.sha256(content).hexdigest() + '  ' + name:
            raise ValueError('Missing or incorrect installer checksum')
    return assets


def main():
    repository = os.environ['GITHUB_REPOSITORY']
    if repository != 'celerxy/termrill':
        raise ValueError('Release destination must be celerxy/termrill')
    sha = os.environ['GITHUB_SHA']
    run_id = os.environ['GITHUB_RUN_ID']
    token = os.environ['GH_TOKEN']
    version = json.loads(Path('package.json').read_text(encoding='utf-8'))['version']
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('Expected a numeric application version')
    tag = 'v' + version
    notes = Path(f'docs/releases/{tag}.md').read_text(encoding='utf-8')
    base = f'https://api.github.com/repos/{repository}'
    client = urllib.request.build_opener(NoRedirect())

    def api(path, method='GET', payload=None, binary=None, optional=False):
        url = path if path.startswith('https://') else base + path
        parsed = urllib.parse.urlparse(url)
        if parsed.scheme != 'https' or parsed.hostname not in ['api.github.com', 'uploads.github.com']:
            raise ValueError('Unexpected authenticated API host')
        headers = {'Authorization': 'Bearer ' + token, 'Accept': 'application/vnd.github+json',
                   'Content-Type': 'application/json', 'User-Agent': 'TermRill-release'}
        data = json.dumps(payload).encode() if payload is not None else None
        if binary is not None:
            data = binary
            headers['Content-Type'] = 'application/octet-stream'
        try:
            with client.open(urllib.request.Request(url, data=data, headers=headers,
                                                  method=method), timeout=120) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            if optional and error.code == 404:
                return None
            raise

    assets = collect_assets(api, run_id, sha, version)
    publish_assets(api, sha, version, notes, assets)


def collect_assets(api, run_id, sha, version):
    run = api(f'/actions/runs/{run_id}')
    if run['head_sha'] != sha:
        raise ValueError('Workflow commit mismatch')
    jobs = api(f'/actions/runs/{run_id}/jobs?per_page=100')['jobs']
    platforms = {'windows-x64', 'macos-arm64', 'macos-x64', 'linux-x64'}
    for platform in platforms:
        if not any(job['name'] == 'Package ' + platform and job['conclusion'] == 'success'
                   for job in jobs):
            raise ValueError('Platform packaging did not pass: ' + platform)
    artifacts = api(f'/actions/runs/{run_id}/artifacts?per_page=100')['artifacts']
    expected_artifacts = {f'TermRill-{platform}-{sha}' for platform in platforms}
    if {artifact['name'] for artifact in artifacts} != expected_artifacts:
        raise ValueError('Expected exactly four platform artifacts')
    expected = installer_names(version)
    assets = {}
    for artifact in artifacts:
        if artifact['expired'] or artifact['workflow_run']['head_sha'] != sha:
            raise ValueError('Expired or mismatched artifact')
        if artifact['size_in_bytes'] > 200 * 1024 * 1024:
            raise ValueError('Artifact exceeds archive budget')
        try:
            api(f'/actions/artifacts/{artifact["id"]}/zip')
            raise ValueError('Expected an artifact download redirect')
        except urllib.error.HTTPError as redirect:
            if redirect.code != 302:
                raise
            location = redirect.headers['Location']
        if urllib.parse.urlparse(location).scheme != 'https':
            raise ValueError('Expected HTTPS artifact download')
        # 签名下载链接使用无凭据请求，GitHub 令牌绝不转发给存储域名。
        with urllib.request.urlopen(location, timeout=120) as response:
            data = response.read(200 * 1024 * 1024 + 1)
        if len(data) != artifact['size_in_bytes']:
            raise ValueError('Artifact size mismatch')
        files = verified_archive(data, artifact['digest'], expected)
        if set(files) & set(assets):
            raise ValueError('Conflicting platform assets')
        assets.update(files)
    if set(assets) != expected:
        raise ValueError('Incomplete release asset set')
    return assets


def publish_assets(api, sha, version, notes, assets):
    tag = 'v' + version
    expected = installer_names(version)
    existing_tag = api('/git/ref/tags/' + tag, optional=True)
    if existing_tag and existing_tag['object']['sha'] != sha:
        raise ValueError('Existing tag points to a different commit')
    releases = api('/releases?per_page=100')
    matches = [release for release in releases if release['tag_name'] == tag]
    release = matches[0] if matches else None
    if release:
        if release['target_commitish'] != sha or release['body'] != notes:
            raise ValueError('Existing release differs; never overwrite it')
    else:
        release = api('/releases', 'POST', {'tag_name': tag, 'target_commitish': sha,
                      'name': 'TermRill ' + tag + ' Preview', 'body': notes,
                      'draft': True, 'prerelease': True})
    uploaded = {asset['name']: asset for asset in release['assets']}
    if set(uploaded) - expected:
        raise ValueError('Unexpected existing release assets')
    for name, content in sorted(assets.items()):
        asset = uploaded.get(name)
        if asset is None:
            if not release['draft']:
                raise ValueError('Never modify a published release')
            url = release['upload_url'].split('{')[0] + '?name=' + urllib.parse.quote(name)
            asset = api(url, 'POST', binary=content)
        if (asset['size'] != len(content) or asset['state'] != 'uploaded' or
                asset.get('digest') != 'sha256:' + hashlib.sha256(content).hexdigest()):
            raise ValueError('Uploaded asset verification failed: ' + name)
        print('Verified release asset:', name, flush=True)
    current = api('/releases/' + str(release['id']))
    if len(current['assets']) != len(expected):
        raise ValueError('Uploaded asset count mismatch')
    if release['draft']:
        release = api('/releases/' + str(release['id']), 'PATCH',
                      {'draft': False, 'prerelease': True})
    if release['draft'] or not release['prerelease']:
        raise ValueError('Release was not published as a preview')
    print('Published:', release['html_url'], flush=True)
    with open(os.environ['GITHUB_STEP_SUMMARY'], 'a', encoding='utf-8') as summary:
        summary.write(f'Published [{tag}]({release["html_url"]}) with 10 verified assets.\n')


if __name__ == '__main__':
    main()
