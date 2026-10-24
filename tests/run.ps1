param([string]$Filter = '', [string]$Python = '', [string]$RuntimeRoot = '')
$ErrorActionPreference = 'Stop'
$termrillProject = Split-Path -Parent $PSScriptRoot
$termrillPython = if ($Python) { (Resolve-Path -LiteralPath $Python).Path } else { Join-Path $PSScriptRoot '.venv/Scripts/python.exe' }
if (-not (Test-Path -LiteralPath $termrillPython)) {
    throw '请先运行 python -m venv tests/.venv，并安装 tests/requirements.txt。'
}
& $termrillPython -m unittest discover -s (Join-Path $termrillProject 'tests') -p test_transfer_benchmark.py
if ($LASTEXITCODE -ne 0) { throw '下载对照工具的校验与清理测试失败。' }
$termrillRuntimeBase = if ($RuntimeRoot) { (Resolve-Path -LiteralPath $RuntimeRoot).Path } else { $PSScriptRoot }
$termrillRuntime = Join-Path $termrillRuntimeBase ('.runtime-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $termrillRuntime | Out-Null
$termrillScript = Join-Path $PSScriptRoot 'ssh_fixture.py'
$termrillFixture = Start-Process -FilePath $termrillPython -ArgumentList @('"' + $termrillScript + '"', '--root', '"' + $termrillRuntime + '"') -WindowStyle Hidden -PassThru -RedirectStandardOutput (Join-Path $termrillRuntime 'server.log') -RedirectStandardError (Join-Path $termrillRuntime 'server-error.log')
$termrillPreviousFixture = $env:TERMRILL_TEST_FIXTURE
$termrillPreviousManifest = $env:TERMRILL_TEST_MANIFEST
$termrillTestsPassed = $false
try {
    $termrillReady = Join-Path $termrillRuntime 'ready.json'
    # 冷启动生成三组 RSA 密钥，构建并行时给本机测试服务最多 30 秒准备时间。
    for ($termrillAttempt = 0; $termrillAttempt -lt 300; $termrillAttempt++) {
        if (Test-Path -LiteralPath $termrillReady) { break }
        if ($termrillFixture.HasExited) { throw '本机 SSH 测试服务启动失败，请检查 server-error.log。' }
        Start-Sleep -Milliseconds 100
    }
    if (-not (Test-Path -LiteralPath $termrillReady)) { throw '本机 SSH 测试服务启动超时。' }
    $env:TERMRILL_TEST_FIXTURE = $termrillReady
    $env:TERMRILL_TEST_MANIFEST = '1'
    # 故障配置共用一个本机 fixture，串行执行避免不同用例互相干扰。
    $termrillFilterArgs = @()
    if ($Filter) { $termrillFilterArgs += $Filter }
    cargo test --locked --offline --manifest-path (Join-Path $termrillProject 'src-tauri/Cargo.toml') --lib @termrillFilterArgs -- --include-ignored --test-threads=1 --show-output
    if ($LASTEXITCODE -ne 0) { throw 'TermRill 测试失败。' }
    $termrillTestsPassed = $true
} finally {
    $env:TERMRILL_TEST_FIXTURE = $termrillPreviousFixture
    $env:TERMRILL_TEST_MANIFEST = $termrillPreviousManifest
    if (-not $termrillFixture.HasExited) { Stop-Process -Id $termrillFixture.Id }
    $termrillFixture.WaitForExit()
    # 成功自动清理；失败只保留最新服务日志，不保留大体积测试文件副本。
    $termrillResolvedRuntime = [System.IO.Path]::GetFullPath($termrillRuntime)
    if ((Split-Path -Parent $termrillResolvedRuntime) -ne $termrillRuntimeBase -or (Split-Path -Leaf $termrillResolvedRuntime) -notmatch '^\.runtime-[0-9a-f]{32}$') {
        throw '测试临时目录超出预期范围，拒绝清理。'
    }
    try {
        if (-not $termrillTestsPassed) {
            $termrillFailureLogs = Join-Path $PSScriptRoot '.runtime-last-failure'
            New-Item -ItemType Directory -Path $termrillFailureLogs -Force | Out-Null
            foreach ($termrillLogName in @('server.log', 'server-error.log')) {
                Copy-Item -LiteralPath (Join-Path $termrillResolvedRuntime $termrillLogName) -Destination (Join-Path $termrillFailureLogs $termrillLogName) -Force
            }
        }
    } catch {
        Write-Warning '无法保存服务日志；仍清理本次测试数据，不覆盖原始测试失败。'
    } finally {
        Remove-Item -LiteralPath $termrillResolvedRuntime -Recurse -Force
    }
}
