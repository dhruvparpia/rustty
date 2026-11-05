# Local release-tag check (issue #87). No GitHub token is used.
#
# Rules:
# - the tag exists;
# - the tag name's version matches Cargo.toml at that tagged commit;
# - a local ZIP artifact, if present, matches its .sha256 file;
# - duplicate tags are reported.
#
# Run from the repo root:
#   powershell -ExecutionPolicy Bypass -File packaging/check-release.ps1 -Tag v0.5.1

param(
    [string]$Tag = 'v0.5.1'
)

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$ErrorActionPreference = 'Continue'
git rev-parse --verify $Tag > $null 2>&1
if ($LASTEXITCODE -ne 0) { throw "tag $Tag does not exist" }

$tagVersion = $Tag -replace '^v', ''
$line = git show "${Tag}:Cargo.toml" 2>$null | Select-String -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (!$line) { throw "tag $Tag has no version line" }
$version = $line.Matches[0].Groups[1].Value
if ($version -ne $tagVersion) {
    throw "tag/version mismatch: tag $Tag -> Cargo.toml $version"
}

# Report duplicate tag names (immutable-release hygiene).
$dups = git tag -l | Group-Object | Where-Object { $_.Count -gt 1 }
if ($dups) {
    throw "duplicate tag names: " + ($dups | ForEach-Object { $_.Name } | Join-String -Separator ', ')
}

# Optional local artifact check.
$zip = Join-Path $root "dist\rustty-$version-win-x64.zip"
$shaFile = "$zip.sha256"
if (Test-Path $zip) {
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash
    if (Test-Path $shaFile) {
        $recorded = (Get-Content $shaFile | Select-Object -First 1).Trim()
        if ($recorded -ne $hash) { throw "artifact SHA mismatch: $zip" }
    }
    "ARTIFACT $zip"
    "SHA256   $hash"
}

"OK       tag $Tag -> commit " + (git rev-parse $Tag) + " -> version $version"
