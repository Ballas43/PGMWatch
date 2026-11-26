$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

$msvcBin = Get-ChildItem "C:\Program Files\Microsoft Visual Studio\2022\*\VC\Tools\MSVC\*\bin\Hostx64\x64" -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
if ($msvcBin) {
    $env:PATH = "$msvcBin;$env:PATH"
}
if (Test-Path "C:\Program Files\LLVM\bin\libclang.dll") {
    $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
}
$ffmpegRoot = Join-Path $Root "vendor\ffmpeg-9.0"
if (Test-Path (Join-Path $ffmpegRoot "include\libavcodec\avcodec.h")) {
    $env:FFMPEG_DIR = $ffmpegRoot
    $env:FFMPEG_INCLUDE_DIR = Join-Path $ffmpegRoot "include"
    $env:FFMPEG_LIBS_DIR = Join-Path $ffmpegRoot "lib"
    $ffmpegBin = Join-Path $ffmpegRoot "bin"
    if (Test-Path $ffmpegBin) {
        $env:PATH = "$ffmpegBin;$env:PATH"
    }
}

cargo test --workspace --features pgmwatch-worker/ffmpeg-ffi
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
