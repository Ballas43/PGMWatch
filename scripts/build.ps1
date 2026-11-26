param(
    [switch]$Release,
    [switch]$WithFfmpegFfi,
    [switch]$SkipUi
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

# 1. Prepend VS 2022 MSVC Hostx64\x64 tools ahead of PATH so GNU coreutils link.exe never shadows MSVC link.exe
$msvcBin = Get-ChildItem "C:\Program Files\Microsoft Visual Studio\2022\*\VC\Tools\MSVC\*\bin\Hostx64\x64" -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
if ($msvcBin) {
    $env:PATH = "$msvcBin;$env:PATH"
    Write-Host "MSVC linker prepended: $msvcBin"
}

# 2. Scope LLVM + FFmpeg 9.0 LGPL env strictly to this build session
if (Test-Path "C:\Program Files\LLVM\bin\libclang.dll") {
    $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
}
$ffmpegRoot = Join-Path $Root "vendor\ffmpeg-9.0"
if (Test-Path (Join-Path $ffmpegRoot "include\libavcodec\avcodec.h")) {
    $env:FFMPEG_DIR = $ffmpegRoot
    $env:FFMPEG_INCLUDE_DIR = Join-Path $ffmpegRoot "include"
    $env:FFMPEG_LIBS_DIR = Join-Path $ffmpegRoot "lib"
}

# 3. Build Embedded React UI unless skipped
if (-not $SkipUi) {
    Write-Host "`n=== Building UI (ui/) ==="
    pnpm --dir ui build
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

# 4. Build Bitfocus Companion TypeScript module if dist is missing or full build
if (-not $SkipUi -or -not (Test-Path (Join-Path $Root "companion-module\dist"))) {
    Write-Host "`n=== Building Companion module (companion-module/) ==="
    pnpm --dir companion-module build
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

# 5. Build Rust workspace
$cargoArgs = @("build", "--workspace")
if ($Release) {
    $cargoArgs += "--release"
}
if ($WithFfmpegFfi -and $env:LIBCLANG_PATH -and $env:FFMPEG_DIR) {
    Write-Host "`n=== Building Rust workspace with FFmpeg 9.0 FFI ==="
    cargo @cargoArgs --features pgmwatch-worker/ffmpeg-ffi
} else {
    Write-Host "`n=== Building Rust workspace ==="
    cargo @cargoArgs
}
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "`nBuild completed successfully."
