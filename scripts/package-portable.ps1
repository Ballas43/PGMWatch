param(
    [switch]$WithFfmpegFfi,
    [switch]$SkipBuild,
    [switch]$SkipUi,
    [switch]$NoZip
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

if (-not $SkipBuild) {
    & (Join-Path $PSScriptRoot "build.ps1") -Release -WithFfmpegFfi:$WithFfmpegFfi -SkipUi:$SkipUi
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$DistRoot = Join-Path $Root "dist\PGMWatch"
$StageRoot = Join-Path $Root "dist\_stage_zip\PGMWatch"
$ZipPath = Join-Path $Root "dist\PGMWatch-Portable.zip"

function Copy-AppPayload([string]$TargetDir, [bool]$FastUpdate) {
    New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null
    foreach ($sub in @("bin", "sidecar-stats", "companion-module-pgmwatch")) {
        New-Item -ItemType Directory -Force -Path (Join-Path $TargetDir $sub) | Out-Null
    }

    $ReleaseDir = Join-Path $Root "target\release"
    Copy-Item -Force (Join-Path $ReleaseDir "PGMWatch.exe") $TargetDir
    Copy-Item -Force (Join-Path $ReleaseDir "pgmwatch-worker.exe") $TargetDir
    if (-not (Test-Path (Join-Path $TargetDir "pgmwatch.toml"))) {
        Copy-Item (Join-Path $Root "pgmwatch.toml") $TargetDir
    }
    if (Test-Path (Join-Path $Root "LICENSE")) {
        Copy-Item -Force (Join-Path $Root "LICENSE") $TargetDir
    }
    if (Test-Path (Join-Path $Root "THIRD_PARTY_NOTICES.txt")) {
        Copy-Item -Force (Join-Path $Root "THIRD_PARTY_NOTICES.txt") $TargetDir
    }
    $FfmpegLicense = Join-Path $Root "vendor\ffmpeg-9.0\LICENSE.txt"
    if (Test-Path $FfmpegLicense) {
        Copy-Item -Force $FfmpegLicense (Join-Path $TargetDir "FFMPEG-LICENSE.txt")
    }

    # Copy FFmpeg 9.0 LGPL shared DLLs + ffmpeg.exe if missing or size changed
    $FfmpegBin = Join-Path $Root "vendor\ffmpeg-9.0\bin"
    if (Test-Path $FfmpegBin) {
        Get-ChildItem $FfmpegBin | Where-Object { $_.Extension -eq ".dll" -or $_.Name -eq "ffmpeg.exe" } | ForEach-Object {
            $dstFile = Join-Path $TargetDir $_.Name
            if ((-not (Test-Path $dstFile)) -or ((Get-Item $dstFile).Length -ne $_.Length)) {
                Copy-Item -Force $_.FullName $dstFile
            }
        }
    }

    # Copy portable bin tools (yt-dlp.exe, streamlink, node.exe) when missing or size changed
    $RuntimeBin = Join-Path $Root "runtime\bin"
    if (Test-Path $RuntimeBin) {
        Get-ChildItem $RuntimeBin | ForEach-Object {
            $dstItem = Join-Path (Join-Path $TargetDir "bin") $_.Name
            if ($_.PSIsContainer) {
                if (-not (Test-Path $dstItem)) {
                    Copy-Item -Recurse -Force $_.FullName $dstItem
                }
            } elseif ((-not (Test-Path $dstItem)) -or ((Get-Item $dstItem).Length -ne $_.Length)) {
                Copy-Item -Force $_.FullName $dstItem
            }
        }
    }

    # Copy sidecar JS + Companion dist (skip redundant node_modules tree copy when already present in fast mode)
    if ($FastUpdate -and (Test-Path (Join-Path $TargetDir "sidecar-stats\node_modules"))) {
        Copy-Item -Force (Join-Path $Root "sidecar-stats\index.js") (Join-Path $TargetDir "sidecar-stats")
        Copy-Item -Force (Join-Path $Root "sidecar-stats\package.json") (Join-Path $TargetDir "sidecar-stats")
    } else {
        Copy-Item -Recurse -Force (Join-Path $Root "sidecar-stats\*") (Join-Path $TargetDir "sidecar-stats")
    }
    $CompTarget = Join-Path $TargetDir "companion-module-pgmwatch"
    Copy-Item -Force (Join-Path $Root "companion-module-pgmwatch\package.json") $CompTarget
    foreach ($subDir in @("dist", "companion")) {
        $srcSub = Join-Path $Root "companion-module-pgmwatch\$subDir"
        $dstSub = Join-Path $CompTarget $subDir
        if (Test-Path $srcSub) {
            if (Test-Path $dstSub) { Remove-Item -Recurse -Force $dstSub }
            Copy-Item -Recurse -Force $srcSub $dstSub
        }
    }
    # Bundle flat production node_modules (dereferenced from pnpm) so Companion Developer Modules work offline in the portable zip
    $CompNodeModules = Join-Path $CompTarget "node_modules"
    if ((-not $FastUpdate) -or (-not (Test-Path (Join-Path $CompNodeModules "@companion-module\base\package.json")))) {
        if (Test-Path $CompNodeModules) { Remove-Item -Recurse -Force $CompNodeModules }
        New-Item -ItemType Directory -Force -Path $CompNodeModules | Out-Null
        $SrcMods = Join-Path $Root "companion-module-pgmwatch\node_modules"
        $SrcHoisted = Join-Path $SrcMods ".pnpm\node_modules"
        foreach ($dep in @("@companion-module", "ws")) {
            $depSrc = Join-Path $SrcMods $dep
            if (Test-Path $depSrc) {
                Copy-Item -Recurse -Force $depSrc (Join-Path $CompNodeModules $dep)
            }
        }
        if (Test-Path $SrcHoisted) {
            Get-ChildItem $SrcHoisted | Where-Object { $_.Name -notin @(".bin", "undici-types") } | ForEach-Object {
                $dst = Join-Path $CompNodeModules $_.Name
                if (-not (Test-Path $dst)) {
                    Copy-Item -Recurse -Force $_.FullName $dst
                }
            }
        }
    }
}

# Update dist/PGMWatch in place so user's saved dist/PGMWatch/data (sentinel.db, presets, profiles, clips) is NEVER deleted
Copy-AppPayload $DistRoot $true

if ($NoZip) {
    Write-Host "`nFast folder update ready (data/ preserved, zip skipped): $DistRoot"
    exit 0
}

# Build pristine distribution zip without bundling local data/
$StageParent = Split-Path -Parent $StageRoot
if (Test-Path $StageParent) { Remove-Item -Recurse -Force $StageParent }
Copy-AppPayload $StageRoot $false
Copy-Item -Force (Join-Path $Root "pgmwatch.toml") $StageRoot

if (Test-Path $ZipPath) { Remove-Item -Force $ZipPath }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[System.IO.Compression.ZipFile]::CreateFromDirectory($StageRoot, $ZipPath, [System.IO.Compression.CompressionLevel]::Fastest, $true)
Remove-Item -Recurse -Force $StageParent

Write-Host "`nPortable package ready (saved data/ preserved):"
Write-Host "  Folder: $DistRoot"
Write-Host "  Zip:    $ZipPath ($([math]::Round((Get-Item $ZipPath).Length / 1MB, 1)) MB)"
