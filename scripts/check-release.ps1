# 本地与 CI 共用的完整检查；不安装依赖，不自动连接真实服务器。
param([switch]$Package, [string]$Python = '')
$ErrorActionPreference = 'Stop'
$termrillProject = Split-Path -Parent $PSScriptRoot
Push-Location $termrillProject
try {
    # 发行脚本调用不带 -Package 的本脚本，完整检查仅执行一次。
    if ($Package) {
        & ./scripts/release-windows.ps1 -Offline -Python $Python
        return
    }
    & npm.cmd run check:versions
    if ($LASTEXITCODE -ne 0) { throw 'Application version check failed.' }
    & npm.cmd run format:check
    if ($LASTEXITCODE -ne 0) { throw 'Format check failed.' }
    & npm.cmd test
    if ($LASTEXITCODE -ne 0) { throw 'Frontend tests failed.' }
    & npm.cmd run build
    if ($LASTEXITCODE -ne 0) { throw 'Frontend build failed.' }
    cargo check --locked --offline --manifest-path src-tauri/Cargo.toml
    if ($LASTEXITCODE -ne 0) { throw 'Rust check failed.' }
    cargo clippy --locked --offline --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
    if ($LASTEXITCODE -ne 0) { throw 'Clippy failed.' }
    & ./tests/run.ps1 -Python $Python
    Write-Host 'Local release checks passed. See docs/READINESS.md for remaining acceptance work.'
} finally {
    Pop-Location
}
