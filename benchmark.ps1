<#
.SYNOPSIS
Benchmarks both Windows search backends and exports process metrics to CSV.

.DESCRIPTION
Builds separate release binaries for the default ignore backend and the
Everything IPC backend. CLI runs measure a complete search. GUI runs measure
both time to the first rendered frame and process resource usage over a fixed
sampling period.

The Everything desktop client must already be running with IPC enabled.

.EXAMPLE
pwsh -NoProfile -File .\benchmark.ps1

.EXAMPLE
pwsh -NoProfile -File .\benchmark.ps1 -Mode scan -ScanRuns 10 -WarmupRuns 2

.EXAMPLE
pwsh -NoProfile -File .\benchmark.ps1 -Mode gui -GuiDurationSeconds 10 -NoBuild
#>

[CmdletBinding()]
param(
    [ValidateSet("all", "scan", "gui")]
    [string] $Mode = "all",

    [ValidateRange(1, 1000)]
    [int] $ScanRuns = 5,

    [ValidateRange(1, 1000)]
    [int] $GuiRuns = 1,

    [ValidateRange(0, 1000)]
    [int] $WarmupRuns = 1,

    [ValidateRange(0, 1000)]
    [int] $GuiWarmupRuns = 0,

    [ValidateRange(0.1, 3600)]
    [double] $GuiDurationSeconds = 5,

    [ValidateRange(1, 10000)]
    [int] $SampleIntervalMilliseconds = 10,

    [ValidateNotNullOrEmpty()]
    [string] $Output = "benchmark-windows.csv",

    [switch] $NoBuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$workspace = $PSScriptRoot
$artifactDir = Join-Path $workspace "target\release\benchmark-windows"
$ignoreBinary = Join-Path $artifactDir "cefdetector-ignore.exe"
$everythingBinary = Join-Path $artifactDir "cefdetector-everything.exe"
$logicalProcessors = [Environment]::ProcessorCount
$records = [Collections.Generic.List[object]]::new()
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) (
    "cefdetector-benchmark-" + [Guid]::NewGuid().ToString("N")
)

