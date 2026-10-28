$ErrorActionPreference = 'Stop'
$root = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$saved = Get-Content -Raw -LiteralPath (Join-Path $root 'build\activation-before-acl.json') | ConvertFrom-Json
$current = Get-Acl -LiteralPath 'D:\AppData\Codex\UserProfile\.codex\config.toml'
if ($current.Sddl -ne $saved.Sddl -or $current.AreAccessRulesProtected -ne $saved.AreAccessRulesProtected) { throw 'Configuration ACL changed; do not reload' }
$record = @{ securityDescriptorIdentical=$true; inheritanceSettingPreserved=$true }
$record | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $root 'build\activation-acl-verification.json') -Encoding utf8
$record | ConvertTo-Json -Compress
