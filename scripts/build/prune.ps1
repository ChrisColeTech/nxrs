<#
.SYNOPSIS
  Keeps target/ small without throwing away the release build you are running.
.DESCRIPTION
  Works on the default profile folders: target/debug, target/release and target/<triple>/<profile>.
  The lane folders other work builds into (target/L2, target/K, ...: CARGO_TARGET_DIR) are left alone
  unless -Lanes is given.
  Always (every run):
    1. Deletes every incremental cache.
    2. Deletes superseded artifacts: in each deps/ folder, for every crate+extension, every build hash
       except the newest. Old hashes pile up (rustc, feature and profile changes).
    3. cargo sweep --time -KeepDays, when cargo-sweep is installed.
  Only while the default folders are still over -BudgetGB: deletes target/debug.
  Never deletes the release executables and engine libraries. Refuses to run while cargo or rustc is
  running.
.EXAMPLE
  scripts/build/prune.ps1 -BudgetGB 5 -KeepDays 7 -WhatIf
  scripts/build/prune.ps1 -Lanes          # also the lane folders' incremental caches and old hashes
#>
[CmdletBinding()]
param(
    [double]$BudgetGB = 5,
    [int]$KeepDays = 14,
    [switch]$Lanes,
    [switch]$WhatIf
)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$target = Join-Path $root 'target'
if (-not (Test-Path $target)) { Write-Host "no target dir at $target"; return }
if (Get-Process cargo, rustc -ErrorAction SilentlyContinue) {
    throw 'cargo or rustc is running; prune after the build finishes.'
}

function Get-SizeGB([string[]]$paths) {
    $sum = ($paths | Where-Object { Test-Path $_ } | ForEach-Object {
            Get-ChildItem $_ -Recurse -File -Force -ErrorAction SilentlyContinue }) |
        Measure-Object Length -Sum
    [math]::Round(($sum.Sum / 1GB), 2)
}

function Remove-Tree([string]$path) {
    if (-not (Test-Path $path)) { return }
    Write-Host ("  remove {0} ({1} GB)" -f $path, (Get-SizeGB $path))
    if (-not $WhatIf) { Remove-Item $path -Recurse -Force -ErrorAction SilentlyContinue }
}

# target/<profile>, target/<triple>/<profile> (a triple has three or four parts: x86_64-pc-windows-gnullvm;
# a lane such as T-gnu has fewer); with -Lanes also target/<lane>/...
function Get-ProfileDirs {
    Get-ChildItem $target -Directory -Force | ForEach-Object {
        if ($_.Name -in 'debug', 'release') { $_.FullName }
        elseif ($_.Name -match '^\w+-\w+-\w+' -or $Lanes) {
            Get-ChildItem $_.FullName -Directory -Force -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -in 'debug', 'release' } | ForEach-Object FullName
        }
    }
}

# keep only the newest build hash of each crate+extension in a deps/ folder
function Remove-OldHashes([string]$deps) {
    if (-not (Test-Path $deps)) { return }
    $rx = '^(?:lib)?(?<name>.+?)[-_](?<hash>[0-9a-f]{16})\.(?<ext>.+)$'
    $files = Get-ChildItem $deps -File -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -match $rx } | ForEach-Object {
            $null = $_.Name -match $rx
            [pscustomobject]@{ File = $_; Key = "$($matches.name).$($matches.ext)"; Hash = $matches.hash }
        }
    $freed = 0
    foreach ($group in ($files | Group-Object Key)) {
        $newest = ($group.Group | Sort-Object { $_.File.LastWriteTime } -Descending | Select-Object -First 1).Hash
        foreach ($old in ($group.Group | Where-Object { $_.Hash -ne $newest })) {
            $freed += $old.File.Length
            if (-not $WhatIf) { Remove-Item $old.File.FullName -Force -ErrorAction SilentlyContinue }
        }
    }
    Write-Host ("  {0}: superseded hashes {1:N2} GB" -f $deps, ($freed / 1GB))
}

$dirs = @(Get-ProfileDirs)
$before = Get-SizeGB $dirs
Write-Host "profile folders: $before GB (budget $BudgetGB GB)$(if (-not $Lanes) { '; lanes left alone' })"

Write-Host 'incremental caches'
foreach ($p in $dirs) { Remove-Tree (Join-Path $p 'incremental') }

Write-Host 'superseded build hashes'
foreach ($p in $dirs) { Remove-OldHashes (Join-Path $p 'deps') }

if (Get-Command cargo-sweep -ErrorAction SilentlyContinue) {
    Write-Host "cargo sweep --time $KeepDays"
    if (-not $WhatIf) {
        Push-Location $root
        try { cargo sweep --time $KeepDays } finally { Pop-Location }
    }
}

if ((Get-SizeGB $dirs) -gt $BudgetGB) {
    Write-Host 'over budget: removing target/debug (the cheapest to rebuild)'
    Remove-Tree (Join-Path $target 'debug')
}

$after = Get-SizeGB @(Get-ProfileDirs)
Write-Host ("profile folders: {0} GB -> {1} GB (freed {2} GB)" -f $before, $after, ($before - $after))
