# Measures the memory of the running Hyper Clips processes.
# The project target (< 60 MB service, < 150 MB UI) is defined on PRIVATE WORKING SET.
# Usage (PowerShell, while the app runs):  .\scripts\measure-memory.ps1

$names = @('hyperclips', 'hyperclips-ui')
foreach ($name in $names) {
    $proc = Get-Process -Name $name -ErrorAction SilentlyContinue
    if (-not $proc) { Write-Output "$name : not running"; continue }

    # "Working Set - Private" comes from the Process V2 performance counters (Windows 10 1809+).
    $counter = Get-Counter "\Process V2(${name}*)\Working Set - Private" -ErrorAction SilentlyContinue
    $privateBytes = ($counter.CounterSamples | Measure-Object CookedValue -Sum).Sum

    [pscustomobject]@{
        Process           = $name
        PID               = ($proc.Id -join ',')
        PrivateWS_MB      = if ($privateBytes) { [math]::Round($privateBytes / 1MB, 1) } else { 'n/a' }
        WorkingSet_MB     = [math]::Round(($proc | Measure-Object WorkingSet64 -Sum).Sum / 1MB, 1)
        PrivateCommit_MB  = [math]::Round(($proc | Measure-Object PrivateMemorySize64 -Sum).Sum / 1MB, 1)
    }
}
