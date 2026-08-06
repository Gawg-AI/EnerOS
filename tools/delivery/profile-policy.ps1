[CmdletBinding()]
param(
    [string] $Repository = ".",
    [string] $ProfileDirectory = "product/profiles",
    [string] $Policy = "product/policy/forbidden.json",
    [string] $CargoEvidenceDirectory = "target/delivery/BASE-002-004",
    [Parameter(Mandatory = $true)]
    [string] $Output,
    [string[]] $Profile = @(),
    [string] $FaultInjectPackage = "",
    [string] $FaultInjectFeature = "",
    [switch] $FaultUnknownField,
    [switch] $FaultReleaseMock,
    [switch] $FaultTestKey,
    [switch] $FaultMissingBinary
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

function Read-StrictJson {
    param([Parameter(Mandatory = $true)][string] $Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Required JSON file is missing: $Path" }
    $text = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
    try {
        return $text | ConvertFrom-Json -Depth 100
    } catch {
        throw "Invalid JSON in $Path`: $($_.Exception.Message)"
    }
}

function Add-Violation {
    param(
        [Parameter(Mandatory = $true)][string] $ProfileId,
        [Parameter(Mandatory = $true)][string] $Code,
        [Parameter(Mandatory = $true)][string] $Detail
    )
    $script:Violations.Add([ordered]@{ profile = $ProfileId; code = $Code; detail = $Detail })
}

function Assert-ObjectShape {
    param(
        [Parameter(Mandatory = $true)] $Object,
        [Parameter(Mandatory = $true)][string[]] $Allowed,
        [Parameter(Mandatory = $true)][string[]] $Required,
        [Parameter(Mandatory = $true)][string] $ProfileId,
        [Parameter(Mandatory = $true)][string] $Path
    )
    $actual = @($Object.PSObject.Properties.Name)
    foreach ($name in $actual) {
        if ($Allowed -notcontains $name) { Add-Violation -ProfileId $ProfileId -Code "unknown-field" -Detail "$Path.$name" }
    }
    foreach ($name in $Required) {
        if ($actual -notcontains $name) { Add-Violation -ProfileId $ProfileId -Code "missing-field" -Detail "$Path.$name" }
    }
}

function Test-GlobMatch {
    param([string] $Value, [string[]] $Patterns)
    foreach ($pattern in $Patterns) {
        if ($Value -like $pattern) { return $true }
    }
    return $false
}

function Validate-Profile {
    param(
        [Parameter(Mandatory = $true)] $Value,
        [Parameter(Mandatory = $true)][string] $ExpectedId
    )
    $profileId = if ($Value.PSObject.Properties.Name -contains "id") { [string] $Value.id } else { $ExpectedId }
    $topFields = @("schema", "id", "class", "platform", "target", "image_layout", "artifacts", "binaries", "signing", "security")
    Assert-ObjectShape -Object $Value -Allowed $topFields -Required $topFields -ProfileId $profileId -Path "profile"
    if ($Value.schema -ne "eneros.product-profile.v1") { Add-Violation $profileId "schema" ([string] $Value.schema) }
    if ($profileId -ne $ExpectedId) { Add-Violation $profileId "profile-id-filename-mismatch" $ExpectedId }
    if ($profileId -notmatch "^[a-z][a-z0-9-]{1,31}$") { Add-Violation $profileId "profile-id" $profileId }
    if (@("test", "development", "release", "recovery", "factory") -notcontains [string] $Value.class) {
        Add-Violation $profileId "profile-class" ([string] $Value.class)
    }
    if ([string]::IsNullOrWhiteSpace([string] $Value.platform)) { Add-Violation $profileId "platform" "empty" }
    if ([string]::IsNullOrWhiteSpace([string] $Value.target)) { Add-Violation $profileId "target" "empty" }
    if (@($Value.artifacts).Count -eq 0) { Add-Violation $profileId "artifacts" "empty" }
    $allowedArtifacts = @("test-binaries", "raw-gpt", "installer-iso", "recovery-image", "firmware-bundle", "provisioning-bundle")
    foreach ($artifact in @($Value.artifacts)) {
        if ($allowedArtifacts -notcontains [string] $artifact) { Add-Violation $profileId "artifact" ([string] $artifact) }
    }
    if (@($Value.artifacts | Sort-Object -Unique).Count -ne @($Value.artifacts).Count) {
        Add-Violation $profileId "duplicate-artifact" "artifacts"
    }

    if (@($Value.binaries).Count -eq 0) { Add-Violation $profileId "binaries" "empty" }
    $binaryFields = @("selector", "role", "target", "default_features", "features")
    $selectors = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    foreach ($binary in @($Value.binaries)) {
        Assert-ObjectShape -Object $binary -Allowed $binaryFields -Required $binaryFields -ProfileId $profileId -Path "profile.binaries[]"
        $selector = [string] $binary.selector
        if ($selector -notmatch "^[A-Za-z0-9_-]+::[A-Za-z0-9_-]+$") { Add-Violation $profileId "binary-selector" $selector }
        if (-not $selectors.Add($selector)) { Add-Violation $profileId "duplicate-binary" $selector }
        if (@("host-gate", "boot-manager", "kernel", "recovery", "factory") -notcontains [string] $binary.role) {
            Add-Violation $profileId "binary-role" ([string] $binary.role)
        }
        if ($binary.default_features -isnot [bool]) { Add-Violation $profileId "default-features-type" $selector }
        if (@($binary.features | Sort-Object -Unique).Count -ne @($binary.features).Count) {
            Add-Violation $profileId "duplicate-feature" $selector
        }
    }

    $signingFields = @("required", "key_source", "allow_test_keys")
    Assert-ObjectShape -Object $Value.signing -Allowed $signingFields -Required $signingFields -ProfileId $profileId -Path "profile.signing"
    if ($Value.signing.required -isnot [bool] -or $Value.signing.allow_test_keys -isnot [bool]) {
        Add-Violation $profileId "signing-boolean-type" "required/allow_test_keys"
    }
    if (@("none", "external-file", "external-provider") -notcontains [string] $Value.signing.key_source) {
        Add-Violation $profileId "key-source" ([string] $Value.signing.key_source)
    }

    $securityFields = @("allow_mock", "allow_unsigned", "minimum_security_version", "entropy_sources")
    Assert-ObjectShape -Object $Value.security -Allowed $securityFields -Required $securityFields -ProfileId $profileId -Path "profile.security"
    if ($Value.security.allow_mock -isnot [bool] -or $Value.security.allow_unsigned -isnot [bool]) {
        Add-Violation $profileId "security-boolean-type" "allow_mock/allow_unsigned"
    }
    $securityVersion = [long] $Value.security.minimum_security_version
    if ($securityVersion -lt 0 -or $securityVersion -gt [uint32]::MaxValue) { Add-Violation $profileId "security-version" ([string] $securityVersion) }
    foreach ($source in @($Value.security.entropy_sources)) {
        if (@("host-os", "virtio-rng", "hardware-trng") -notcontains [string] $source) {
            Add-Violation $profileId "entropy-source" ([string] $source)
        }
    }

    if ($script:ReleaseClasses -contains [string] $Value.class) {
        if (-not [bool] $Value.signing.required) { Add-Violation $profileId "release-signing-required" "false" }
        if ([string] $Value.signing.key_source -eq "none") { Add-Violation $profileId "release-key-source" "none" }
        if ([bool] $Value.signing.allow_test_keys) { Add-Violation $profileId "release-test-key" "true" }
        if ([bool] $Value.security.allow_mock) { Add-Violation $profileId "release-mock" "true" }
        if ([bool] $Value.security.allow_unsigned) { Add-Violation $profileId "release-unsigned" "true" }
        if (@($Value.security.entropy_sources).Count -eq 0) { Add-Violation $profileId "release-entropy" "empty" }
        if ($securityVersion -lt 1) { Add-Violation $profileId "release-security-version" ([string] $securityVersion) }
    }
}

$resolvedRepository = Resolve-Path -LiteralPath $Repository -ErrorAction Stop
$script:RepositoryRoot = [IO.Path]::GetFullPath($resolvedRepository.Path)
$topLevel = & git -C $script:RepositoryRoot rev-parse --show-toplevel 2>$null
if ($LASTEXITCODE -ne 0) { throw "Repository is not a Git work tree: $script:RepositoryRoot" }
$script:RepositoryRoot = [IO.Path]::GetFullPath(([string] $topLevel).Trim())

$profileDirectoryPath = Resolve-RepositoryPath -Path $ProfileDirectory
$policyPath = Resolve-RepositoryPath -Path $Policy
$schemaPath = Join-Path $profileDirectoryPath "schema.json"
$evidenceDirectoryPath = Resolve-RepositoryPath -Path $CargoEvidenceDirectory
$graphPath = Join-Path $evidenceDirectoryPath "cargo-graph.json"
$bomsPath = Join-Path $evidenceDirectoryPath "binary-boms.json"
$policyValue = Read-StrictJson -Path $policyPath
$graph = Read-StrictJson -Path $graphPath
$boms = Read-StrictJson -Path $bomsPath

$policyFields = @("schema", "release_classes", "packages", "feature_patterns", "path_fragments")
$script:Violations = [Collections.Generic.List[object]]::new()
Assert-ObjectShape -Object $policyValue -Allowed $policyFields -Required $policyFields -ProfileId "policy" -Path "policy"
if ($policyValue.schema -ne "eneros.forbidden-policy.v1") { Add-Violation "policy" "schema" ([string] $policyValue.schema) }
$script:ReleaseClasses = @($policyValue.release_classes | ForEach-Object { [string] $_ })

$profileFiles = if ($Profile.Count -eq 0) {
    @(Get-ChildItem -LiteralPath $profileDirectoryPath -Filter "*.json" -File | Where-Object { $_.Name -ne "schema.json" } | Sort-Object Name)
} else {
    @($Profile | Sort-Object -Unique | ForEach-Object {
        $candidate = Join-Path $profileDirectoryPath "$_.json"
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { throw "Unknown profile: $_" }
        Get-Item -LiteralPath $candidate
    })
}
$profileFiles = @($profileFiles)
if ($profileFiles.Count -eq 0) { throw "No profiles selected" }

$profileReports = [Collections.Generic.List[object]]::new()
foreach ($file in $profileFiles) {
    $value = Read-StrictJson -Path $file.FullName
    $expectedId = [IO.Path]::GetFileNameWithoutExtension($file.Name)
    if ($FaultUnknownField) { $value | Add-Member -NotePropertyName "unexpected" -NotePropertyValue $true -Force }
    if ($FaultReleaseMock) { $value.security.allow_mock = $true }
    if ($FaultTestKey) {
        $value.signing.allow_test_keys = $true
        $value.signing.key_source = "none"
    }
    if ($FaultMissingBinary) {
        $value.binaries = @($value.binaries) + [pscustomobject]@{
            selector = "missing-package::missing-binary"
            role = "kernel"
            default_features = $false
            features = @()
        }
    }

    $violationStart = $script:Violations.Count
    Validate-Profile -Value $value -ExpectedId $expectedId
    $closureById = @{}
    foreach ($binary in @($value.binaries)) {
        $selector = [string] $binary.selector
        $matches = @($boms.binaries | Where-Object { $_.selector -eq $selector })
        if ($matches.Count -ne 1) {
            Add-Violation $expectedId "binary-not-in-cargo-bom" $selector
            continue
        }
        $rootPackageId = [string] $matches[0].package_id
        $rootGraphPackage = @($graph.packages | Where-Object { $_.id -eq $rootPackageId })
        if ($rootGraphPackage.Count -ne 1) {
            Add-Violation $expectedId "binary-root-not-in-graph" $selector
            continue
        }
        $declaredFeatureNames = @($rootGraphPackage[0].declared_features | Where-Object { $null -ne $_ -and $_.PSObject.Properties.Name -contains "name" } | ForEach-Object { [string] $_.name })
        foreach ($requestedFeature in @($binary.features)) {
            if ($declaredFeatureNames -notcontains [string] $requestedFeature) {
                Add-Violation $expectedId "undeclared-requested-feature" "$selector`:$requestedFeature"
            }
        }
        foreach ($package in @($matches[0].packages)) { $closureById[[string] $package.id] = $package }
    }

    if (-not [string]::IsNullOrEmpty($FaultInjectPackage)) {
        $candidates = @($graph.packages | Where-Object { $_.name -eq $FaultInjectPackage } | Sort-Object @{ Expression = "workspace_member"; Descending = $true }, @{ Expression = "id"; Descending = $false })
        if ($candidates.Count -eq 0) { throw "Fault injection package is not in Cargo graph: $FaultInjectPackage" }
        $candidate = $candidates[0]
        $closureById[[string] $candidate.id] = [pscustomobject]@{
            id = [string] $candidate.id
            name = [string] $candidate.name
            version = [string] $candidate.version
            license = $candidate.license
            source = $candidate.source
            manifest_path = [string] $candidate.manifest_path
            workspace_member = [bool] $candidate.workspace_member
            resolved_features = @($candidate.resolved_features)
        }
    }

    $isRelease = $script:ReleaseClasses -contains [string] $value.class
    if ($isRelease) {
        foreach ($package in @($closureById.Values | Sort-Object id)) {
            if (@($policyValue.packages) -contains [string] $package.name) {
                Add-Violation $expectedId "forbidden-package" ([string] $package.name)
            }
            $manifestPath = ([string] $package.manifest_path).Replace("\", "/")
            foreach ($fragment in @($policyValue.path_fragments)) {
                if ($manifestPath.Contains([string] $fragment, [StringComparison]::OrdinalIgnoreCase)) {
                    Add-Violation $expectedId "forbidden-path" "$($package.name):$fragment"
                }
            }
            foreach ($feature in @($package.resolved_features)) {
                if (Test-GlobMatch -Value ([string] $feature) -Patterns @($policyValue.feature_patterns)) {
                    Add-Violation $expectedId "forbidden-resolved-feature" "$($package.name):$feature"
                }
            }
        }
        foreach ($binary in @($value.binaries)) {
            foreach ($feature in @($binary.features)) {
                if (Test-GlobMatch -Value ([string] $feature) -Patterns @($policyValue.feature_patterns)) {
                    Add-Violation $expectedId "forbidden-requested-feature" "$($binary.selector):$feature"
                }
            }
        }
        if (-not [string]::IsNullOrEmpty($FaultInjectFeature)) {
            if (Test-GlobMatch -Value $FaultInjectFeature -Patterns @($policyValue.feature_patterns)) {
                Add-Violation $expectedId "forbidden-fault-injected-feature" $FaultInjectFeature
            } else {
                throw "Fault injected feature does not match forbidden policy: $FaultInjectFeature"
            }
        }
    }

    $profileViolationCount = $script:Violations.Count - $violationStart
    $profileReports.Add([ordered]@{
        id = $expectedId
        class = [string] $value.class
        profile_sha256 = Get-Sha256 -Path $file.FullName
        binary_count = @($value.binaries).Count
        closure_package_count = $closureById.Count
        result = if ($profileViolationCount -eq 0) { "PASS" } else { "FAIL" }
        violation_count = $profileViolationCount
    })
}

$head = (& git -C $script:RepositoryRoot rev-parse HEAD 2>$null).Trim()
$report = [ordered]@{
    schema = "eneros.delivery.profile-policy-report.v1"
    source_commit = $head
    profile_schema_sha256 = Get-Sha256 -Path $schemaPath
    forbidden_policy_sha256 = Get-Sha256 -Path $policyPath
    cargo_graph_sha256 = Get-Sha256 -Path $graphPath
    binary_boms_sha256 = Get-Sha256 -Path $bomsPath
    fault_injection = [ordered]@{
        package = $FaultInjectPackage
        feature = $FaultInjectFeature
        unknown_field = [bool] $FaultUnknownField
        release_mock = [bool] $FaultReleaseMock
        test_key = [bool] $FaultTestKey
        missing_binary = [bool] $FaultMissingBinary
    }
    result = if ($script:Violations.Count -eq 0) { "PASS" } else { "FAIL" }
    profile_count = $profileReports.Count
    profiles = @($profileReports)
    violation_count = $script:Violations.Count
    violations = @($script:Violations)
}

$outputPath = if ([IO.Path]::IsPathRooted($Output)) { [IO.Path]::GetFullPath($Output) } else { [IO.Path]::GetFullPath((Join-Path (Get-Location) $Output)) }
$outputDirectory = Split-Path -Parent $outputPath
if (-not (Test-Path -LiteralPath $outputDirectory -PathType Container)) { New-Item -ItemType Directory -Path $outputDirectory | Out-Null }
$json = ($report | ConvertTo-Json -Depth 100) + "`n"
[IO.File]::WriteAllText($outputPath, $json, [Text.UTF8Encoding]::new($false))
$json
if ($script:Violations.Count -ne 0) { exit 2 }
