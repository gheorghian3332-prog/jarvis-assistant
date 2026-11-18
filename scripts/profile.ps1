[CmdletBinding()]
param(
    [string]$Exe,
    [string[]]$Args = @(),
    [ValidateRange(1, 86400)][int]$Seconds = 300,
    [string]$Out = 'profile.csv',
    [switch]$IncludeChildren,
    # Adds a GPU3D% column: summed 3D-engine utilization of the process tree (needs the Windows GPU Engine counters).
    [switch]$Gpu,
    # Attach to a process that is already running (it is left running afterwards) instead of starting -Exe.
    [int]$ProcessId
)
$ErrorActionPreference = 'Stop'

if ($ProcessId) {
    $process = Get-Process -Id $ProcessId
}
else {
    if (-not $Exe) { throw 'Give -Exe or -ProcessId.' }
    $resolvedExe = (Resolve-Path -LiteralPath $Exe).Path
    $startInfo = @{ FilePath = $resolvedExe; PassThru = $true }
    if ($Args.Count -gt 0) { $startInfo.ArgumentList = $Args }
    $process = Start-Process @startInfo
}

# Descendant lookup via Win32_Process.ParentProcessId; the tree is re-read at most every 5 s.
$script:treeCache = @($process.Id)
$script:treeAt = [DateTime]::MinValue
function Get-ProcessTreeIds([int]$RootId) {
    if (([DateTime]::UtcNow - $script:treeAt).TotalSeconds -lt 5) { return $script:treeCache }
    $children = @{}
    foreach ($item in (Get-CimInstance -ClassName Win32_Process -Property ProcessId, ParentProcessId)) {
        $parentId = [int]$item.ParentProcessId
        if (-not $children.ContainsKey($parentId)) { $children[$parentId] = New-Object System.Collections.Generic.List[int] }
        $children[$parentId].Add([int]$item.ProcessId)
    }
    $found = New-Object System.Collections.Generic.List[int]
    $queue = New-Object System.Collections.Generic.Queue[int]
    $queue.Enqueue($RootId)
    while ($queue.Count -gt 0) {
        $current = $queue.Dequeue()
        if ($found.Contains($current)) { continue }
        $found.Add($current)
        if ($children.ContainsKey($current)) { foreach ($child in $children[$current]) { $queue.Enqueue($child) } }
    }
    $script:treeCache = $found.ToArray()
    $script:treeAt = [DateTime]::UtcNow
    return $script:treeCache
}
$previousByPid = @{}

# GPU Engine counter set. Counter paths are English on every locale; the set may be missing (old drivers, VMs).
# Get-Counter blocks for its 1 s sample interval, so with -Gpu it replaces the per-iteration Start-Sleep.
$gpuPath = '\GPU Engine(*engtype_3D)\Utilization Percentage'
$script:gpuUsable = [bool]$Gpu
function Get-Gpu3dPercent([int[]]$ProcessIds) {
    try {
        $counter = Get-Counter -Counter $gpuPath -ErrorAction Stop
    }
    catch {
        $script:gpuUsable = $false
        return $null
    }
    $total = 0.0
    foreach ($sample in $counter.CounterSamples) {
        if ($sample.Path -match 'pid_(\d+)_' -and $ProcessIds -contains [int]$Matches[1]) { $total += $sample.CookedValue }
    }
    return [Math]::Round($total, 4)
}

