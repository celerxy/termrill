# 构建当前源码的正式 Windows 安装包；不安装、不发布、不修改全局环境。
param([switch]$Offline, [string]$Python = '')
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$previousLocation = Get-Location
$previousOffline = $env:CARGO_NET_OFFLINE
try {
    Set-Location -LiteralPath $projectRoot
    & ./scripts/check-release.ps1 -Python $Python
    $buildArguments = @('run', 'tauri', '--', 'build', '--bundles', 'nsis')
    if ($Offline) { $env:CARGO_NET_OFFLINE = 'true' }
    $startedAt = Get-Date
    & npm.cmd @buildArguments
    if ($LASTEXITCODE -ne 0) { throw 'Release 构建失败。' }
    # 只列出本次生成的安装包，避免把旧构建误报为本次结果。
    $installers = @(Get-ChildItem -LiteralPath 'src-tauri/target/release/bundle/nsis' -Filter '*.exe' |
        Where-Object { $_.LastWriteTime -ge $startedAt })
    if ($installers.Count -eq 0) { throw '构建结束，但没有找到本次生成的 NSIS 安装包。' }
    foreach ($installer in $installers) {
        # 使用 .NET，兼容没有加载 Get-FileHash cmdlet 的 PowerShell 环境。
        $algorithm = [System.Security.Cryptography.SHA256]::Create()
        $stream = [System.IO.File]::OpenRead($installer.FullName)
        try {
            $checksum = [System.BitConverter]::ToString($algorithm.ComputeHash($stream)).Replace('-', '').ToLowerInvariant()
        } finally {
            $stream.Dispose()
            $algorithm.Dispose()
        }
        $checksumPath = $installer.FullName + '.sha256'
        [System.IO.File]::WriteAllText($checksumPath,
            ($checksum + '  ' + $installer.Name + "`n"),
            [System.Text.UTF8Encoding]::new($false))
        Write-Host ('安装包：' + $installer.FullName)
        Write-Host ('SHA256：' + $checksum)
    }
} finally {
    $env:CARGO_NET_OFFLINE = $previousOffline
    Set-Location -LiteralPath $previousLocation.Path
}
