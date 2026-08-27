$ErrorActionPreference = "Stop"

$toolchain = "nightly-2026-06-30"
$repository = "https://github.com/themixednuts/mlua-typegen"

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    throw "rustup is required; install Rust from https://rustup.rs and retry"
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo is not on PATH; restart PowerShell after installing rustup"
}

$componentArgs = @("--component", "rustc-dev")
if ($env:OS -ne "Windows_NT") {
    $componentArgs += @("--component", "llvm-tools-preview")
}

& rustup toolchain install $toolchain --profile minimal @componentArgs
if ($LASTEXITCODE -ne 0) {
    throw "rustup could not install $toolchain"
}

& cargo "+$toolchain" install `
    --git $repository `
    --locked `
    --force
if ($LASTEXITCODE -ne 0) {
    throw "cargo could not install mlua-typegen"
}

& cargo mlua-typegen --version
if ($LASTEXITCODE -ne 0) {
    throw "cargo installed mlua-typegen, but the Cargo subcommand could not start"
}

Write-Output "Run 'cargo mlua-typegen' in any Cargo workspace."
