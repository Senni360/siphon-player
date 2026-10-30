#requires -Version 7.0
<#
Build at this clean checkout's HEAD, validate and package a fresh Windows x64
candidate. The manifest records that source revision and the verified hash.
This script never installs, updates, or launches a real application profile.
#>
[CmdletBinding()]
param(
    [string]$OutputDirectory = '',
    [string]$ActionsUrl = '',
    [string]$ExpectedCommit = $env:GITHUB_SHA
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if (-not $IsWindows) { throw 'This script requires Windows and PowerShell 7.' }
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $root 'dist\Siphon' }
$output = [IO.Path]::GetFullPath($ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory))
if ($output -eq [IO.Path]::GetPathRoot($output) -or
    -not ([IO.Path]::GetPathRoot($output) -match '^[A-Za-z]:\\$')) {
    throw 'Choose a fresh directory on a local Windows drive.'
}
if (Test-Path -LiteralPath $output) { throw "Output directory already exists: $output" }
$dist = Join-Path $root 'dist'
if ($root.StartsWith($output.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase) -or
    ($output.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase) -and
     -not $output.StartsWith($dist + '\', [StringComparison]::OrdinalIgnoreCase))) {
    throw 'Inside the checkout, candidate output must be a fresh directory under dist.'
}
foreach ($protected in @($env:SystemRoot, $env:ProgramFiles, ${env:ProgramFiles(x86)}, $env:ProgramData)) {
    if ($protected -and $output.StartsWith($protected.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Candidate output cannot be inside a system or installation directory.'
    }
}
for ($ancestor = Split-Path -Parent $output; $ancestor; $ancestor = Split-Path -Parent $ancestor) {
    if ((Test-Path -LiteralPath $ancestor) -and
        ((Get-Item -LiteralPath $ancestor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw 'Candidate output cannot traverse a junction or symbolic link.'
    }
}
if ($ActionsUrl) {
    $url = [uri]$ActionsUrl
    if (-not $url.IsAbsoluteUri -or $url.Scheme -ne 'https' -or $url.Host -ne 'github.com' -or
        $url.UserInfo -or $url.Query -or $url.Fragment -or
        $url.AbsolutePath -notmatch '^/[^/]+/[^/]+/actions/runs/[0-9]+/?$') {
        throw 'ActionsUrl must be an HTTPS GitHub Actions run URL.'
    }
}

Push-Location $root
try {
    $dirty = & git status --porcelain --untracked-files=all
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($dirty) { throw 'Source changes and untracked build inputs must be committed before packaging.' }
    $commit = (& git rev-parse --verify HEAD | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($commit -notmatch '^[0-9a-f]{40}$') { throw 'Could not identify the exact source commit.' }
    if ($ExpectedCommit -and $ExpectedCommit -ne $commit) {
        throw 'ExpectedCommit (or GITHUB_SHA) does not match this checkout HEAD.'
    }
    $target = Join-Path $root 'target'
    $binary = Join-Path $target 'release\siphon.exe'
    for ($ancestor = $binary; $ancestor; $ancestor = Split-Path -Parent $ancestor) {
        if ((Test-Path -LiteralPath $ancestor) -and
            ((Get-Item -LiteralPath $ancestor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'The build output cannot traverse a junction or symbolic link.'
        }
    }
    $metadata = & cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $package = @($metadata.packages | Where-Object { $_.manifest_path -eq (Join-Path $root 'Cargo.toml') })
    if ($package.Count -ne 1) { throw 'Cargo did not identify the root package.' }
    $built = @(& cargo build --locked --release --bin siphon --target-dir $target --message-format=json |
        ForEach-Object {
            $event = $_ | ConvertFrom-Json
            if ($event.reason -eq 'compiler-message' -and $event.message.rendered) {
                [Console]::Error.Write($event.message.rendered)
            }
            if ($event.reason -eq 'compiler-artifact' -and $event.package_id -eq $package[0].id -and
                $event.target.name -eq 'siphon' -and $event.target.kind -contains 'bin' -and $event.executable) {
                $event.executable
            }
        })
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($built.Count -ne 1 -or [IO.Path]::GetFullPath($built[0]) -ne $binary) {
        throw 'Cargo must produce target/release/siphon.exe; remove custom target/output overrides and rebuild.'
    }
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "The release build did not produce $binary" }
    $hash = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    $imports = & dumpbin /dependents $binary | Out-String
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($imports -match '(?i)\b(?:MSVCP|VCRUNTIME|MSVCR)[0-9][a-z0-9_]*\.dll\b') {
        throw 'The executable requires a separately installed MSVC runtime.'
    }
    $headers = & dumpbin /headers $binary | Out-String
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($headers -notmatch '8664 machine \(x64\)') { throw 'The executable is not Windows x64.' }
    if ((Get-AuthenticodeSignature -LiteralPath $binary).Status -ne 'NotSigned') {
        throw 'This unsigned-candidate script requires an unsigned executable.'
    }
    # clap exits on --version before registering the helper or opening a profile.
    $version = (& $binary --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($version -ne "siphon $($package[0].version)") {
        throw "Unexpected executable version: $version"
    }
    $dirty = & git status --porcelain --untracked-files=all
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $current = (& git rev-parse --verify HEAD | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($dirty -or $current -ne $commit) { throw 'Source changed during validation; build and package again.' }

    # No -Force: existing destinations, including installed copies, are never replaced.
    New-Item -ItemType Directory -Path $output | Out-Null
    Copy-Item -LiteralPath $binary, (Join-Path $root 'LICENSE'), (Join-Path $root 'packaging\siphon-portable.txt') -Destination $output
    if ((Get-FileHash -LiteralPath (Join-Path $output 'siphon.exe') -Algorithm SHA256).Hash.ToLowerInvariant() -ne $hash) {
        throw 'Executable changed during packaging; discard this candidate and rebuild.'
    }
    $build = [ordered]@{
        commit = $commit
        version = $version
        target = 'x86_64-pc-windows-msvc'
        sha256 = $hash
        signed = $false
    }
    if ($ActionsUrl) { $build.workflow = $ActionsUrl }
    $build | ConvertTo-Json | Set-Content -Encoding utf8 (Join-Path $output 'build.json')
    @'
Extract this entire folder into a writable location, then open siphon.exe.
No compiler is needed. This is a private, unsigned Windows x64 candidate.

build.json records the checked source commit, reported version, and executable
SHA256. SHA256SUMS.txt also covers the accompanying files. This folder is not
an installer and does not replace the installed Siphon app or helper.

The portable marker keeps this copy's state in siphon-data beside the executable.
Spotify and SoundCloud still need your account setup. YouTube uses the installed
helper. Keep this candidate separate from any existing Siphon installation.
'@ | Set-Content -Encoding utf8 (Join-Path $output 'OPEN-ME.txt')
    Get-ChildItem -LiteralPath $output -File | Where-Object Name -ne 'SHA256SUMS.txt' | Sort-Object Name | ForEach-Object {
        "{0}  {1}" -f (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(), $_.Name
    } | Set-Content -Encoding ascii (Join-Path $output 'SHA256SUMS.txt')
    Write-Host "Unsigned candidate packaged at $output (commit $commit)."
}
finally { Pop-Location }
