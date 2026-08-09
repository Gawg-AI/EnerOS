[CmdletBinding(DefaultParameterSetName = "Create")]
param(
    [Parameter(Mandatory = $true, ParameterSetName = "Create")]
    [string] $OutputDirectory,

    [Parameter(Mandatory = $true, ParameterSetName = "Verify")]
    [string] $VerifyDirectory,

    [Parameter(ParameterSetName = "Create")]
    [Parameter(ParameterSetName = "Verify")]
    [string] $Repository = ".",

    [Parameter(ParameterSetName = "Create")]
    [Parameter(ParameterSetName = "Verify")]
    [string[]] $Binary = @(),

    # 外部（非 cargo workspace）二进制选择器，如 seL4 elfloader（C 构建产物）。
    # 每条生成一个闭包为空的 BOM 条目：外部产物无 cargo 依赖闭包，其
    # 完整性由 profile audit 的 selectors 覆盖性检查另行保证。
    [Parameter(ParameterSetName = "Create")]
    [Parameter(ParameterSetName = "Verify")]
    [string[]] $ExternalBinary = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-TextSha256 {
    param([Parameter(Mandatory = $true)][string] $Text)
    $bytes = [Text.Encoding]::UTF8.GetBytes($Text)
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return [Convert]::ToHexString($sha.ComputeHash($bytes)).ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Get-FileSha256 {
    param([Parameter(Mandatory = $true)][string] $Path)
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Convert-RepositoryPath {
    param([AllowNull()][string] $Path)
    if ([string]::IsNullOrEmpty($Path)) { return $null }
    $absolute = [IO.Path]::GetFullPath($Path)
    $relative = [IO.Path]::GetRelativePath($script:RepositoryRoot, $absolute)
    if (-not $relative.StartsWith(".." + [IO.Path]::DirectorySeparatorChar) -and $relative -ne "..") {
        return $relative.Replace("\", "/")
    }
    return $absolute.Replace("\", "/")
}

function Get-OptionalArray {
    param(
        [Parameter(Mandatory = $true)] $Object,
        [Parameter(Mandatory = $true)][string] $Name
    )
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property -or $null -eq $property.Value) { return @() }
    return @($property.Value)
}

function Get-DeclaredFeatures {
    param($Package)
    $features = [Collections.Generic.List[object]]::new()
    foreach ($property in @($Package.features.PSObject.Properties | Sort-Object Name)) {
        $features.Add([ordered]@{
            name = [string] $property.Name
            enables = @($property.Value | ForEach-Object { [string] $_ } | Sort-Object)
        })
    }
    return @($features)
}

function Get-TargetRecords {
    param($Package)
    $records = [Collections.Generic.List[object]]::new()
    foreach ($target in @($Package.targets | Sort-Object name, src_path)) {
        $records.Add([ordered]@{
            name = [string] $target.name
            kind = @($target.kind | ForEach-Object { [string] $_ } | Sort-Object)
            crate_types = @($target.crate_types | ForEach-Object { [string] $_ } | Sort-Object)
            required_features = @(Get-OptionalArray -Object $target -Name "required_features" | ForEach-Object { [string] $_ } | Sort-Object)
            src_path = Convert-RepositoryPath -Path ([string] $target.src_path)
            edition = [string] $target.edition
            doctest = [bool] $target.doctest
            test = [bool] $target.test
            doc = [bool] $target.doc
        })
    }
    return @($records)
}

function Get-DependencyKinds {
    param($Dependency)
    $records = [Collections.Generic.List[object]]::new()
    foreach ($kind in @($Dependency.dep_kinds | Sort-Object kind, target)) {
        $kindName = if ($null -eq $kind.kind) { "normal" } else { [string] $kind.kind }
        $records.Add([ordered]@{
            kind = $kindName
            target = if ($null -eq $kind.target) { $null } else { [string] $kind.target }
        })
    }
    return @($records)
}

function Resolve-Closure {
    param([Parameter(Mandatory = $true)][string] $RootId)
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($RootId)
    while ($pending.Count -gt 0) {
        $id = $pending.Pop()
        if (-not $seen.Add($id)) { continue }
        if (-not $script:NodeById.ContainsKey($id)) {
            throw "Resolved package has no dependency node: $id"
        }
        foreach ($dependency in @($script:NodeById[$id].deps)) {
            $pending.Push([string] $dependency.pkg)
        }
    }
    return @($seen | Sort-Object)
}

function Get-PackageSummary {
    param([Parameter(Mandatory = $true)][string] $Id)
    if (-not $script:PackageById.ContainsKey($Id)) { throw "Unknown package id: $Id" }
    $package = $script:PackageById[$Id]
    $node = $script:NodeById[$Id]
    return [ordered]@{
        id = $Id
        name = [string] $package.name
        version = [string] $package.version
        license = if ($null -eq $package.license) { $null } else { [string] $package.license }
        source = if ($null -eq $package.source) { "workspace-or-path" } else { [string] $package.source }
        manifest_path = Convert-RepositoryPath -Path ([string] $package.manifest_path)
        workspace_member = [bool] $script:WorkspaceIds.Contains($Id)
        resolved_features = @($node.features | ForEach-Object { [string] $_ } | Sort-Object)
    }
}

$resolvedRepository = Resolve-Path -LiteralPath $Repository -ErrorAction Stop
$script:RepositoryRoot = [IO.Path]::GetFullPath($resolvedRepository.Path)
$topLevel = & git -C $script:RepositoryRoot rev-parse --show-toplevel 2>$null
if ($LASTEXITCODE -ne 0) { throw "Repository is not a Git work tree: $script:RepositoryRoot" }
$script:RepositoryRoot = [IO.Path]::GetFullPath(([string] $topLevel).Trim())

Push-Location $script:RepositoryRoot
try {
    $metadataOutput = & cargo metadata --locked --offline --format-version 1 2>$null
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata --locked --offline failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}
$metadataText = $metadataOutput -join "`n"
$metadata = $metadataText | ConvertFrom-Json -Depth 100

$script:PackageById = @{}
foreach ($package in $metadata.packages) { $script:PackageById[[string] $package.id] = $package }
$script:NodeById = @{}
foreach ($node in $metadata.resolve.nodes) { $script:NodeById[[string] $node.id] = $node }
$script:WorkspaceIds = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
foreach ($id in $metadata.workspace_members) { [void] $script:WorkspaceIds.Add([string] $id) }

$packageRecords = [Collections.Generic.List[object]]::new()
foreach ($package in @($metadata.packages | Sort-Object id)) {
    $id = [string] $package.id
    if (-not $script:NodeById.ContainsKey($id)) { throw "Package has no resolved node: $id" }
    $node = $script:NodeById[$id]
    $packageRecords.Add([ordered]@{
        id = $id
        name = [string] $package.name
        version = [string] $package.version
        license = if ($null -eq $package.license) { $null } else { [string] $package.license }
        source = if ($null -eq $package.source) { "workspace-or-path" } else { [string] $package.source }
        manifest_path = Convert-RepositoryPath -Path ([string] $package.manifest_path)
        workspace_member = [bool] $script:WorkspaceIds.Contains($id)
        declared_features = Get-DeclaredFeatures -Package $package
        resolved_features = @($node.features | ForEach-Object { [string] $_ } | Sort-Object)
        targets = Get-TargetRecords -Package $package
    })
}

$edgeRecords = [Collections.Generic.List[object]]::new()
foreach ($node in @($metadata.resolve.nodes | Sort-Object id)) {
    foreach ($dependency in @($node.deps | Sort-Object pkg, name)) {
        $edgeRecords.Add([ordered]@{
            from = [string] $node.id
            to = [string] $dependency.pkg
            name = [string] $dependency.name
            dependency_kinds = Get-DependencyKinds -Dependency $dependency
        })
    }
}

$binaryRoots = [Collections.Generic.List[object]]::new()
foreach ($package in @($metadata.packages | Where-Object { $script:WorkspaceIds.Contains([string] $_.id) } | Sort-Object id)) {
    foreach ($target in @($package.targets | Where-Object { @($_.kind) -contains "bin" } | Sort-Object name, src_path)) {
        $binaryRoots.Add([ordered]@{
            selector = "$($package.name)::$($target.name)"
            package_id = [string] $package.id
            package_name = [string] $package.name
            target_name = [string] $target.name
            src_path = Convert-RepositoryPath -Path ([string] $target.src_path)
            required_features = @(Get-OptionalArray -Object $target -Name "required_features" | ForEach-Object { [string] $_ } | Sort-Object)
        })
    }
}

if ($Binary.Count -gt 0) {
    $selected = [Collections.Generic.List[object]]::new()
    foreach ($requested in @($Binary | Sort-Object -Unique)) {
        $matches = @($binaryRoots | Where-Object { $_.selector -eq $requested -or $_.target_name -eq $requested })
        if ($matches.Count -eq 0) { throw "Unknown workspace binary selector: $requested" }
        if ($matches.Count -gt 1) { throw "Ambiguous workspace binary selector: $requested" }
        $selected.Add($matches[0])
    }
    $binaryRoots = $selected
}

$head = (& git -C $script:RepositoryRoot rev-parse HEAD 2>$null).Trim()
if ($LASTEXITCODE -ne 0) { throw "Cannot resolve source commit" }
$lockfile = Join-Path $script:RepositoryRoot "Cargo.lock"
if (-not (Test-Path -LiteralPath $lockfile -PathType Leaf)) { throw "Cargo.lock is required" }

$graph = [ordered]@{
    schema = "eneros.delivery.cargo-graph.v1"
    source_commit = $head
    cargo_lock_sha256 = Get-FileSha256 -Path $lockfile
    workspace_package_count = $script:WorkspaceIds.Count
    resolved_package_count = $packageRecords.Count
    workspace_binary_target_count = $binaryRoots.Count
    packages = @($packageRecords)
    resolved_edges = @($edgeRecords)
    workspace_binaries = @($binaryRoots)
}
$graphJson = ($graph | ConvertTo-Json -Depth 100 -Compress) + "`n"
$graphHash = Get-TextSha256 -Text $graphJson

$reachableWorkspace = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
$binaryBoms = [Collections.Generic.List[object]]::new()
foreach ($root in @($binaryRoots | Sort-Object selector)) {
    $closureIds = @(Resolve-Closure -RootId ([string] $root.package_id))
    $workspaceClosureCount = 0
    $closurePackages = [Collections.Generic.List[object]]::new()
    foreach ($id in $closureIds) {
        if ($script:WorkspaceIds.Contains($id)) {
            $workspaceClosureCount += 1
            [void] $reachableWorkspace.Add($id)
        }
        $closurePackages.Add((Get-PackageSummary -Id $id))
    }
    $binaryBoms.Add([ordered]@{
        selector = [string] $root.selector
        package_id = [string] $root.package_id
        target_name = [string] $root.target_name
        src_path = [string] $root.src_path
        closure_package_count = $closureIds.Count
        workspace_closure_package_count = $workspaceClosureCount
        packages = @($closurePackages)
    })
}

foreach ($external in @($ExternalBinary | Sort-Object -Unique)) {
    $binaryBoms.Add([ordered]@{
        selector = [string] $external
        package_id = $null
        target_name = $null
        src_path = $null
        closure_package_count = 0
        workspace_closure_package_count = 0
        packages = @()
    })
}

$unreachable = [Collections.Generic.List[object]]::new()
foreach ($id in @($script:WorkspaceIds | Sort-Object)) {
    if (-not $reachableWorkspace.Contains($id)) { $unreachable.Add((Get-PackageSummary -Id $id)) }
}
$boms = [ordered]@{
    schema = "eneros.delivery.binary-boms.v1"
    source_commit = $head
    cargo_graph_sha256 = $graphHash
    binary_count = $binaryBoms.Count
    reachable_workspace_package_count = $reachableWorkspace.Count
    unreachable_workspace_package_count = $unreachable.Count
    binaries = @($binaryBoms)
    unreachable_workspace_packages = @($unreachable)
}
$bomsJson = ($boms | ConvertTo-Json -Depth 100 -Compress) + "`n"
$bomsHash = Get-TextSha256 -Text $bomsJson

$outputs = [ordered]@{
    "cargo-graph.json" = $graphJson
    "cargo-graph.json.sha256" = "$graphHash  cargo-graph.json`n"
    "binary-boms.json" = $bomsJson
    "binary-boms.json.sha256" = "$bomsHash  binary-boms.json`n"
}

$directoryArgument = if ($PSCmdlet.ParameterSetName -eq "Verify") { $VerifyDirectory } else { $OutputDirectory }
$directoryPath = if ([IO.Path]::IsPathRooted($directoryArgument)) {
    [IO.Path]::GetFullPath($directoryArgument)
} else {
    [IO.Path]::GetFullPath((Join-Path (Get-Location) $directoryArgument))
}

if ($PSCmdlet.ParameterSetName -eq "Verify") {
    $mismatches = [Collections.Generic.List[object]]::new()
    foreach ($entry in $outputs.GetEnumerator()) {
        $path = Join-Path $directoryPath $entry.Key
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            $mismatches.Add([ordered]@{ file = $entry.Key; reason = "missing" })
            continue
        }
        $actual = Get-Content -LiteralPath $path -Raw -Encoding UTF8
        if ($actual -ne $entry.Value) {
            $mismatches.Add([ordered]@{
                file = $entry.Key
                reason = "content-mismatch"
                expected_sha256 = Get-TextSha256 -Text $entry.Value
                actual_sha256 = Get-TextSha256 -Text $actual
            })
        }
    }
    $result = [ordered]@{
        schema = "eneros.delivery.cargo-closure-verification.v1"
        result = if ($mismatches.Count -eq 0) { "PASS" } else { "FAIL" }
        mismatch_count = $mismatches.Count
        mismatches = @($mismatches)
    }
    $result | ConvertTo-Json -Depth 8
    if ($mismatches.Count -ne 0) { exit 2 }
    exit 0
}

if (-not (Test-Path -LiteralPath $directoryPath -PathType Container)) {
    New-Item -ItemType Directory -Path $directoryPath | Out-Null
}
foreach ($entry in $outputs.GetEnumerator()) {
    [IO.File]::WriteAllText((Join-Path $directoryPath $entry.Key), $entry.Value, [Text.UTF8Encoding]::new($false))
}

[ordered]@{
    schema = "eneros.delivery.cargo-closure-generation.v1"
    result = "PASS"
    workspace_package_count = $script:WorkspaceIds.Count
    resolved_package_count = $packageRecords.Count
    binary_count = $binaryBoms.Count
    reachable_workspace_package_count = $reachableWorkspace.Count
    unreachable_workspace_package_count = $unreachable.Count
    cargo_graph_sha256 = $graphHash
    binary_boms_sha256 = $bomsHash
} | ConvertTo-Json -Depth 8
