#requires -Version 5.1
<#
.SYNOPSIS
Builds and tests SpaceTrace, then creates the Windows NSIS installer.
.DESCRIPTION
Uses installed Node.js, Rust, Visual Studio C++ tools, and Windows OpenSSH.
Initializes the x64 Visual Studio developer environment when needed. Does not
install system tools. npm ci restores JavaScript dependencies if they are missing.
.PARAMETER Jobs
Maximum concurrent Cargo build jobs. Defaults to 4; accepts 1 through 16.
#>
[CmdletBinding()]
param(
    [ValidateRange(1, 16)]
    [int]$Jobs = 4
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Invoke-CheckedNative {
    param(
        [Parameter(Mandatory = $true)] [string]$Executable,
        [string[]]$Arguments = @()
    )
    & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$([IO.Path]::GetFileName($Executable)) failed with exit code $LASTEXITCODE. Build stopped."
    }
}

function Find-RequiredCommand {
    param([string]$Name, [string]$Requirement)
    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $command) { throw "$Requirement is required. '$Name' was not found on PATH." }
    return $command.Source
}

function Initialize-CppEnvironment {
    $hasCompiler = $null -ne (Get-Command cl.exe -CommandType Application -ErrorAction SilentlyContinue)
    $hasLinker = $null -ne (Get-Command link.exe -CommandType Application -ErrorAction SilentlyContinue)
    if ($env:VSCMD_ARG_TGT_ARCH -eq 'x64' -and $hasCompiler -and $hasLinker -and $env:WindowsSdkDir) {
        Write-Host 'Using the current x64 Visual Studio developer environment.'
        return
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
        throw 'Visual Studio Installer/vswhere.exe was not found. Install Visual Studio Build Tools with Desktop development with C++ and a Windows SDK.'
    }
    $installations = @(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
    if ($LASTEXITCODE -ne 0 -or $installations.Count -eq 0 -or [string]::IsNullOrWhiteSpace($installations[0])) {
        throw 'No Visual Studio installation with x64 C++ build tools was found. Add Desktop development with C++ and a Windows SDK through Visual Studio Installer.'
    }
    $installation = $installations[0].Trim()
    $developerShell = Join-Path $installation 'Common7/Tools/Launch-VsDevShell.ps1'
    if (-not (Test-Path -LiteralPath $developerShell -PathType Leaf)) {
        throw "The Visual Studio developer-shell script is missing: $developerShell"
    }
    Write-Host "Initializing x64 C++ tools from $installation"
    & $developerShell -VsInstallationPath $installation -SkipAutomaticLocation -Arch amd64 -HostArch amd64
    $null = Find-RequiredCommand 'cl.exe' 'The Visual Studio C++ compiler'
    $null = Find-RequiredCommand 'link.exe' 'The Visual Studio linker'
    if (-not $env:WindowsSdkDir) { throw 'The Visual Studio environment does not include a Windows SDK. Add one through Visual Studio Installer.' }
}

if ($env:OS -ne 'Windows_NT') { throw 'Run this script on Windows using PowerShell.' }
$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$previousCargoJobs = $env:CARGO_BUILD_JOBS
Push-Location -LiteralPath $repositoryRoot
try {
    $node = Find-RequiredCommand 'node.exe' 'Node.js 22 or later'
    $npm = Find-RequiredCommand 'npm.cmd' 'npm'
    $cargo = Find-RequiredCommand 'cargo.exe' 'Rust stable with the x64 MSVC toolchain'
    $rustc = Find-RequiredCommand 'rustc.exe' 'Rust stable with the x64 MSVC toolchain'
    $nodeVersion = & $node --version
    if ($LASTEXITCODE -ne 0 -or $nodeVersion -notmatch '^v(\d+)\.' -or [int]$Matches[1] -lt 22) {
        throw 'Node.js 22 or later is required.'
    }
    $rustDetails = @(& $rustc -vV)
    if ($LASTEXITCODE -ne 0 -or -not ($rustDetails -contains 'host: x86_64-pc-windows-msvc')) {
        throw 'Use the x86_64-pc-windows-msvc Rust toolchain for this Windows x64 build.'
    }
    $keygen = Join-Path $env:SystemRoot 'System32/OpenSSH/ssh-keygen.exe'
    if (-not (Test-Path -LiteralPath $keygen -PathType Leaf)) {
        throw 'Windows OpenSSH Client is required by the SSH verification tests. Enable it in Windows Optional Features before building.'
    }

    Initialize-CppEnvironment
    $env:CARGO_BUILD_JOBS = $Jobs.ToString()
    Write-Host "Cargo concurrency: $env:CARGO_BUILD_JOBS jobs"

    $missingDependencies = @('node_modules/.bin/tauri.cmd', 'node_modules/.bin/tsc.cmd', 'node_modules/.bin/vite.cmd') |
        Where-Object { -not (Test-Path -LiteralPath $_ -PathType Leaf) }
    if ($missingDependencies) {
        Write-Host 'Restoring JavaScript dependencies from package-lock.json...'
        Invoke-CheckedNative -Executable $npm -Arguments @('ci', '--include=dev')
    }

    # A clean checkout needs the frontend assets before the native test build.
    Write-Host 'Building the interface...'
    Invoke-CheckedNative -Executable $npm -Arguments @('run', 'build')
    Write-Host 'Running Rust workspace tests...'
    Invoke-CheckedNative -Executable $cargo -Arguments @('test', '--workspace', '--locked')
    Write-Host 'Building the Windows application and installer...'
    Invoke-CheckedNative -Executable $npm -Arguments @('run', 'package')

    Write-Host ''
    Write-Host 'Build complete.'
    Write-Host "Application: $(Join-Path $repositoryRoot 'target/release/spacetrace.exe')"
    Write-Host "Installer:   $(Join-Path $repositoryRoot 'target/release/bundle/nsis')"
}
finally {
    $env:CARGO_BUILD_JOBS = $previousCargoJobs
    Pop-Location
}
