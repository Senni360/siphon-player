#requires -Version 7.0
<#
Compile with the same features as the preceding all-features suite. Only the
actual ignored dummy-grant test receives the timeout, never Cargo compilation.
Run from an MSVC-enabled PowerShell 7 terminal; no real profile is opened.
#>
[CmdletBinding()]
param([ValidateRange(1, 3600)][int]$TimeoutSeconds = 120)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
if (-not $IsWindows) { throw 'This script requires Windows and PowerShell 7.' }
$root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$manifest = Join-Path $root 'Cargo.toml'
$testName = 'credentials::tests::native_store_round_trip'
Push-Location $root
try {
    $metadata = & cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    $package = @($metadata.packages | Where-Object { $_.manifest_path -eq $manifest })
    if ($package.Count -ne 1) { throw 'Cargo did not identify the root package.' }
    $library = @($package[0].targets | Where-Object { $_.kind -contains 'lib' })
    if ($library.Count -ne 1) { throw 'Cargo did not identify exactly one library target.' }
    $artifacts = @(& cargo test --locked --all-features --lib --no-run --message-format=json |
        ForEach-Object {
            $event = $_ | ConvertFrom-Json
            if ($event.reason -eq 'compiler-message' -and $event.message.rendered) {
                [Console]::Error.Write($event.message.rendered)
            }
            if ($event.reason -eq 'compiler-artifact' -and $event.profile.test -and
                $event.package_id -eq $package[0].id -and $event.target.name -eq $library[0].name -and
                $event.target.kind -contains 'lib' -and $event.executable) {
                $event.executable
            }
        })
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    if ($artifacts.Count -ne 1 -or -not (Test-Path -LiteralPath $artifacts[0] -PathType Leaf)) {
        throw 'Cargo did not identify exactly one root library test executable.'
    }

    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $artifacts[0]
    $start.WorkingDirectory = $root
    $start.UseShellExecute = $false
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in @($testName, '--ignored', '--exact', '--test-threads=1')) {
        $start.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    try {
        if (-not $process.Start()) { throw 'Could not start the native credential test.' }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        $timedOut = -not $process.WaitForExit($TimeoutSeconds * 1000)
        if ($timedOut) {
            # Kill only this process and its descendants, never by executable name.
            try { if (-not $process.HasExited) { $process.Kill($true) } }
            catch [InvalidOperationException] { if (-not $process.HasExited) { throw } }
            if (-not $process.WaitForExit(10000)) { throw 'The timed-out test process did not stop.' }
        }
        $output = $stdout.GetAwaiter().GetResult()
        [Console]::Out.Write($output)
        [Console]::Error.Write($stderr.GetAwaiter().GetResult())
        if ($timedOut) { throw "The native credential test exceeded $TimeoutSeconds seconds." }
        if ($process.ExitCode -ne 0) { exit $process.ExitCode }
        if ($output -notmatch '(?m)^test result: ok\. 1 passed; 0 failed; 0 ignored;') {
            throw 'The harness did not report exactly one passing native credential test.'
        }
    }
    finally { $process.Dispose() }
}
finally { Pop-Location }
