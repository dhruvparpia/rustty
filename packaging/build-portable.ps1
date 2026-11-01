# Build the portable ZIP artifact (SPEC PKG-001..004, NFR-305).
#
# ZIP layout (FR-103: extract the folder, run the exe, no install, no admin):
#   rusttyterm-<version>-win-x64/
#     rusttyterm.exe
#     rusttyterm.portable      <- selects local config (CONFIG-020, PKG-004)
#     config.example.toml
#     LICENSE                  <- MIT, in the package (OSS-003)
#     README.md
#
# Run from the repo root:
#   powershell -ExecutionPolicy Bypass -File packaging\build-portable.ps1
# Output: dist\rusttyterm-<version>-win-x64.zip + SHA-256.

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# cargo writes progress to stderr; keep EAP at Continue so PowerShell does
# not turn that into a terminating NativeCommandError, and trust the exit code.
$ErrorActionPreference = 'Continue'
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed ($LASTEXITCODE)" }
cargo test --release --lib 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "cargo test --release --lib failed ($LASTEXITCODE)" }
$ErrorActionPreference = 'Stop'

# Version comes from Cargo.toml, the single source of truth.
$version = (Select-String -Path Cargo.toml -Pattern '^\s*version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$name = "rusttyterm-$version-win-x64"
$stage = Join-Path $root "dist\$name"
$zip = Join-Path $root "dist\$name.zip"

if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
if (Test-Path $zip) { Remove-Item -Force $zip }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item target\release\rusttyterm.exe $stage
Copy-Item LICENSE, README.md, config.example.toml $stage
New-Item -ItemType File -Path (Join-Path $stage 'rusttyterm.portable') | Out-Null

Compress-Archive -Path $stage -DestinationPath $zip

$size = (Get-Item $zip).Length
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash
"ZIP    $zip"
"SIZE   $size bytes"
"SHA256 $hash"
