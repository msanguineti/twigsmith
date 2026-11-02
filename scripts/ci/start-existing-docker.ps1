param(
    [Parameter(Mandatory = $true)][string]$EvidenceDirectory,
    [ValidateRange(1, 300)][int]$ReadyTimeoutSeconds = 60
)

# Start only the registered, enabled Docker service on this disposable CI host.
# Missing/disabled services stay unavailable; no install or configuration changes.
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$report = @{ passed = $false; action = 'none'; before = $null; after = $null }
$service = $null
function ServiceState($value) {
    @{ name = $value.Name; status = $value.Status.ToString(); startup_type = $value.StartType.ToString() }
}
try {
    $service = Get-Service -Name docker -ErrorAction Stop
    $report.before = ServiceState $service
    if ($service.StartType.ToString() -eq 'Disabled') { throw 'The existing Docker service is disabled; unchanged.' }
    if ($service.Status.ToString() -eq 'Stopped') {
        $report.action = 'start-existing-service-attempted'
        Start-Service -Name docker -ErrorAction Stop
        $service.WaitForStatus('Running', [TimeSpan]::FromSeconds(30))
        $report.action = 'started-existing-service'
    } elseif ($service.Status.ToString() -ne 'Running') {
        throw 'The existing Docker service is not running or stopped; unchanged.'
    }
    $service.Refresh()
    $report.after = ServiceState $service
    $docker = (Get-Command docker -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    $deadline = (Get-Date).AddSeconds($ReadyTimeoutSeconds)
    do {
        $output = (& $docker info --format '{{.OSType}}' 2>&1 | Out-String).Trim()
        $report.engine = @{ exit = $LASTEXITCODE; output = $output }
        if ($LASTEXITCODE -eq 0 -and $output -eq 'windows') {
            $report.passed = $true
            break
        }
        if ((Get-Date) -ge $deadline) { break }
        Start-Sleep -Seconds 2
    } while ($true)
} catch {
    $report.error = $_.Exception.Message
} finally {
    if ($null -ne $service) {
        try { $service.Refresh(); $report.after = ServiceState $service } catch {}
    }
    [void](New-Item -ItemType Directory -Path $EvidenceDirectory -Force)
    $path = Join-Path $EvidenceDirectory 'docker-service.json'
    [IO.File]::WriteAllText($path, ($report | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
}
if (!$report.passed) { exit 1 }
exit 0
