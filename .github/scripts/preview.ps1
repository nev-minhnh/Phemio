# Runs Phemio a few times — collapsed, on each island page, and on each Settings
# tab — with ISLET_DUMP on, and turns the last frame of each window into a PNG.
param([string]$Exe = "app/Phemio.exe", [string]$Out = "previews")
$ErrorActionPreference = "Continue"
$exe = (Resolve-Path $Exe).Path
New-Item -ItemType Directory -Force $Out | Out-Null
$out = (Resolve-Path $Out).Path
$data = Join-Path $env:APPDATA "Phemio"

function Run-Phemio([string]$name, [hashtable]$envs, [string]$config, [int]$secs = 9) {
    Get-Process phemio -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    New-Item -ItemType Directory -Force $data | Out-Null
    Set-Content -Path (Join-Path $data "config.ini") -Value "# Phemio settings`nautostart=0`nlink_on=0`n$config" -Encoding utf8
    $dump = Join-Path $env:RUNNER_TEMP "dump-$name"
    New-Item -ItemType Directory -Force $dump | Out-Null
    $env:ISLET_DUMP = $dump
    foreach ($k in $envs.Keys) { Set-Item "env:$k" $envs[$k] }
    $p = Start-Process -FilePath $exe -PassThru
    Start-Sleep -Seconds $secs
    foreach ($k in $envs.Keys) { Remove-Item "env:$k" -ErrorAction SilentlyContinue }
    Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    python .github/scripts/bgra2png.py $dump (Join-Path $out $name)
}

Run-Phemio "collapsed" @{} "island_always=0" 12
Run-Phemio "collapsed-island" @{} "island_always=0`nisland_style=0" 8
for ($i = 0; $i -lt 12; $i++) {
    Run-Phemio ("page{0:d2}" -f $i) @{ ISLET_OPEN_PAGE = "$i" } "island_always=1"
}
for ($t = 0; $t -lt 7; $t++) {
    Run-Phemio "settings-dark-$t" @{ ISLET_OPEN_SETTINGS = "$t" } "theme=1" 7
}
Run-Phemio "settings-light-0" @{ ISLET_OPEN_SETTINGS = "0" } "theme=2" 7
Run-Phemio "settings-light-2" @{ ISLET_OPEN_SETTINGS = "2" } "theme=2" 7
Get-Process phemio -ErrorAction SilentlyContinue | Stop-Process -Force
Get-ChildItem $out | Select-Object Name, Length | Format-Table -AutoSize
Copy-Item (Join-Path $data "phemio.log") $out -ErrorAction SilentlyContinue
