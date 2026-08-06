[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, ParameterSetName = "Create")]
    [string] $Output,

    [Parameter(Mandatory = $true, ParameterSetName = "Verify")]
    [string] $Verify,

    [Parameter(ParameterSetName = "Create")]
    [Parameter(ParameterSetName = "Verify")]
    [string] $Repository = "."
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Invoke-GitLines {
    param([string[]] $Arguments)

    $commandOutput = & git -C $script:RepositoryRoot -c core.quotepath=false @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed: $($commandOutput -join [Environment]::NewLine)"
    }
    return @($commandOutput | ForEach-Object { [string] $_ })
}

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string] $Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-RelativePath {
    param([Parameter(Mandatory = $true)][string] $Path)
    return [IO.Path]::GetRelativePath($script:RepositoryRoot, $Path).Replace("\", "/")
}

function Get-FileRecord {
    param([Parameter(Mandatory = $true)][string] $RelativePath)

    $absolutePath = Join-Path $script:RepositoryRoot $RelativePath
    if (-not (Test-Path -LiteralPath $absolutePath -PathType Leaf)) {
        throw "Expected file is missing: $RelativePath"
    }
    $file = Get-Item -LiteralPath $absolutePath
    return [ordered]@{
        path = $RelativePath.Replace("\", "/")
        bytes = [long] $file.Length
        sha256 = Get-Sha256 -Path $absolutePath
    }
}

$resolvedRepository = Resolve-Path -LiteralPath $Repository -ErrorAction Stop
$script:RepositoryRoot = [IO.Path]::GetFullPath($resolvedRepository.Path)
$insideWorkTree = (& git -C $script:RepositoryRoot rev-parse --is-inside-work-tree 2>$null)
if ($LASTEXITCODE -ne 0 -or $insideWorkTree -ne "true") {
    throw "Repository is not a Git work tree: $script:RepositoryRoot"
}
$topLevel = (& git -C $script:RepositoryRoot rev-parse --show-toplevel 2>$null)
if ($LASTEXITCODE -ne 0) {
    throw "Cannot resolve Git top level: $script:RepositoryRoot"
}
$script:RepositoryRoot = [IO.Path]::GetFullPath($topLevel.Trim())

if ($PSCmdlet.ParameterSetName -eq "Verify") {
    $snapshotPath = [IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Verify).Path)
    $snapshot = Get-Content -LiteralPath $snapshotPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $mismatches = [Collections.Generic.List[object]]::new()
    foreach ($record in $snapshot.untracked_files) {
        $absolutePath = Join-Path $script:RepositoryRoot ([string] $record.path)
        if (-not (Test-Path -LiteralPath $absolutePath -PathType Leaf)) {
            $mismatches.Add([ordered]@{ path = $record.path; reason = "missing" })
            continue
        }
        $actualSize = (Get-Item -LiteralPath $absolutePath).Length
        $actualHash = Get-Sha256 -Path $absolutePath
        if ($actualSize -ne [long] $record.bytes -or $actualHash -ne [string] $record.sha256) {
            $mismatches.Add([ordered]@{
                path = $record.path
                reason = "content-mismatch"
                expected_bytes = [long] $record.bytes
                actual_bytes = [long] $actualSize
                expected_sha256 = [string] $record.sha256
                actual_sha256 = $actualHash
            })
        }
    }

    $head = @(Invoke-GitLines -Arguments @("rev-parse", "HEAD"))[0].Trim()
    $statusLines = @(Invoke-GitLines -Arguments @("status", "--porcelain=v1", "--untracked-files=all"))
    $statusText = ($statusLines -join "`n") + "`n"
    $statusBytes = [Text.Encoding]::UTF8.GetBytes($statusText)
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        $statusHash = [Convert]::ToHexString($sha.ComputeHash($statusBytes)).ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
    if ($head -ne [string] $snapshot.source_commit) {
        $mismatches.Add([ordered]@{ reason = "head-mismatch"; expected = $snapshot.source_commit; actual = $head })
    }
    if ($statusHash -ne [string] $snapshot.git_status_sha256) {
        $mismatches.Add([ordered]@{ reason = "status-mismatch"; expected = $snapshot.git_status_sha256; actual = $statusHash })
    }

    $verification = [ordered]@{
        schema = "eneros.delivery.baseline-verification.v1"
        snapshot = Get-RelativePath -Path $snapshotPath
        result = if ($mismatches.Count -eq 0) { "PASS" } else { "FAIL" }
        mismatch_count = $mismatches.Count
        mismatches = @($mismatches)
    }
    $verification | ConvertTo-Json -Depth 8
    if ($mismatches.Count -ne 0) { exit 2 }
    exit 0
}

$outputPath = [IO.Path]::GetFullPath((Join-Path (Get-Location) $Output))
$outputDirectory = Split-Path -Parent $outputPath
if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) {
    New-Item -ItemType Directory -Path $outputDirectory | Out-Null
}

$head = @(Invoke-GitLines -Arguments @("rev-parse", "HEAD"))[0].Trim()
$branch = @(Invoke-GitLines -Arguments @("branch", "--show-current"))[0].Trim()
$statusLines = @(Invoke-GitLines -Arguments @("status", "--porcelain=v1", "--untracked-files=all"))
$statusText = ($statusLines -join "`n") + "`n"
$statusBytes = [Text.Encoding]::UTF8.GetBytes($statusText)
$sha = [Security.Cryptography.SHA256]::Create()
try {
    $statusHash = [Convert]::ToHexString($sha.ComputeHash($statusBytes)).ToLowerInvariant()
} finally {
    $sha.Dispose()
}

$untrackedPaths = Invoke-GitLines -Arguments @("ls-files", "--others", "--exclude-standard") |
    Where-Object { $_ -ne "" } |
    Sort-Object -Unique
$untrackedFiles = @($untrackedPaths | ForEach-Object { Get-FileRecord -RelativePath $_ })
$trackedRecords = @($statusLines | Where-Object { $_.Length -ge 3 -and -not $_.StartsWith("?? ") })

$snapshot = [ordered]@{
    schema = "eneros.delivery.baseline-snapshot.v1"
    source_commit = $head
    branch = $branch
    tree_state = if ($statusLines.Count -eq 0) { "clean" } else { "dirty" }
    git_status_sha256 = $statusHash
    tracked_change_count = $trackedRecords.Count
    untracked_file_count = $untrackedFiles.Count
    tracked_status = $trackedRecords
    untracked_files = $untrackedFiles
}

$json = $snapshot | ConvertTo-Json -Depth 8
[IO.File]::WriteAllText($outputPath, $json + "`n", [Text.UTF8Encoding]::new($false))
$digestPath = "$outputPath.sha256"
$snapshotHash = Get-Sha256 -Path $outputPath
[IO.File]::WriteAllText($digestPath, "$snapshotHash  $([IO.Path]::GetFileName($outputPath))`n", [Text.UTF8Encoding]::new($false))
Write-Output $json
