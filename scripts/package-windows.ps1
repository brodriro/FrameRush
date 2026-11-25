# Genera el instalador NSIS de FrameRush (GUI, CLI y helper), sin firmar.
#
# Compila en release la CLI y el helper, los copia como sidecars de Tauri
# (`binaries/<nombre>-<triple>.exe`, que el instalador deja junto a la GUI) y ejecuta
# `cargo tauri build` con `tauri.bundle.json` encima de la configuración normal. La carpeta
# de compilación sale de `cargo metadata`, así que respeta CARGO_TARGET_DIR.
#
# Uso (PowerShell 7): pwsh -NoProfile -File scripts\package-windows.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tauri = Join-Path $root 'apps\gui\src-tauri'

function Invoke-Checked {
    & $args[0] $args[1..($args.Length - 1)]
    if ($LASTEXITCODE -ne 0) { throw "falló: $($args -join ' ')" }
}

Push-Location $root
try {
    $triple = (rustc --print host-tuple).Trim()
    $target = (cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).target_directory

    Invoke-Checked cargo build --release --locked -p framerush-cli -p framerush-helper

    $bins = Join-Path $tauri 'binaries'
    New-Item -ItemType Directory -Force $bins | Out-Null
    foreach ($name in 'framerush', 'framerush-helper') {
        Copy-Item (Join-Path $target "release\$name.exe") (Join-Path $bins "$name-$triple.exe") -Force
    }

    Set-Location $tauri
    Invoke-Checked cargo tauri build --config (Join-Path $tauri 'tauri.bundle.json')

    Get-ChildItem (Join-Path $target 'release\bundle\nsis\*-setup.exe') |
        ForEach-Object { "Instalador: $($_.FullName)" }
}
finally {
    Pop-Location
}
