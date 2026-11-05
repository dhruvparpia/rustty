# Local release-tag check (issue #87). Remote check is optional and needs gh auth.
#
# Rules:
# - the tag exists;
# - the tag name's version matches Cargo.toml at that tagged commit;
# - a local ZIP artifact, if present, matches its .sha256 file;
# - duplicate tags are reported;
# - with -Remote, GitHub reports the release immutable and the asset digest matches.
#
# Run from the repo root:
#   powershell -ExecutionPolicy Bypass -File packaging/check-release.ps1 -Tag v0.5.1
#   powershell -ExecutionPolicy Bypass -File packaging/check-release.ps1 -Tag v0.5.1 -Remote

param(
    [string]$Tag = 'v0.5.1',
    [switch]$Remote
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
$hash = ''
if (Test-Path $zip) {
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash
    if (Test-Path $shaFile) {
        $recorded = (Get-Content $shaFile | Select-Object -First 1).Trim()
        if ($recorded -ne $hash) { throw "artifact SHA mismatch: $zip" }
    }
    "ARTIFACT $zip"
    "SHA256   $hash"
}

# Optional remote immutable-release check (requires gh + auth).
if ($Remote) {
    $json = gh release view $Tag --repo dhruvparpia/rustty --json tagName,isImmutable,assets 2>$null
    if ($LASTEXITCODE -ne 0) { throw "gh release view failed for $Tag" }
    $release = $json | ConvertFrom-Json
    if ($release.isImmutable -ne $true) { throw "remote release $Tag is not immutable" }
    $remoteAssets = @($release.assets | Where-Object { $_.name -eq "rustty-$version-win-x64.zip" })
    if ($remoteAssets.Count -eq 0) { throw "remote release $Tag has no zip asset" }
    $remoteZip = $remoteAssets[0]
    $remoteHash = ($remoteZip.digest -replace '^sha256:', '').ToLower()
    if ($hash -ne '' -and $remoteHash -ne $hash.ToLower()) {
        throw "remote SHA mismatch: $Tag -> $remoteHash, local $hash"
    }
    "REMOTE   $Tag immutable, asset $($remoteZip.name) sha256 $remoteHash"
}

"OK       tag $Tag -> commit " + (git rev-parse $Tag) + " -> version $version"
