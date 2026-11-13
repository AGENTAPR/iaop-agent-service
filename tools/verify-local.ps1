<#
.SYNOPSIS
    Local verification for Agent Lifecycle - mirrors CI quality gates.
    Run from the repository root on Windows PowerShell.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$failed = $false

function Step($name, $script) {
    Write-Host "`n=== $name ===" -ForegroundColor Cyan
    & $script
    if ($LASTEXITCODE -ne 0) { $script:failed = $true; Write-Warning "$name FAILED" }
    else { Write-Host "$name OK" -ForegroundColor Green }
}

Step 'cargo fmt --check' { cargo fmt --all -- --check }
Step 'cargo clippy'      { cargo clippy --workspace --all-targets --all-features --locked -- -D warnings }
Step 'cargo test'        { cargo test --workspace --locked }
Step 'cargo build'       { cargo build --workspace --locked }

if ($failed) { exit 1 }
Write-Host "`nAll checks passed." -ForegroundColor Green
