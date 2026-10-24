# 本机性能对照入口；Python fixture 与下载副本始终位于独立的仓库临时目录。
param(
    [string]$Python = '',
    [ValidateSet('release', 'debug')][string]$BuildProfile = 'release',
    [ValidateRange(1, 256)][int]$SizeMiB = 32,
    [ValidateRange(1, 5)][int]$Rounds = 3,
    [ValidateSet(1, 2, 3, 4)][int[]]$Concurrency = @(1, 3),
    [ValidateSet('loopback', 'direct', 'tun')][string]$Scenario = 'loopback',
    [ValidateRange(0, 100)][int]$ReadDelayMs = 0
)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$pythonPath = if ($Python) { (Resolve-Path -LiteralPath $Python).Path } else { Join-Path $projectRoot 'tests/.venv/Scripts/python.exe' }
if (-not (Test-Path -LiteralPath $pythonPath)) { throw '请先按 README 准备 Python fixture 环境，或使用 -Python 指定。' }
$arguments = @((Join-Path $PSScriptRoot 'benchmark-transfer.py'), '--build-profile', $BuildProfile,
    '--size-mib', $SizeMiB, '--rounds', $Rounds, '--scenario', $Scenario, '--read-delay-ms', $ReadDelayMs,
    '--concurrency') + $Concurrency
& $pythonPath @arguments
if ($LASTEXITCODE -ne 0) { throw '下载性能对照未通过；不能将部分样本当作成功报告。' }
