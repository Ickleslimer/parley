param(
    [string]$OutputDirectory = "artifacts\conversation-studio"
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
$vite = Join-Path $viewerRoot "node_modules\vite\bin\vite.js"
if (-not (Test-Path -LiteralPath $vite)) {
    throw "Run npm ci before capturing fixtures"
}
$output = if ([System.IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory
} else {
    Join-Path $viewerRoot $OutputDirectory
}
New-Item -ItemType Directory -Path $output -Force | Out-Null
$serverOut = Join-Path $output "vite.stdout.log"
$serverErr = Join-Path $output "vite.stderr.log"
$server = Start-Process -FilePath $node `
    -ArgumentList @($vite, "--host", "127.0.0.1", "--port", "1420", "--strictPort") `
    -WorkingDirectory $viewerRoot `
    -WindowStyle Hidden `
    -RedirectStandardOutput $serverOut `
    -RedirectStandardError $serverErr `
    -PassThru

$fixtures = @(
    @{ File = "widget-short-560x360.png"; Query = "view=widget&fixture=short-exchange"; Width = 560; Height = 360 },
    @{ File = "widget-maximum-560x360.png"; Query = "view=widget&fixture=maximum-exchange"; Width = 560; Height = 360 },
    @{ File = "widget-compact-320x180.png"; Query = "view=widget&fixture=short-exchange"; Width = 320; Height = 180 },
    @{ File = "widget-reversed-560x360.png"; Query = "view=widget&fixture=reversed-route"; Width = 560; Height = 360 },
    @{ File = "widget-unknown-560x360.png"; Query = "view=widget&fixture=unknown-agent"; Width = 560; Height = 360 },
    @{ File = "widget-pending-560x360.png"; Query = "view=widget&fixture=pending"; Width = 560; Height = 360 },
    @{ File = "widget-error-560x360.png"; Query = "view=widget&fixture=error"; Width = 560; Height = 360 },
    @{ File = "widget-idle-560x360.png"; Query = "view=widget&fixture=idle"; Width = 560; Height = 360 },
    @{ File = "widget-missing-image-560x360.png"; Query = "view=widget&fixture=missing-image"; Width = 560; Height = 360 },
    @{ File = "detail-event-1120x760.png"; Query = "view=detail&fixture=short-exchange&tab=event"; Width = 1120; Height = 760 },
    @{ File = "detail-activity-840x560.png"; Query = "view=detail&fixture=short-exchange&tab=activity"; Width = 840; Height = 560 },
    @{ File = "detail-sources-search-1120x760.png"; Query = "view=detail&fixture=search-results&tab=sources"; Width = 1120; Height = 760 },
    @{ File = "detail-settings-empty-1120x760.png"; Query = "view=detail&fixture=empty&tab=settings"; Width = 1120; Height = 760 },
    @{ File = "detail-error-840x560.png"; Query = "view=detail&fixture=source-error&tab=event"; Width = 840; Height = 560 },
    @{ File = "detail-reduced-motion-1120x760.png"; Query = "view=detail&fixture=maximum-exchange&tab=event&text-scale=2"; Width = 1120; Height = 760; ReducedMotion = $true }
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
            "--hide-scrollbars",
            "--run-all-compositor-stages-before-draw",
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
        Stop-Process -Id $server.Id -Force
        $server.WaitForExit()
    }
}