function Invoke-CargoBuild {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [string[]] $Arguments,

        [Parameter(Mandatory)]
        [string] $Destination
    )

    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo exited with status $LASTEXITCODE"
    }
    Copy-Item -LiteralPath (Join-Path $workspace "target\release\cefdetector.exe") `
        -Destination $Destination -Force
}

function Build-BenchmarkBinaries {
    New-Item -ItemType Directory -Path $artifactDir -Force | Out-Null

    Write-Host "Building locked release binary for the ignore backend..."
    Invoke-CargoBuild `
        -Arguments @(
            "build",
            "--locked",
            "--release",
            "--no-default-features",
            "--features",
            "gui"
        ) `
        -Destination $ignoreBinary

    Write-Host "Building locked release binary for the Everything backend..."
    Invoke-CargoBuild `
        -Arguments @(
            "build",
            "--locked",
            "--release",
            "--no-default-features",
            "--features",
            "gui,index"
        ) `
        -Destination $everythingBinary
}

function Assert-IndexBackend {
    param(
        [Parameter(Mandatory)]
        [string] $Binary,

        [Parameter(Mandatory)]
        [string] $ExpectedBackend
    )

    $resultFile = Join-Path $temporaryRoot "backend-validation.json"
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Binary
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.WorkingDirectory = $workspace
    foreach ($argument in @("cli", "--json", "--output", $resultFile)) {
        $startInfo.ArgumentList.Add($argument)
    }

    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw "failed to start index backend validation: $Binary"
    }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $process.WaitForExit()
    $stdout = $stdoutTask.GetAwaiter().GetResult().Trim()
    $stderr = $stderrTask.GetAwaiter().GetResult().Trim()
    if ($process.ExitCode -ne 0) {
        throw (
            "index backend validation failed with status $($process.ExitCode): " +
            "$stderr $stdout"
        ).Trim()
    }

    $marker = "cefdetector-search-backend=$ExpectedBackend"
    if (-not (($stderr -split "`r?`n") -contains $marker)) {
        throw (
            "expected index backend '$ExpectedBackend', but the scanner reported: " +
            $(if ($stderr) { $stderr } else { "<no backend>" })
        )
    }
    Write-Host "Verified index backend: $ExpectedBackend"
}

function New-ProcessStartInfo {
    param(
        [Parameter(Mandatory)]
        [string] $Binary,

        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [string[]] $Arguments,

        [Parameter(Mandatory)]
        [bool] $ExitAfterFirstFrame
    )

    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Binary
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardError = $true
    $startInfo.WorkingDirectory = $workspace
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }
    if ($ExitAfterFirstFrame) {
        $startInfo.Environment["CEFDETECTOR_GUI_SMOKE_TEST"] = "1"
    } else {
        $null = $startInfo.Environment.Remove("CEFDETECTOR_GUI_SMOKE_TEST")
    }
    $startInfo
}

function Invoke-MeasuredProcess {
    param(
        [Parameter(Mandatory)]
        [ValidateSet("scan", "gui-startup", "gui")]
        [string] $MeasureMode,

        [Parameter(Mandatory)]
        [ValidateSet("ignore", "everything")]
        [string] $Backend,

        [Parameter(Mandatory)]
        [string] $Binary,

        [Parameter(Mandatory)]
        [int] $Run,

        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [string] $ResultFile
    )

    [string[]] $arguments = @()
    if ($MeasureMode -eq "scan") {
        $arguments = @("cli", "--json", "--output", $ResultFile)
    }
    $startInfo = New-ProcessStartInfo -Binary $Binary -Arguments $arguments `
        -ExitAfterFirstFrame ($MeasureMode -eq "gui-startup")
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $stopwatch = [Diagnostics.Stopwatch]::StartNew()
    if (-not $process.Start()) {
        throw "failed to start $Binary"
    }
    $stderrTask = $process.StandardError.ReadToEndAsync()

    [long] $peakWorkingSetBytes = 0
    [long] $peakPrivateBytes = 0
    [int] $peakHandles = 0
    [int] $peakThreads = 0
    [double] $cpuTotalMilliseconds = 0
    [double] $cpuUserMilliseconds = 0
    [double] $cpuKernelMilliseconds = 0
    [int] $sampleCount = 0
    [Nullable[double]] $measurementElapsedMilliseconds = $null
    $stopMethod = "process-exit"

    while (-not $process.HasExited) {
        $process.Refresh()
        try {
            $peakWorkingSetBytes = [Math]::Max(
                $peakWorkingSetBytes,
                [Math]::Max($process.WorkingSet64, $process.PeakWorkingSet64)
            )
            $peakPrivateBytes = [Math]::Max(
                $peakPrivateBytes,
                $process.PrivateMemorySize64
            )
            $peakHandles = [Math]::Max($peakHandles, $process.HandleCount)
            $peakThreads = [Math]::Max($peakThreads, $process.Threads.Count)
            $cpuTotalMilliseconds = [Math]::Max(
                $cpuTotalMilliseconds,
                $process.TotalProcessorTime.TotalMilliseconds
            )
            $cpuUserMilliseconds = [Math]::Max(
                $cpuUserMilliseconds,
                $process.UserProcessorTime.TotalMilliseconds
            )
            $cpuKernelMilliseconds = [Math]::Max(
                $cpuKernelMilliseconds,
                $process.PrivilegedProcessorTime.TotalMilliseconds
            )
            $sampleCount++
        } catch [InvalidOperationException] {
            break
        }

        if (
            $MeasureMode -eq "gui" -and
            $stopwatch.Elapsed.TotalSeconds -ge $GuiDurationSeconds
        ) {
            $measurementElapsedMilliseconds = $stopwatch.Elapsed.TotalMilliseconds
            $stopMethod = "close-window"
            $closed = $process.CloseMainWindow()
            if (-not $closed -or -not $process.WaitForExit(250)) {
                $stopMethod = "kill"
                $process.Kill($true)
            }
            break
        }
        Start-Sleep -Milliseconds $SampleIntervalMilliseconds
    }

    $process.WaitForExit()
    $stopwatch.Stop()

    if ($null -eq $measurementElapsedMilliseconds) {
        # A short-lived process can exit between two samples. CPU time and the
        # process-maintained peak working set remain readable after WaitForExit
        # on Windows, so take one final snapshot where possible. Skip it after
        # a fixed GUI interval to exclude shutdown work from measured metrics.
        try {
            $cpuTotalMilliseconds = [Math]::Max(
                $cpuTotalMilliseconds,
                $process.TotalProcessorTime.TotalMilliseconds
            )
            $cpuUserMilliseconds = [Math]::Max(
                $cpuUserMilliseconds,
                $process.UserProcessorTime.TotalMilliseconds
            )
            $cpuKernelMilliseconds = [Math]::Max(
                $cpuKernelMilliseconds,
                $process.PrivilegedProcessorTime.TotalMilliseconds
            )
        } catch [InvalidOperationException] {
            # The process exited before Windows retained the final counters.
        }
        try {
            $peakWorkingSetBytes = [Math]::Max(
                $peakWorkingSetBytes,
                $process.PeakWorkingSet64
            )
            $peakPrivateBytes = [Math]::Max(
                $peakPrivateBytes,
                $process.PrivateMemorySize64
            )
        } catch [InvalidOperationException] {
            # The sampled peak remains valid if the final snapshot is unavailable.
        }
    }

    $stderr = $stderrTask.GetAwaiter().GetResult().Trim()
    $exitStatus = $process.ExitCode
    $expectedExit = (
        $exitStatus -eq 0 -or
        ($MeasureMode -eq "gui" -and $stopMethod -eq "kill")
    )
    if (-not $expectedExit) {
        throw "$Backend $MeasureMode failed with status ${exitStatus}: $stderr"
    }

    $resultCount = $null
    if ($MeasureMode -eq "scan") {
        if (-not (Test-Path -LiteralPath $ResultFile -PathType Leaf)) {
            throw "$Backend scan did not create $ResultFile"
        }
        $parsed = Get-Content -Raw -LiteralPath $ResultFile | ConvertFrom-Json
        $resultCount = if ($null -eq $parsed) { 0 } else { @($parsed).Count }
    }

    $processLifetimeMilliseconds = [Math]::Round(
        $stopwatch.Elapsed.TotalMilliseconds,
        3
    )
    $elapsedMilliseconds = if ($null -ne $measurementElapsedMilliseconds) {
        [Math]::Round($measurementElapsedMilliseconds, 3)
    } else {
        $processLifetimeMilliseconds
    }
    $cpuPercentOneCore = if ($elapsedMilliseconds -gt 0) {
        [Math]::Round($cpuTotalMilliseconds * 100 / $elapsedMilliseconds, 3)
    } else {
        0
    }
    $cpuPercentMachine = if ($logicalProcessors -gt 0) {
        [Math]::Round($cpuPercentOneCore / $logicalProcessors, 3)
    } else {
        0
    }

    [pscustomobject][ordered]@{
        timestamp_utc            = [DateTime]::UtcNow.ToString("o")
        mode                     = $MeasureMode
        backend                  = $Backend
        run                      = $Run
        elapsed_ms               = $elapsedMilliseconds
        process_lifetime_ms       = $processLifetimeMilliseconds
        cpu_total_ms              = [Math]::Round($cpuTotalMilliseconds, 3)
        cpu_user_ms               = [Math]::Round($cpuUserMilliseconds, 3)
        cpu_kernel_ms             = [Math]::Round($cpuKernelMilliseconds, 3)
        cpu_percent_one_core      = $cpuPercentOneCore
        cpu_percent_machine       = $cpuPercentMachine
        peak_working_set_bytes    = $peakWorkingSetBytes
        peak_private_bytes        = $peakPrivateBytes
        peak_handles              = $peakHandles
        peak_threads              = $peakThreads
        samples                   = $sampleCount
        exit_status               = $exitStatus
        stop_method               = $stopMethod
        result_count              = $resultCount
        binary_bytes              = (Get-Item -LiteralPath $Binary).Length
        logical_processors        = $logicalProcessors
        sample_interval_ms        = $SampleIntervalMilliseconds
        requested_gui_duration_ms = if ($MeasureMode -eq "gui") {
            [Math]::Round($GuiDurationSeconds * 1000, 3)
        } else {
            $null
        }
    }
}

