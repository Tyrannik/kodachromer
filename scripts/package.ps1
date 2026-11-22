# Package release binaries and docs into dist\Remjet-<version>-windows-x64(.zip).
# Used by the GitHub Actions release workflow and for local builds.
# Expects `cargo build --release` to have run. Prints the package folder path.
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$BinDir = "target\release",
    [string]$OutDir = "dist",
    [switch]$NoZip
)
$ErrorActionPreference = "Stop"

$name = "Remjet-$Version-windows-x64"
$stage = Join-Path $OutDir $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item (Join-Path $BinDir "remjet.exe"), (Join-Path $BinDir "remjet-cli.exe") $stage
Copy-Item LICENSE, THIRD_PARTY.md, CHANGELOG.md $stage

@"
Remjet $Version (alpha) - film emulation for Leica M9 DNGs
=================================================================

Run remjet.exe, choose "Open folder..." and pick a folder of M9 DNGs.

- Your DNGs are never modified, and nothing is written into your photo folders.
- Edits, settings and tuned films are kept in %APPDATA%\Remjet.
- Exports go to an "exports" folder next to your DNGs by default.
- Remjet makes no network connections.

Shortcuts: Left/Right = previous/next, hold B = before/after, click image or Z = 100%,
Ctrl+wheel = zoom (to 800%), Esc = fit, Ctrl+Z / Ctrl+Y = undo / redo,
Ctrl+A / Ctrl+C / Ctrl+V = select all / copy / paste settings,
double-click a slider = reset it.

Command line: remjet-cli.exe export <folder> [--max-edge 2048] [--edited-only]

Project page, documentation and issues: https://github.com/Tyrannik/remjet
License: GNU GPL v3 or later (see LICENSE). Third-party notices: THIRD_PARTY.md.
Kodachrome and Kodak are trademarks of Eastman Kodak Company. This project is not
affiliated with or endorsed by Kodak or Leica.
"@ | Set-Content -Encoding utf8 (Join-Path $stage "README.txt")

if (-not $NoZip) {
    $zip = Join-Path $OutDir "$name.zip"
    if (Test-Path $zip) { Remove-Item $zip }
    Compress-Archive -Path $stage -DestinationPath $zip
}
$stage
