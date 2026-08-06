[CmdletBinding()]
param(
    [string] $Repository = ".",
    [string] $CargoGraph = "target/delivery/BASE-002-004/cargo-graph.json",
    [Parameter(Mandatory = $true)]
    [string] $Output,
    [switch] $FaultMissingMit,
    [switch] $FaultUnsupportedSpdx
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-RepositoryPath {
    param([Parameter(Mandatory = $true)][string] $Path)
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $script:RepositoryRoot $Path))
}

function Get-Sha256 {
    param([Parameter(Mandatory = $true)][string] $Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

$resolvedRepository = Resolve-Path -LiteralPath $Repository -ErrorAction Stop
$script:RepositoryRoot = [IO.Path]::GetFullPath($resolvedRepository.Path)
$topLevel = & git -C $script:RepositoryRoot rev-parse --show-toplevel 2>$null
if ($LASTEXITCODE -ne 0) { throw "Repository is not a Git work tree: $script:RepositoryRoot" }
$script:RepositoryRoot = [IO.Path]::GetFullPath(([string] $topLevel).Trim())

$graphPath = Resolve-RepositoryPath -Path $CargoGraph
if (-not (Test-Path -LiteralPath $graphPath -PathType Leaf)) { throw "Cargo graph is missing: $graphPath" }
$graph = Get-Content -LiteralPath $graphPath -Raw -Encoding UTF8 | ConvertFrom-Json -Depth 100
$apachePath = Resolve-RepositoryPath -Path "LICENSE"
$mitPath = Resolve-RepositoryPath -Path "LICENSE-MIT"
$violations = [Collections.Generic.List[object]]::new()

$apachePresent = Test-Path -LiteralPath $apachePath -PathType Leaf
$mitPresent = (Test-Path -LiteralPath $mitPath -PathType Leaf) -and -not $FaultMissingMit
if (-not $apachePresent) { $violations.Add([ordered]@{ code = "missing-license-text"; detail = "Apache-2.0" }) }
if (-not $mitPresent) { $violations.Add([ordered]@{ code = "missing-license-text"; detail = "MIT" }) }
if ($apachePresent) {
    $apacheText = Get-Content -LiteralPath $apachePath -Raw -Encoding UTF8
    if ($apacheText -notmatch "Apache License\s+Version 2\.0") { $violations.Add([ordered]@{ code = "invalid-license-text"; detail = "Apache-2.0" }) }
}
if ($mitPresent) {
    $mitText = Get-Content -LiteralPath $mitPath -Raw -Encoding UTF8
    if ($mitText -notmatch "Permission is hereby granted, free of charge") { $violations.Add([ordered]@{ code = "invalid-license-text"; detail = "MIT" }) }
}

$allowedExpressions = @("MIT", "Apache-2.0", "MIT OR Apache-2.0")
$workspacePackages = @($graph.packages | Where-Object { [bool] $_.workspace_member } | Sort-Object name, id)
$licenseCounts = [ordered]@{}
foreach ($package in $workspacePackages) {
    $expression = if ($FaultUnsupportedSpdx -and $package -eq $workspacePackages[0]) { "LicenseRef-Fault" } else { [string] $package.license }
    if ([string]::IsNullOrWhiteSpace($expression)) {
        $violations.Add([ordered]@{ code = "missing-package-license"; detail = [string] $package.name })
        continue
    }
    if ($allowedExpressions -notcontains $expression) {
        $violations.Add([ordered]@{ code = "unsupported-workspace-spdx"; detail = "$($package.name):$expression" })
    }
    if (-not $licenseCounts.Contains($expression)) { $licenseCounts[$expression] = 0 }
    $licenseCounts[$expression] = [int] $licenseCounts[$expression] + 1
    if ($expression -match "MIT" -and -not $mitPresent) {
        $violations.Add([ordered]@{ code = "package-license-text-unavailable"; detail = "$($package.name):MIT" })
    }
    if ($expression -match "Apache-2\.0" -and -not $apachePresent) {
        $violations.Add([ordered]@{ code = "package-license-text-unavailable"; detail = "$($package.name):Apache-2.0" })
    }
}

$workspaceManifest = Get-Content -LiteralPath (Resolve-RepositoryPath -Path "Cargo.toml") -Raw -Encoding UTF8
if ($workspaceManifest -notmatch 'license\s*=\s*"MIT OR Apache-2\.0"') {
    $violations.Add([ordered]@{ code = "workspace-license-policy-drift"; detail = "Cargo.toml" })
}

$head = (& git -C $script:RepositoryRoot rev-parse HEAD 2>$null).Trim()
$report = [ordered]@{
    schema = "eneros.delivery.license-consistency.v1"
    source_commit = $head
    fault_injection = [ordered]@{
        missing_mit = [bool] $FaultMissingMit
        unsupported_spdx = [bool] $FaultUnsupportedSpdx
    }
    workspace_package_count = $workspacePackages.Count
    license_counts = $licenseCounts
    license_texts = [ordered]@{
        mit = if ($mitPresent) { [ordered]@{ path = "LICENSE-MIT"; sha256 = Get-Sha256 -Path $mitPath } } else { $null }
        apache_2_0 = if ($apachePresent) { [ordered]@{ path = "LICENSE"; sha256 = Get-Sha256 -Path $apachePath } } else { $null }
    }
    result = if ($violations.Count -eq 0) { "PASS" } else { "FAIL" }
    violation_count = $violations.Count
    violations = @($violations)
}

$outputPath = if ([IO.Path]::IsPathRooted($Output)) { [IO.Path]::GetFullPath($Output) } else { [IO.Path]::GetFullPath((Join-Path (Get-Location) $Output)) }
$outputDirectory = Split-Path -Parent $outputPath
if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) { New-Item -ItemType Directory -Path $outputDirectory | Out-Null }
$json = ($report | ConvertTo-Json -Depth 50) + "`n"
[IO.File]::WriteAllText($outputPath, $json, [Text.UTF8Encoding]::new($false))
$json
if ($violations.Count -ne 0) { exit 2 }