function Invoke-Warmups {
    param(
        [Parameter(Mandatory)]
        [ValidateSet("scan", "gui")]
        [string] $WarmupMode,

        [Parameter(Mandatory)]
        [ValidateSet("ignore", "everything")]
        [string] $Backend,

        [Parameter(Mandatory)]
        [string] $Binary,

        [Parameter(Mandatory)]
        [int] $Count
    )

    for ($warmup = 1; $warmup -le $Count; $warmup++) {
        Write-Host "Warmup $warmup/$Count`: $Backend $WarmupMode"
        $resultFile = Join-Path $temporaryRoot (
            "warmup-$Backend-$WarmupMode-$warmup.json"
        )
        $measureMode = if ($WarmupMode -eq "scan") { "scan" } else { "gui" }
        $null = Invoke-MeasuredProcess -MeasureMode $measureMode `
            -Backend $Backend -Binary $Binary -Run 0 -ResultFile $resultFile
    }
}

function Write-RunSummary {
    param(
        [Parameter(Mandatory)]
        [object] $Record
    )

    $workingSetMiB = [Math]::Round($Record.peak_working_set_bytes / 1MB, 2)
    $privateMiB = [Math]::Round($Record.peak_private_bytes / 1MB, 2)
    $resultText = if ($null -eq $Record.result_count) {
        ""
    } else {
        ", $($Record.result_count) results"
    }
    Write-Host (
        "$($Record.backend) $($Record.mode) run $($Record.run): " +
        "$($Record.elapsed_ms) ms, $workingSetMiB MiB peak working set, " +
        "$privateMiB MiB peak private$resultText"
    )
}

try {
    Set-Location -LiteralPath $workspace
    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null

    if (-not $NoBuild) {
        Build-BenchmarkBinaries
    }
    foreach ($binary in @($ignoreBinary, $everythingBinary)) {
        if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
            throw "benchmark binary not found: $binary (run without -NoBuild)"
        }
    }
    Assert-IndexBackend -Binary $everythingBinary -ExpectedBackend "everything"

    $backends = [ordered]@{
        ignore     = $ignoreBinary
        everything = $everythingBinary
    }

    foreach ($entry in $backends.GetEnumerator()) {
        $backend = $entry.Key
        $binary = $entry.Value

        if ($Mode -in @("all", "scan")) {
            Invoke-Warmups -WarmupMode scan -Backend $backend -Binary $binary `
                -Count $WarmupRuns
            for ($run = 1; $run -le $ScanRuns; $run++) {
                $resultFile = Join-Path $temporaryRoot (
                    "result-$backend-scan-$run.json"
                )
                $record = Invoke-MeasuredProcess -MeasureMode scan `
                    -Backend $backend -Binary $binary -Run $run `
                    -ResultFile $resultFile
                $records.Add($record)
                Write-RunSummary $record
            }
        }

        if ($Mode -in @("all", "gui")) {
            Invoke-Warmups -WarmupMode gui -Backend $backend -Binary $binary `
                -Count $GuiWarmupRuns
            for ($run = 1; $run -le $GuiRuns; $run++) {
                $startupRecord = Invoke-MeasuredProcess -MeasureMode gui-startup `
                    -Backend $backend -Binary $binary -Run $run `
                    -ResultFile ""
                $records.Add($startupRecord)
                Write-RunSummary $startupRecord

                $guiRecord = Invoke-MeasuredProcess -MeasureMode gui `
                    -Backend $backend -Binary $binary -Run $run `
                    -ResultFile ""
                $records.Add($guiRecord)
                Write-RunSummary $guiRecord
            }
        }
    }

    $outputPath = if ([IO.Path]::IsPathRooted($Output)) {
        [IO.Path]::GetFullPath($Output)
    } else {
        [IO.Path]::GetFullPath((Join-Path $workspace $Output))
    }
    $outputDirectory = Split-Path -Parent $outputPath
    if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) {
        New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
    }
    $records | Export-Csv -LiteralPath $outputPath -NoTypeInformation `
        -Encoding utf8

    Write-Host ""
    Write-Host "Summary:"
    $records |
        Group-Object backend, mode |
        ForEach-Object {
            $elapsed = $_.Group.elapsed_ms | Measure-Object -Average -Minimum -Maximum
            $workingSet = (
                $_.Group.peak_working_set_bytes |
                    Measure-Object -Average -Minimum -Maximum
            )
            Write-Host (
                "  $($_.Name): elapsed $([Math]::Round($elapsed.Average, 1)) ms " +
                "mean ($($elapsed.Minimum)-$($elapsed.Maximum)); peak working set " +
                "$([Math]::Round($workingSet.Average / 1MB, 2)) MiB mean"
            )
        }
    Write-Host "Raw results: $outputPath"
} finally {
    if (
        (Test-Path -LiteralPath $temporaryRoot -PathType Container) -and
        (Split-Path -Leaf $temporaryRoot).StartsWith("cefdetector-benchmark-")
    ) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}
