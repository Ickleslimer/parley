param(
    [string]$OutputDirectory = "artifacts\lab-chat"
)

$ErrorActionPreference = "Stop"
$viewerRoot = Split-Path -Parent $PSScriptRoot
$edgeCandidates = @(
    "C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    "C:\Program Files\Microsoft\Edge\Application\msedge.exe"
)
$edge = $edgeCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $edge) {
    throw "Microsoft Edge was not found"
}
$node = (Get-Command node.exe -ErrorAction Stop).Source
$serverScript = Join-Path $viewerRoot "scripts\fixture-server.mjs"
if (-not (Test-Path -LiteralPath (Join-Path $viewerRoot "node_modules\vite"))) {
    throw "Run npm ci before capturing fixtures"
}
$output = if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory
} else {
    Join-Path $viewerRoot $OutputDirectory
}
New-Item -ItemType Directory -Path $output -Force | Out-Null
$resolvedOutput = [System.IO.Path]::GetFullPath($output)
$edgeProfile = Join-Path $resolvedOutput "edge-profile"
New-Item -ItemType Directory -Path $edgeProfile -Force | Out-Null
$serverOut = Join-Path $output "vite.stdout.log"
$serverErr = Join-Path $output "vite.stderr.log"
$server = Start-Process -FilePath $node `
    -ArgumentList @($serverScript) `
    -WorkingDirectory $viewerRoot `
    -WindowStyle Hidden `
    -RedirectStandardOutput $serverOut `
    -RedirectStandardError $serverErr `
    -PassThru

$fixtures = @(
    @{ File = "underlay-live-720x560.png"; Query = "view=widget&fixture=live"; Width = 720; Height = 560 },
    @{ File = "underlay-minimum-480x420.png"; Query = "view=widget&fixture=live"; Width = 480; Height = 420 },
    @{ File = "underlay-large-960x720.png"; Query = "view=widget&fixture=live"; Width = 960; Height = 720 },
    @{ File = "surface-live-720x560.png"; Query = "view=widget-surface&fixture=live"; Width = 720; Height = 560 },
    @{ File = "surface-paused-unread-720x560.png"; Query = "view=widget-surface&fixture=paused-unread"; Width = 720; Height = 560 },
    @{ File = "surface-collapsed-720x560.png"; Query = "view=widget-surface&fixture=collapsed"; Width = 720; Height = 560 },
    @{ File = "surface-expanded-720x560.png"; Query = "view=widget-surface&fixture=expanded"; Width = 720; Height = 560 },
    @{ File = "surface-pending-720x560.png"; Query = "view=widget-surface&fixture=pending"; Width = 720; Height = 560 },
    @{ File = "surface-completion-720x560.png"; Query = "view=widget-surface&fixture=completion"; Width = 720; Height = 560 },
    @{ File = "surface-error-720x560.png"; Query = "view=widget-surface&fixture=error"; Width = 720; Height = 560 },
    @{ File = "surface-unknown-agent-720x560.png"; Query = "view=widget-surface&fixture=unknown-agent"; Width = 720; Height = 560 },
    @{ File = "surface-empty-720x560.png"; Query = "view=widget-surface&fixture=empty"; Width = 720; Height = 560 },
    @{ File = "surface-missing-image-720x560.png"; Query = "view=widget-surface&fixture=missing-image"; Width = 720; Height = 560 },
    @{ File = "surface-pending-reduced-motion-720x560.png"; Query = "view=widget-surface&fixture=pending"; Width = 720; Height = 560; ReducedMotion = $true },
    @{ File = "surface-text-scale-200-720x560.png"; Query = "view=widget-surface&fixture=live&text-scale=2"; Width = 720; Height = 560 },
    @{ File = "surface-minimum-480x420.png"; Query = "view=widget-surface&fixture=live"; Width = 480; Height = 420 },
    @{ File = "surface-large-960x720.png"; Query = "view=widget-surface&fixture=live"; Width = 960; Height = 720 },
    @{ File = "detail-event-1120x760.png"; Query = "view=detail&fixture=short-exchange&tab=event"; Width = 1120; Height = 760 },
    @{ File = "detail-activity-840x560.png"; Query = "view=detail&fixture=short-exchange&tab=activity"; Width = 840; Height = 560 },
    @{ File = "detail-sources-search-1120x760.png"; Query = "view=detail&fixture=search-results&tab=sources"; Width = 1120; Height = 760 },
    @{ File = "detail-settings-empty-1120x760.png"; Query = "view=detail&fixture=empty&tab=settings"; Width = 1120; Height = 760 },
    @{ File = "detail-error-840x560.png"; Query = "view=detail&fixture=source-error&tab=event"; Width = 840; Height = 560 },
    @{ File = "detail-reduced-motion-1120x760.png"; Query = "view=detail&fixture=maximum-exchange&tab=event"; Width = 1120; Height = 760; ReducedMotion = $true },
    @{ File = "detail-text-scale-200-1120x760.png"; Query = "view=detail&fixture=maximum-exchange&tab=event&text-scale=2"; Width = 1120; Height = 760 }
)

try {
    $ready = $false
    for ($attempt = 0; $attempt -lt 40; $attempt++) {
        if ($server.HasExited) {
            throw "Vite exited before becoming ready. See $serverErr"
        }
        try {
            $response = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:1420" -TimeoutSec 1
            if ($response.StatusCode -eq 200) {
                $ready = $true
                break
            }
        } catch {
            Start-Sleep -Milliseconds 250
        }
    }
    if (-not $ready) {
        throw "Vite did not become ready"
    }
    foreach ($fixture in $fixtures) {
        $path = Join-Path $output $fixture.File
        $arguments = @(
            "--headless=new",
            "--disable-gpu",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-extensions",
            "--no-first-run",
            "--no-proxy-server",
            "--run-all-compositor-stages-before-draw",
            "--user-data-dir=$edgeProfile",
            "--virtual-time-budget=3000",
            "--window-size=$($fixture.Width),$($fixture.Height)",
            "--screenshot=$path"
        )
        if ($fixture.ReducedMotion) {
            $arguments += "--force-prefers-reduced-motion"
        }
        $arguments += "http://127.0.0.1:1420/?$($fixture.Query)"
        $edgeProcess = Start-Process -FilePath $edge -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
        if ($edgeProcess.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $path)) {
            throw "Edge failed to capture $($fixture.File)"
        }
    }
    $manifest = Get-ChildItem -LiteralPath $output -Filter "*.png" | Sort-Object Name | ForEach-Object {
        $hash = Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName
        [pscustomobject]@{ file = $_.Name; bytes = $_.Length; sha256 = $hash.Hash.ToLowerInvariant() }
    }
    $manifest | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $output "manifest.json") -Encoding utf8
} finally {
    if (-not $server.HasExited) {
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:1420/__fixture_shutdown" -TimeoutSec 2 | Out-Null
        } catch {
        }
        if (-not $server.WaitForExit(5000)) {
            Stop-Process -Id $server.Id -Force
            $server.WaitForExit()
        }
    }
    $resolvedProfile = [System.IO.Path]::GetFullPath($edgeProfile)
    if ($resolvedProfile.StartsWith($resolvedOutput + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $resolvedProfile)) {
        Remove-Item -LiteralPath $resolvedProfile -Recurse -Force
    }
}
