$ErrorActionPreference = "Stop"

Push-Location (Join-Path $PSScriptRoot "..")
try {
    rustup toolchain install nightly-x86_64-pc-windows-msvc --profile minimal
    rustup +nightly-x86_64-pc-windows-msvc component add rust-src

    $registry = Join-Path $env:USERPROFILE ".cargo\registry\src"
    $windowsLib048 = Get-ChildItem -Path $registry -Recurse -Directory -Filter "windows_x86_64_msvc-0.48.5" |
        Select-Object -First 1 |
        ForEach-Object { Join-Path $_.FullName "lib" }

    if (-not $windowsLib048 -or -not (Test-Path (Join-Path $windowsLib048 "windows.0.48.5.lib"))) {
        throw "Could not find windows_x86_64_msvc-0.48.5 library directory."
    }

    $oldRustFlags = $env:RUSTFLAGS
    $env:RUSTFLAGS = "-C target-feature=+crt-static -C link-arg=/LIBPATH:$windowsLib048"

    cargo +nightly build `
        -Z build-std=std,panic_abort `
        --no-default-features `
        --release `
        --bin pyauto-rs-win7-native `
        --target x86_64-win7-windows-msvc

    Write-Host ""
    Write-Host "Win7 x64 executable:"
    $exe = "target\x86_64-win7-windows-msvc\release\pyauto-rs-win7-native.exe"
    if (-not (Test-Path $exe)) {
        throw "Build completed without producing $exe"
    }
    Write-Host (Resolve-Path $exe)
}
finally {
    $env:RUSTFLAGS = $oldRustFlags
    Pop-Location
}
