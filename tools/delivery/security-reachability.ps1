[CmdletBinding()]
param(
    [string] $Repository = ".",
    [string] $Profile = "qemu-prod",
    [string] $CargoEvidenceDirectory = "target/delivery/BASE-002-004",
    [Parameter(Mandatory = $true)]
    [string] $Output,
    [switch] $FaultInjectFinding
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Resolve-RepositoryPath {
    param([Parameter(Mandatory = $true)][string] $Path)
    if ([IO.Path]::IsPathRooted($Path)) { return [IO.Path]::GetFullPath($Path) }
    return [IO.Path]::GetFullPath((Join-Path $script:RepositoryRoot $Path))
}

function Read-Json {
    param([Parameter(Mandatory = $true)][string] $Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Required JSON file is missing: $Path" }
    return Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json -Depth 100
}

function To-RepositoryPath {
    param([Parameter(Mandatory = $true)][string] $Path)
    return [IO.Path]::GetRelativePath($script:RepositoryRoot, $Path).Replace("\", "/")
}

$resolvedRepository = Resolve-Path -LiteralPath $Repository -ErrorAction Stop
$script:RepositoryRoot = [IO.Path]::GetFullPath($resolvedRepository.Path)
$topLevel = & git -C $script:RepositoryRoot rev-parse --show-toplevel 2>$null
if ($LASTEXITCODE -ne 0) { throw "Repository is not a Git work tree: $script:RepositoryRoot" }
$script:RepositoryRoot = [IO.Path]::GetFullPath(([string] $topLevel).Trim())

$profilePath = Resolve-RepositoryPath -Path "product/profiles/$Profile.json"
$evidencePath = Resolve-RepositoryPath -Path $CargoEvidenceDirectory
$graph = Read-Json -Path (Join-Path $evidencePath "cargo-graph.json")
$boms = Read-Json -Path (Join-Path $evidencePath "binary-boms.json")
$profileValue = Read-Json -Path $profilePath
if ($profileValue.id -ne $Profile) { throw "Profile id mismatch: $($profileValue.id)" }

$closureIds = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
foreach ($binary in @($profileValue.binaries)) {
    $matches = @($boms.binaries | Where-Object { $_.selector -eq [string] $binary.selector })
    if ($matches.Count -ne 1) { throw "Profile binary is absent or ambiguous in binary BOM: $($binary.selector)" }
    foreach ($package in @($matches[0].packages)) { [void] $closureIds.Add([string] $package.id) }
}

$workspacePackages = @($graph.packages | Where-Object { [bool] $_.workspace_member } | Sort-Object manifest_path)
$findings = [Collections.Generic.List[object]]::new()
$patterns = @(
    [ordered]@{ id = "fixed-rng-constructor"; regex = "CsRng::(new|default)\s*\(" },
    [ordered]@{ id = "fixed-seed-literal"; regex = "0x31,\s*0x32,\s*0x33,\s*0x34" },
    [ordered]@{ id = "custom-mtls-symbol"; regex = "(eneros[_-]mtls|MockMtlsTransport|MtlsContext)" }
)

foreach ($package in $workspacePackages) {
    $manifestAbsolute = Resolve-RepositoryPath -Path ([string] $package.manifest_path)
    $packageDirectory = Split-Path -Parent $manifestAbsolute
    foreach ($sourceFile in @(Get-ChildItem -LiteralPath $packageDirectory -Recurse -File -Include "*.rs", "Cargo.toml" | Sort-Object FullName)) {
        $lines = Get-Content -LiteralPath $sourceFile.FullName -Encoding UTF8
        for ($index = 0; $index -lt $lines.Count; $index += 1) {
            foreach ($pattern in $patterns) {
                if ($lines[$index] -match $pattern.regex) {
                    $findings.Add([ordered]@{
                        kind = $pattern.id
                        package_id = [string] $package.id
                        package_name = [string] $package.name
                        path = To-RepositoryPath -Path $sourceFile.FullName
                        line = $index + 1
                        product_reachable = [bool] $closureIds.Contains([string] $package.id)
                    })
                }
            }
        }
    }
}

if ($FaultInjectFinding) {
    $rootId = @($closureIds | Sort-Object)[0]
    $root = @($graph.packages | Where-Object { $_.id -eq $rootId })[0]
    $findings.Add([ordered]@{
        kind = "fault-injected-fixed-rng"
        package_id = $rootId
        package_name = [string] $root.name
        path = "fault-injection/fixed-rng.rs"
        line = 1
        product_reachable = $true
    })
}

$productFindings = @($findings | Where-Object { $_.product_reachable })
$cryptoSource = Get-Content -LiteralPath (Resolve-RepositoryPath -Path "crates/security/crypto/src/rng/csrng.rs") -Raw -Encoding UTF8
$mtlsSource = Get-Content -LiteralPath (Resolve-RepositoryPath -Path "crates/security/mtls/src/lib.rs") -Raw -Encoding UTF8
$fixedConstructorReleaseGuard = $cryptoSource -match '#\[cfg\(any\(test, debug_assertions, feature = "insecure-test-rng"\)\)\]\s*pub fn new'
$fixedDefaultReleaseGuard = $cryptoSource -match '#\[cfg\(any\(test, debug_assertions, feature = "insecure-test-rng"\)\)\]\s*impl Default for CsRng'
$mtlsReleaseGuard = $mtlsSource -match 'not\(debug_assertions\)' -and $mtlsSource -match 'not\(feature = "insecure-mtls-prototype"\)' -and $mtlsSource -match 'compile_error!'

$violations = [Collections.Generic.List[object]]::new()
foreach ($finding in $productFindings) {
    $violations.Add([ordered]@{ code = "security-finding-product-reachable"; detail = "$($finding.kind):$($finding.path):$($finding.line)" })
}
if (-not $fixedConstructorReleaseGuard) { $violations.Add([ordered]@{ code = "fixed-constructor-release-guard-missing"; detail = "CsRng::new" }) }
if (-not $fixedDefaultReleaseGuard) { $violations.Add([ordered]@{ code = "fixed-default-release-guard-missing"; detail = "CsRng::default" }) }
if (-not $mtlsReleaseGuard) { $violations.Add([ordered]@{ code = "mtls-release-guard-missing"; detail = "eneros-mtls" }) }
if ([bool] $profileValue.security.allow_mock) { $violations.Add([ordered]@{ code = "profile-allows-mock"; detail = $Profile }) }
if ([bool] $profileValue.security.allow_unsigned) { $violations.Add([ordered]@{ code = "profile-allows-unsigned"; detail = $Profile }) }
if ([bool] $profileValue.signing.allow_test_keys) { $violations.Add([ordered]@{ code = "profile-allows-test-key"; detail = $Profile }) }

$head = (& git -C $script:RepositoryRoot rev-parse HEAD 2>$null).Trim()
$report = [ordered]@{
    schema = "eneros.delivery.security-reachability.v1"
    source_commit = $head
    profile = $Profile
    fault_injection = [bool] $FaultInjectFinding
    product_closure_package_count = $closureIds.Count
    repository_finding_count = $findings.Count
    product_reachable_finding_count = $productFindings.Count
    guards = [ordered]@{
        fixed_constructor_absent_from_release_api = [bool] $fixedConstructorReleaseGuard
        fixed_default_absent_from_release_api = [bool] $fixedDefaultReleaseGuard
        custom_mtls_default_release_rejected = [bool] $mtlsReleaseGuard
    }
    result = if ($violations.Count -eq 0) { "PASS" } else { "FAIL" }
    violation_count = $violations.Count
    violations = @($violations)
    findings = @($findings)
}

$outputPath = if ([IO.Path]::IsPathRooted($Output)) { [IO.Path]::GetFullPath($Output) } else { [IO.Path]::GetFullPath((Join-Path (Get-Location) $Output)) }
$outputDirectory = Split-Path -Parent $outputPath
if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) { New-Item -ItemType Directory -Path $outputDirectory | Out-Null }
$json = ($report | ConvertTo-Json -Depth 100) + "`n"
[IO.File]::WriteAllText($outputPath, $json, [Text.UTF8Encoding]::new($false))
$json
if ($violations.Count -ne 0) { exit 2 }
