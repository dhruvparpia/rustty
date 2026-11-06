# Build the portable ZIP artifact (SPEC PKG-001..004, NFR-305).
#
# ZIP layout (FR-103: extract the folder, run the exe, no install, no admin):
#   rustty-<version>-win-x64/
#     rustty.exe
#     rustty-host.exe      <- persistent session host (HOST-001)
#     rustty.portable      <- selects local config (CONFIG-020, PKG-004)
#     config.example.toml
#     LICENSE                  <- MIT, in the package (OSS-003)
#     fonts/                   <- licence for the embedded symbols font
#     README.md
#
# Run from the repo root:
#   powershell -ExecutionPolicy Bypass -File packaging\build-portable.ps1
#   powershell -ExecutionPolicy Bypass -File packaging\build-portable.ps1 -Arch arm64
# Output: dist\rustty-<version>-win-x64.zip (+ -win-arm64) + SHA-256.

param(
    # 'x64' (default) or 'arm64' (roadmap v0.5 Windows ARM64).
    [ValidateSet('x64', 'arm64')]
    [string]$Arch = 'x64'
)

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# cargo writes progress to stderr; keep EAP at Continue so PowerShell does
# not turn that into a terminating NativeCommandError, and trust the exit code.
$ErrorActionPreference = 'Continue'
if ($Arch -eq 'arm64') {
    # Cross-built here, run-tested only on ARM64 hardware; the CI gate is
    # cargo check --target aarch64-pc-windows-msvc.
    # shortcut: no cargo test for the cross target (can't run on an x64
    # host); upgrade when ARM64 CI runners exist.
    cargo build --release --target aarch64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw "cargo build (arm64) failed ($LASTEXITCODE)" }
    $bindir = 'target\aarch64-pc-windows-msvc\release'
} else {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed ($LASTEXITCODE)" }
    cargo test --release --lib 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "cargo test --release --lib failed ($LASTEXITCODE)" }
    $bindir = 'target\release'
}
$ErrorActionPreference = 'Stop'

# Version comes from Cargo.toml, the single source of truth.
$version = (Select-String -Path Cargo.toml -Pattern '^\s*version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
$name = "rustty-$version-win-$Arch"
$stage = Join-Path $root "dist\$name"
$zip = Join-Path $root "dist\$name.zip"

if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
if (Test-Path $zip) { Remove-Item -Force $zip }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item "$bindir\rustty.exe" $stage
Copy-Item "$bindir\rustty-host.exe" $stage
Copy-Item LICENSE, README.md, config.example.toml $stage
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'fonts') | Out-Null
Copy-Item assets\fonts\LICENSE, assets\fonts\README.md (Join-Path $stage 'fonts')
New-Item -ItemType File -Path (Join-Path $stage 'rustty.portable') | Out-Null

Compress-Archive -Path $stage -DestinationPath $zip

$size = (Get-Item $zip).Length
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash
"ZIP    $zip"
"SIZE   $size bytes"
"SHA256 $hash"