$samples = New-Object System.Collections.Generic.List[object]
$cpuCount = [Environment]::ProcessorCount
$failure = $null
try {
    $process.Refresh()
    $previousCpu = $process.TotalProcessorTime
    $previousAt = [DateTime]::UtcNow
    for ($index = 0; $index -lt $Seconds; $index++) {
        $gpuPercent = $null
        if ($script:gpuUsable) { $gpuPercent = Get-Gpu3dPercent (Get-ProcessTreeIds $process.Id) }
        if ($null -eq $gpuPercent) { Start-Sleep -Seconds 1 }
        $process.Refresh()
        if ($process.HasExited) { throw "Process exited early with code $($process.ExitCode)." }
        $now = [DateTime]::UtcNow
        $elapsed = ($now - $previousAt).TotalSeconds
        if ($IncludeChildren) {
            $workingSet = 0L; $privateBytes = 0L; $threads = 0; $handles = 0; $count = 0; $cpuSeconds = 0.0
            foreach ($treeId in (Get-ProcessTreeIds $process.Id)) {
                $member = Get-Process -Id $treeId -ErrorAction SilentlyContinue
                if (-not $member) { continue }
                try {
                    $cpuTotal = $member.TotalProcessorTime.TotalSeconds
                    $workingSet += $member.WorkingSet64
                    $privateBytes += $member.PrivateMemorySize64
                    $threads += $member.Threads.Count
                    $handles += $member.HandleCount
                    $count++
                    $before = if ($previousByPid.ContainsKey($treeId)) { $previousByPid[$treeId] } elseif ($index -eq 0) { $cpuTotal } else { 0.0 }
                    $cpuSeconds += [Math]::Max(0.0, $cpuTotal - $before)
                    $previousByPid[$treeId] = $cpuTotal
                }
                catch { continue }
            }
            $cpu = ($cpuSeconds / $elapsed / $cpuCount) * 100
            $samples.Add([pscustomobject]@{
                ts           = $now.ToString('o')
                WorkingSet   = $workingSet
                PrivateBytes = $privateBytes
                'CPU%'       = [Math]::Round($cpu, 4)
                Threads      = $threads
                Handles      = $handles
                ProcCount    = $count
            })
            if ($Gpu) { $samples[$samples.Count - 1] | Add-Member -NotePropertyName 'GPU3D%' -NotePropertyValue $gpuPercent }
            $cpuNow = $process.TotalProcessorTime
        }
        else {
            $cpuNow = $process.TotalProcessorTime
            $cpu = (($cpuNow - $previousCpu).TotalSeconds / $elapsed / $cpuCount) * 100
            $samples.Add([pscustomobject]@{
                ts           = $now.ToString('o')
                WorkingSet   = $process.WorkingSet64
                PrivateBytes = $process.PrivateMemorySize64
                'CPU%'       = [Math]::Round($cpu, 4)
                Threads      = $process.Threads.Count
                Handles      = $process.HandleCount
            })
            if ($Gpu) { $samples[$samples.Count - 1] | Add-Member -NotePropertyName 'GPU3D%' -NotePropertyValue $gpuPercent }
        }
        $previousCpu = $cpuNow
        $previousAt = $now
    }
}
catch {
    $failure = $_
}
finally {
    if (-not $ProcessId -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        $process.WaitForExit()
    }
}

if ($samples.Count -gt 0) {
    $samples | Export-Csv -LiteralPath $Out -NoTypeInformation -Encoding UTF8

    function Get-Summary([string]$Name) {
        $values = @($samples | Where-Object { $null -ne $_.$Name -and "$($_.$Name)" -ne '' } | ForEach-Object { [double]$_.$Name } | Sort-Object)
        if ($values.Count -eq 0) { return [pscustomobject]@{ Metric = $Name; Min = ''; Avg = ''; P95 = ''; Max = '' } }
        $p95Index = [Math]::Min($values.Count - 1, [int][Math]::Ceiling($values.Count * 0.95) - 1)
        [pscustomobject]@{
            Metric = $Name
            Min    = [Math]::Round($values[0], 4)
            Avg    = [Math]::Round((($values | Measure-Object -Average).Average), 4)
            P95    = [Math]::Round($values[$p95Index], 4)
            Max    = [Math]::Round($values[$values.Count - 1], 4)
        }
    }
    Write-Host "Samples: $($samples.Count), logical CPUs: $cpuCount (WorkingSet/PrivateBytes in bytes)"
    $metrics = @('WorkingSet', 'PrivateBytes', 'CPU%', 'Threads', 'Handles')
    if ($IncludeChildren) { $metrics += 'ProcCount' }
    if ($Gpu) { $metrics += 'GPU3D%' }
    if ($Gpu -and -not $script:gpuUsable) { Write-Warning 'GPU Engine counters are unavailable; the GPU3D% column is empty.' }
    $metrics |
        ForEach-Object { Get-Summary $_ } | Format-Table -AutoSize | Out-String | Write-Host
    Write-Host "CSV: $Out"
}
if ($failure) {
    Write-Error $failure
    exit 1
}
