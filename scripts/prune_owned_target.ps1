$ErrorActionPreference = 'Stop'
$workspace = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
if ($workspace -ne 'D:\Workplaces\R3 Bio\Parley-root\.parley-worktrees\Parley\paginated-context-history') { throw 'Unexpected workspace' }
if (@(Get-Process cargo,rustc -ErrorAction SilentlyContinue).Count) { throw 'Rust compiler active' }
$target = (Resolve-Path -LiteralPath (Join-Path $workspace 'target')).Path
if ($target -ne ($workspace + '\target')) { throw 'Unexpected target' }
$directories = @(Get-Item -LiteralPath $target) + @(Get-ChildItem -LiteralPath $target -Directory -Recurse -Force)
if (@($directories | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }).Count) { throw 'Redirected target' }
$remove = [Collections.Generic.List[string]]::new()
foreach ($directory in (Get-ChildItem -LiteralPath $target -Directory -Recurse -Filter incremental)) { $remove.Add($directory.FullName) }
foreach ($directory in (Get-ChildItem -LiteralPath $target -Directory -Recurse -Filter deps)) {
    $files = @(Get-ChildItem -LiteralPath $directory.FullName -File | ForEach-Object {
        if ($_.Name -match '^(?<crate>.+)-(?<hash>[a-f0-9]{16})(?:\..*)?$') {
            [pscustomobject]@{ Path=$_.FullName; Crate=$Matches.crate; Hash=$Matches.hash; Modified=$_.LastWriteTimeUtc }
        }
    })
    foreach ($group in ($files | Group-Object Crate)) {
        $newest = ($group.Group | Sort-Object Modified -Descending | Select-Object -First 1).Hash
        foreach ($file in $group.Group) { if ($file.Hash -ne $newest) { $remove.Add($file.Path) } }
    }
}
foreach ($path in $remove) {
    $resolved = (Resolve-Path -LiteralPath $path).Path
    if (-not $resolved.StartsWith($target + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Escaped target' }
    if ((Get-Item -LiteralPath $resolved).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Redirected removal' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
$record = @{ RemovedCount=$remove.Count; Paths=@($remove); FreeBytes=(Get-PSDrive D).Free }
$receipt = Join-Path $workspace ('build\owned-target-prune-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.json')
$record | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $receipt -Encoding utf8
@{ RemovedCount=$remove.Count; FreeBytes=(Get-PSDrive D).Free; Receipt=$receipt } | ConvertTo-Json -Compress
