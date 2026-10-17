#Requires -Version 7.0
<#
.SYNOPSIS
    Interactive demo: pick an organization, find a repository script, find a device
    in that org, run the script on it, and gather the result — via the N-able
    GraphQL API.

.DESCRIPTION
    Guided, interactive workflow designed for live demos:
      1. Pick an organization
      2. Search for and choose a script in that org
      3. Search for and choose a device in that org
      4. Fill in the script's input variables, confirm, and run
      5. Poll until complete and display the result

    Any search term can be pre-supplied as a parameter; anything omitted is prompted
    for interactively. At each search step you can type 'r' to search again. All
    GraphQL operations are validated against the live N-able schema
    (organizationSearch / organization, scriptSearch, assetSearch, assetsScriptRun,
    task).

.PARAMETER ApiKey
    Your N-able API-Access token, sent as a Bearer credential (Authorization:
    Bearer <token>). Generate it in the N-able dashboard (API client / "Generate
    JSON Web Token"). The token is used exactly as issued — it may be a JWT or an
    N-central-style key (e.g. a 64-hex string with a "-N" suffix). A plain object
    id / UUID is NOT a token and fails with HTTP 403 "RBAC: access denied".
    Defaults to the NABLE_API_KEY environment variable, which keeps the token
    out of your shell history.

.PARAMETER ApiBaseUrl
    N-able GraphQL API endpoint. Default: https://api.n-able.com/graphql

.PARAMETER TaskId
    Resume watching an existing script-run task (the Task ID printed when a run
    starts). Skips steps 1-4 and goes straight to polling for the result.

.PARAMETER OrganizationName
    Optional pre-fill for the organization search. Omit to be prompted.

.PARAMETER OrganizationId
    Optional exact organization ID. Skips the org search.

.PARAMETER ScriptName
    Optional pre-fill for the script search.

.PARAMETER DeviceName
    Optional pre-fill for the device search.

.PARAMETER TaskName
    Friendly label for this run (max 128 chars). Defaults to a timestamped name.

.PARAMETER ScriptInputs
    Optional hashtable of script input values keyed by the script's *variable*
    name. Any declared variable not supplied here is prompted for interactively.
    Example: @{ "targetPath" = "C:\Temp"; "maxRetries" = 3 }

.PARAMETER ScriptTimeoutSeconds
    Per-run execution timeout (0-3600). 0 (default) uses the script's own
    configured timeout; the API treats 0 as the max (3600s).

.PARAMETER PollIntervalSeconds
    How often (seconds) to check task status. Default: 5.

.PARAMETER MaxWaitSeconds
    Stop polling after this many seconds even if unfinished. Default (0): the
    run's timeout plus 5 minutes for the agent to pick up the task.

.PARAMETER AssumeYes
    Skip the final "run this now?" confirmation (useful for scripted runs).

.EXAMPLE
    $env:NABLE_API_KEY = "KEYHERE"
    .\Invoke-NableScriptRun.ps1
    # Fully interactive — prompts for org, script, device, and inputs.

.EXAMPLE
    .\Invoke-NableScriptRun.ps1 -OrganizationName "FocusMSP" -ScriptName "App Usage Tracker" -DeviceName "SLS-2188"
    # Pre-fills every search term; exact name matches are selected automatically.

.EXAMPLE
    .\Invoke-NableScriptRun.ps1 -TaskId "MDFhMTRhZjMt...NQ=="
    # Resume polling a run that was interrupted or timed out.
#>

[CmdletBinding()]
param(
  [string]$ApiKey = $env:NABLE_API_KEY,

  [string]$ApiBaseUrl = 'https://api.n-able.com/graphql',

  [string]$TaskId,

  [string]$OrganizationName,

  [string]$OrganizationId,

  [string]$ScriptName,

  [string]$DeviceName,

  [string]$TaskName = "Demo Run - $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')",

  [hashtable]$ScriptInputs = @{},

  [ValidateRange(0, 3600)]
  [int]$ScriptTimeoutSeconds = 0,

  [ValidateRange(1, 3600)]
  [int]$PollIntervalSeconds = 5,

  [ValidateRange(0, 86400)]
  [int]$MaxWaitSeconds = 0,

  [switch]$AssumeYes
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($ApiKey)) {
  throw "No API key. Pass -ApiKey or set the NABLE_API_KEY environment variable."
}

# Work on a copy so prompted values aren't written back into the caller's hashtable.
$ScriptInputs = $ScriptInputs.Clone()

# Partial-data GraphQL errors already reported, so polling doesn't repeat them.
$ReportedPartialErrors = [System.Collections.Generic.HashSet[string]]::new()

# ---------------------------------------------------------------------------
# Helper: Execute a GraphQL operation
# ---------------------------------------------------------------------------
function Invoke-GraphQL {
  param(
    [Parameter(Mandatory)][string]$Query,
    [hashtable]$Variables = @{},
    [switch]$ToleratePartialData
  )

  $body = @{
    query = $Query
    variables = $Variables
  } | ConvertTo-Json -Depth 20 -Compress

  $headers = @{
    'Content-Type'  = 'application/json'
    'Authorization' = "Bearer $ApiKey"
  }

  try {
    $statusCode = 0
    $response = Invoke-RestMethod -Uri $ApiBaseUrl -Method POST -Headers $headers `
      -Body $body -SkipHttpErrorCheck -StatusCodeVariable statusCode
  }
  catch {
    throw "N-able API request failed (transport error): $($_.Exception.Message)"
  }

  if ($null -ne $response -and $response.PSObject.Properties['errors'] -and $response.errors) {
    $errMsg = ($response.errors | ForEach-Object { $_.message }) -join '; '
    $hasData = $response.PSObject.Properties['data'] -and $null -ne $response.data
    if ($ToleratePartialData -and $hasData) {
      if ($ReportedPartialErrors.Add($errMsg)) {
        Write-Host ""
        Write-Warning "Partial GraphQL response [HTTP $statusCode]: $errMsg"
      }
    }
    else {
      throw "GraphQL error(s) [HTTP $statusCode]: $errMsg"
    }
  }

  if ($statusCode -ge 400) {
    throw "N-able API returned HTTP $statusCode with no GraphQL error detail."
  }

  if ($null -eq $response -or -not $response.PSObject.Properties['data']) {
    throw "N-able API response contained no 'data' field."
  }

  return $response.data
}

# ---------------------------------------------------------------------------
# Console helpers
# ---------------------------------------------------------------------------
function Write-Step { param([string]$Message) Write-Host "`n==> $Message" -ForegroundColor Cyan }
function Write-Success { param([string]$Message) Write-Host "    [OK] $Message" -ForegroundColor Green }
function Write-Info { param([string]$Message) Write-Host "    $Message" -ForegroundColor Gray }

function Get-OrgName {
  param($OrgObject)
  if ($OrgObject -and $OrgObject.PSObject.Properties['name'] -and $OrgObject.name) {
    return $OrgObject.name
  }
  return '(none)'
}

function Select-Interactively {
  param(
    [Parameter(Mandatory)][string]$Label,
    [Parameter(Mandatory)][scriptblock]$SearchFn,
    [Parameter(Mandatory)][scriptblock]$FormatFn,
    [Parameter(Mandatory)][int]$PageSize,
    [string]$InitialTerm
  )

  $term = $InitialTerm
  $isInitialTerm = -not [string]::IsNullOrWhiteSpace($InitialTerm)
  while ($true) {
    if ([string]::IsNullOrWhiteSpace($term)) {
      $term = Read-Line "    Search for $Label by name"
      if ([string]::IsNullOrWhiteSpace($term)) { continue }
    }

    Write-Host "    Searching for $Label matching '$term'..." -ForegroundColor DarkGray
    $results = @(& $SearchFn $term | Where-Object { $_ })

    if ($results.Count -eq 0) {
      Write-Host "    No $Label found matching '$term'. Try a different term." -ForegroundColor Yellow
      $term = $null
      $isInitialTerm = $false
      continue
    }

    # A pre-supplied name that matches exactly one result needs no prompt.
    if ($isInitialTerm) {
      $exact = @($results | Where-Object { $_.name -eq $term })
      if ($exact.Count -eq 1) {
        Write-Host "    Exact match: $(& $FormatFn $exact[0])" -ForegroundColor DarkGray
        return $exact[0]
      }
      $isInitialTerm = $false
    }

    while ($true) {
      Write-Host ""
      for ($i = 0; $i -lt $results.Count; $i++) {
        Write-Host ("    [{0}] {1}" -f $i, (& $FormatFn $results[$i]))
      }
      if ($results.Count -ge $PageSize) {
        Write-Host "    (Showing the first $PageSize matches — search again with a more specific term to narrow it down.)" -ForegroundColor DarkGray
      }
      Write-Host "    [r] Search again"
      $sel = Read-Line "    Select $Label (0-$($results.Count - 1), or r)"

      if ($sel -in @('r', 'R')) { $term = $null; $isInitialTerm = $false; break } # re-search
      if ($sel -match '^\d{1,9}$' -and [int]$sel -lt $results.Count) {
        return $results[[int]$sel]
      }
      Write-Host "    Invalid selection — enter 0-$($results.Count - 1) or 'r'." -ForegroundColor Yellow
    }
  }
}

# Merges a search's aliased `byName` / `byText` results: name matches first,
# then extra whole-word matches, de-duplicated and capped at one page.
function Merge-SearchResults {
  param([Parameter(Mandatory)]$Data, [Parameter(Mandatory)][int]$PageSize)
  $seen = [System.Collections.Generic.HashSet[string]]::new()
  $(foreach ($n in @($Data.byName.nodes) + @($Data.byText.nodes)) {
      if ($n -and $seen.Add($n.id)) { $n }
    }) | Select-Object -First $PageSize
}

# Read-Host replacement. On Linux/macOS pwsh, Read-Host swallows Ctrl+C
# (the keypress shows up as garbage input), leaving no way out of a prompt.
# Reading key-by-key with TreatControlCAsInput lets us catch Ctrl+C ourselves.
function Read-Line {
  param([Parameter(Mandatory)][string]$Prompt, [switch]$Mask)

  Write-Host "${Prompt}: " -NoNewline
  if ([Console]::IsInputRedirected) {
    $line = [Console]::In.ReadLine()
    if ($null -eq $line) { throw "Input ended before a value was entered." }
    Write-Host $(if ($Mask) { '*' * $line.Length } else { $line })
    return $line
  }

  $buffer = [System.Text.StringBuilder]::new()
  $previous = [Console]::TreatControlCAsInput
  [Console]::TreatControlCAsInput = $true
  try {
    while ($true) {
      $key = [Console]::ReadKey($true)
      if ($key.Key -eq [ConsoleKey]::C -and ($key.Modifiers -band [ConsoleModifiers]::Control)) {
        Write-Host '^C'
        Write-Host "    Cancelled." -ForegroundColor Yellow
        exit 130
      }
      if ($key.Key -eq [ConsoleKey]::Enter) { Write-Host ''; return $buffer.ToString() }
      if ($key.Key -eq [ConsoleKey]::Backspace) {
        if ($buffer.Length -gt 0) { $null = $buffer.Remove($buffer.Length - 1, 1); Write-Host "`b `b" -NoNewline }
        continue
      }
      if ($key.KeyChar -and -not [char]::IsControl($key.KeyChar)) {
        $null = $buffer.Append($key.KeyChar)
        Write-Host $(if ($Mask) { '*' } else { $key.KeyChar }) -NoNewline
      }
    }
  }
  finally {
    [Console]::TreatControlCAsInput = $previous
  }
}

function Read-YesNo {
  param([string]$Prompt, [switch]$DefaultYes)
  $suffix = if ($DefaultYes) { '[Y/n]' } else { '[y/N]' }
  $answer = Read-Line "$Prompt $suffix"
  if ([string]::IsNullOrWhiteSpace($answer)) { return [bool]$DefaultYes }
  return $answer -match '^(y|yes)$'
}

# Converts a raw input (typed text or a -ScriptInputs value) into the API's
# typed value object. Throws a short reason when the value doesn't fit the type.
function ConvertTo-ScriptInputValue {
  param([Parameter(Mandatory)][string]$Type, [Parameter(Mandatory)]$Raw)

  switch ($Type) {
    'BOOLEAN' {
      if ($Raw -is [bool]) { return @{ booleanValue = $Raw } }
      if ("$Raw" -match '^(true|yes|y|1)$') { return @{ booleanValue = $true } }
      if ("$Raw" -match '^(false|no|n|0)$') { return @{ booleanValue = $false } }
      throw "expected true or false"
    }
    'INT' {
      $n = 0
      if ([int]::TryParse("$Raw", [ref]$n)) { return @{ intValue = $n } }
      throw "expected a whole number"
    }
    'FLOAT' {
      $d = 0.0
      if ([double]::TryParse("$Raw", [Globalization.NumberStyles]::Float,
          [Globalization.CultureInfo]::InvariantCulture, [ref]$d)) { return @{ floatValue = $d } }
      throw "expected a number"
    }
    'DATE_TIME' {
      $dt = [datetimeoffset]::MinValue
      if ($Raw -is [datetime] -or $Raw -is [datetimeoffset]) { $dt = [datetimeoffset]$Raw }
      elseif (-not [datetimeoffset]::TryParse("$Raw", [Globalization.CultureInfo]::InvariantCulture,
          [Globalization.DateTimeStyles]::AssumeLocal, [ref]$dt)) {
        throw "expected a date/time, e.g. 2026-10-17T13:00:00Z"
      }
      return @{ dateTimeValue = $dt.UtcDateTime.ToString("yyyy-MM-dd'T'HH:mm:ss'Z'") }
    }
    default { return @{ stringValue = "$Raw" } } # STRING, PASSWORD
  }
}

# ===========================================================================
Write-Host ""
Write-Host "  N-able GraphQL — Script Execution Demo" -ForegroundColor White
Write-Host "  ---------------------------------------" -ForegroundColor White

if ($TaskId) {
  # =========================================================================
  # RESUME — skip steps 1-4 and watch an existing task
  # =========================================================================
  Write-Step "Resuming existing task"

  $taskInfoQuery = @'
query GetTask($id: ID!) {
  task(id: $id) {
    id
    NAME_FIELD
    status
    ... on RunScriptTask { script { id name } }
  }
}
'@
  # On a just-created task `name` resolves to null, which nulls the whole task:
  # if that happens, ask again without it.
  $taskInfo = (Invoke-GraphQL -Query $taskInfoQuery.Replace('NAME_FIELD', 'name') -Variables @{ id = $TaskId } -ToleratePartialData).task
  if (-not $taskInfo) {
    $taskInfo = (Invoke-GraphQL -Query $taskInfoQuery.Replace('NAME_FIELD', '') -Variables @{ id = $TaskId }).task
  }
  if (-not $taskInfo) { throw "No task found with ID '$TaskId'." }

  # Script-run task IDs are base64 "<task uuid>:<asset uuid>"; the task itself
  # doesn't expose its asset, so recover the device from the ID.
  $asset = $null
  try {
    $assetId = ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($TaskId)) -split ':')[1]
    $asset = (Invoke-GraphQL -Query 'query GetAsset($id: ID!) { asset(id: $id) { id name } }' -Variables @{ id = $assetId }).asset
  }
  catch { Write-Verbose "Could not resolve the task's device: $($_.Exception.Message)" }
  if (-not $asset) { $asset = [PSCustomObject]@{ id = $null; name = '(unknown device)' } }

  $selectedScript = if ($taskInfo.PSObject.Properties['script'] -and $taskInfo.script) { $taskInfo.script }
  else { [PSCustomObject]@{ id = $null; name = '(unknown script)' } }
  $effectiveName = if ($taskInfo.PSObject.Properties['name'] -and $taskInfo.name) { $taskInfo.name } else { '(unnamed task)' }
  $effectiveTimeout = 0 # not exposed on the task, so the wait falls back to the maximum

  Write-Success "Found task: $effectiveName"
  Write-Info "  Status: $($taskInfo.status)"
  Write-Info "  Script: $($selectedScript.name)"
  Write-Info "  Device: $($asset.name)"
}
else {
  # ===========================================================================
  # STEP 1 — Choose the organization
  # ===========================================================================
  Write-Step "Step 1: Select organization"

  if ($OrganizationId) {
    $orgData = Invoke-GraphQL -Query @'
  query GetOrg($id: ID!) {
    organization(id: $id) { id name __typename }
  }
'@ -Variables @{ id = $OrganizationId }
    $org = $orgData.organization
    if (-not $org) {
      throw "No organization found with ID '$OrganizationId'."
    }
  }
  else {
    $orgSearchQuery = @'
  query FindOrgs($name: String!) {
    organizationSearch(where: { name: { contains: $name } }, first: 25) {
      nodes { id name __typename }
    }
  }
'@
    $org = Select-Interactively -Label 'organization' -InitialTerm $OrganizationName -PageSize 25 `
      -SearchFn {
      param($term)
      (Invoke-GraphQL -Query $orgSearchQuery -Variables @{ name = $term }).organizationSearch.nodes
    } `
      -FormatFn { param($n) "$($n.name)  ($($n.__typename))" }
  }

  Write-Success "Using organization: $($org.name)"
  Write-Info "  ID:   $($org.id)"
  Write-Info "  Type: $($org.__typename)"

  # ===========================================================================
  # STEP 2 — Find and choose the script (within the selected org)
  # ===========================================================================
  Write-Step "Step 2: Find a script to execute"

  # `name` does a case-insensitive substring match; `text` is a whole-word
  # search that also covers the description. Run both and merge.
  $scriptFields = 'nodes { id name language description timeoutSeconds inputs { name variable type } }'
  $scriptSearchQuery = @"
  query FindScript(`$orgIds: [ID!]!, `$name: String!) {
    byName: scriptSearch(inOrganizations: `$orgIds, where: { name: { contains: `$name } }, first: 15) { $scriptFields }
    byText: scriptSearch(inOrganizations: `$orgIds, where: { text: { contains: `$name } }, first: 15) { $scriptFields }
  }
"@

  $selectedScript = Select-Interactively -Label 'script' -InitialTerm $ScriptName -PageSize 15 `
    -SearchFn {
    param($term)
    Merge-SearchResults (Invoke-GraphQL -Query $scriptSearchQuery -Variables @{ orgIds = @($org.id); name = $term }) -PageSize 15
  } `
    -FormatFn { param($n) "$($n.name)  ($($n.language))" }

  $declaredInputs = @($selectedScript.inputs | Where-Object { $_ })

  Write-Success "Selected script: $($selectedScript.name)"
  Write-Info "  ID:       $($selectedScript.id)"
  Write-Info "  Language: $($selectedScript.language)"
  if ($selectedScript.description) { Write-Info "  Description: $($selectedScript.description)" }
  if ($declaredInputs.Count -gt 0) {
    Write-Info "  Input variables:"
    $declaredInputs | ForEach-Object { Write-Info "    - $($_.name) [$($_.variable)] ($($_.type))" }
  }

  # ===========================================================================
  # STEP 3 — Find and choose the device (within the selected org)
  # ===========================================================================
  Write-Step "Step 3: Find a device to run it against"

  $assetFields = 'nodes { id name customer { name } site { name } agentConnection { status } }'
  $assetSearchQuery = @"
  query FindAsset(`$orgIds: [ID!]!, `$name: String!) {
    byName: assetSearch(inOrganizations: `$orgIds, where: { name: { contains: `$name } }, first: 10) { $assetFields }
    byText: assetSearch(inOrganizations: `$orgIds, where: { text: { contains: `$name } }, first: 10) { $assetFields }
  }
"@

  $asset = Select-Interactively -Label 'device' -InitialTerm $DeviceName -PageSize 10 `
    -SearchFn {
    param($term)
    Merge-SearchResults (Invoke-GraphQL -Query $assetSearchQuery -Variables @{ orgIds = @($org.id); name = $term }) -PageSize 10
  } `
    -FormatFn {
    param($n)
    $status = if ($n.agentConnection) { $n.agentConnection.status } else { 'UNKNOWN' }
    "$($n.name)  [$status]  (Customer: $(Get-OrgName $n.customer), Site: $(Get-OrgName $n.site))"
  }

  $agentStatus = if ($asset.agentConnection) { $asset.agentConnection.status } else { 'UNKNOWN' }

  Write-Success "Selected device: $($asset.name)"
  Write-Info "  ID:       $($asset.id)"
  Write-Info "  Agent:    $agentStatus"
  Write-Info "  Customer: $(Get-OrgName $asset.customer)"
  Write-Info "  Site:     $(Get-OrgName $asset.site)"
  if ($agentStatus -ne 'CONNECTED') {
    Write-Host "    [warn] Agent is $agentStatus — the run may not execute until the device reconnects." -ForegroundColor Yellow
  }

  # ===========================================================================
  # STEP 4 — Collect inputs, confirm, and kick off the run
  # ===========================================================================
  Write-Step "Step 4: Configure and run"

  if ($declaredInputs.Count -gt 0) {
    Write-Info "Enter values for the script's input variables (leave blank to skip):"
    foreach ($decl in $declaredInputs) {
      if ($ScriptInputs.ContainsKey($decl.variable)) {
        Write-Info "  $($decl.name) [$($decl.variable)] = (supplied via -ScriptInputs)"
        continue
      }

      if ($decl.type -eq 'PASSWORD') {
        $secret = Read-Line "      $($decl.name) [$($decl.variable)] (PASSWORD)" -Mask
        if ($secret.Length -gt 0) {
          $ScriptInputs[$decl.variable] = $secret
        }
      }
      else {
        while ($true) {
          $val = Read-Line "      $($decl.name) [$($decl.variable)] ($($decl.type))"
          if ([string]::IsNullOrWhiteSpace($val)) { break }
          try {
            $null = ConvertTo-ScriptInputValue -Type $decl.type -Raw $val
            $ScriptInputs[$decl.variable] = $val
            break
          }
          catch {
            Write-Host "      Invalid value — $($_.Exception.Message). Try again or leave blank to skip." -ForegroundColor Yellow
          }
        }
      }
    }
  }

  $declaredVarNames = @($declaredInputs | ForEach-Object { $_.variable })
  foreach ($key in $ScriptInputs.Keys) {
    if ($declaredVarNames -notcontains $key) {
      Write-Host "    [warn] Input '$key' is not a declared variable on this script — ignored." -ForegroundColor Yellow
    }
  }

  $inputValues = @()
  foreach ($decl in $declaredInputs) {
    if (-not $ScriptInputs.ContainsKey($decl.variable)) { continue }
    try {
      $valueObj = ConvertTo-ScriptInputValue -Type $decl.type -Raw $ScriptInputs[$decl.variable]
    }
    catch {
      throw "Input '$($decl.variable)' ($($decl.type)): $($_.Exception.Message)."
    }

    $inputValues += @{
      name = $decl.name
      variable = $decl.variable
      type = $decl.type
      value = $valueObj
    }
  }

  $effectiveTimeout = if ($ScriptTimeoutSeconds -gt 0) { $ScriptTimeoutSeconds } else { [int]$selectedScript.timeoutSeconds }
  $effectiveName = if ($TaskName.Length -gt 128) { $TaskName.Substring(0, 128) } else { $TaskName }

  Write-Host ""
  Write-Host "    About to run:" -ForegroundColor White
  Write-Host "      Script:   $($selectedScript.name)  ($($selectedScript.language))"
  Write-Host "      Device:   $($asset.name)  [$agentStatus]"
  Write-Host "      Org:      $($org.name)"
  Write-Host "      Task:     $effectiveName"
  Write-Host "      Timeout:  $(if ($effectiveTimeout -eq 0) { 'max (3600s)' } else { "${effectiveTimeout}s" })"
  if ($inputValues.Count -gt 0) {
    Write-Host "      Inputs:"
    $inputValues | ForEach-Object {
      $shown = if ($_.type -eq 'PASSWORD') { '********' } else { $_.value.Values | Select-Object -First 1 }
      Write-Host "        - $($_.variable) = $shown"
    }
  }
  Write-Host ""

  if (-not $AssumeYes) {
    if (-not (Read-YesNo -Prompt "    Run this script now?")) {
      Write-Host "    Cancelled — nothing was executed." -ForegroundColor Yellow
      exit 0
    }
  }

  $runMutation = @'
  mutation RunScript($input: AssetsScriptRunMutationInput!) {
    assetsScriptRun(input: $input) {
      # Only select `id` here. On a just-created task the non-nullable String
      # fields (name) resolve to null and error the whole response; name/status
      # are read from the polling step instead.
      items { id }
      errors {
        __typename
        ... on ScriptRunNotPermittedError   { message asset { id name } }
        ... on ScriptRunValidationError     { message asset { id name } }
        ... on ScriptRunAssetNotFoundError  { message assetId }
        ... on ScriptRunScriptNotFoundError { message assetId }
      }
    }
  }
'@

  $configuration = @{
    scriptId = $selectedScript.id
    name = $effectiveName
    timeoutSeconds = $effectiveTimeout
  }
  if ($inputValues.Count -gt 0) { $configuration.inputs = @($inputValues) }

  $runVariables = @{
    input = @{
      assetIds = @($asset.id)
      configuration = $configuration
    }
  }

  $runData = Invoke-GraphQL -Query $runMutation -Variables $runVariables
  $runResult = $runData.assetsScriptRun

  $runErrors = @()
  if ($runResult.PSObject.Properties['errors'] -and $runResult.errors) { $runErrors = @($runResult.errors) }
  $runItems = @()
  if ($runResult.PSObject.Properties['items'] -and $runResult.items) { $runItems = @($runResult.items) }

  if ($runItems.Count -eq 0) {
    $detail = ($runErrors | ForEach-Object { $_.message }) -join '; '
    throw "Script run was not created. $detail"
  }

  # Partial success is possible: a task was created but some assets errored.
  foreach ($e in $runErrors) { Write-Host "    [warn] $($e.message)" -ForegroundColor Yellow }

  $runTask = $runItems[0]
  $taskId = $runTask.id

  Write-Success "Script run initiated!"
  Write-Info "  Task ID:   $taskId"
  Write-Info "  Task Name: $effectiveName"
}

# ===========================================================================
# STEP 5 — Poll until complete and gather the result
# ===========================================================================
# Default wait: the run's timeout (0 = API max of 3600s) plus time for the
# agent to pick up the task.
$effectiveWait = if ($MaxWaitSeconds -gt 0) { $MaxWaitSeconds }
elseif ($effectiveTimeout -gt 0) { $effectiveTimeout + 300 }
else { 3600 + 300 }
$resumeHint = "Resume later with: -TaskId '$taskId'"

Write-Step "Step 5: Gathering result (polling every ${PollIntervalSeconds}s, up to ${effectiveWait}s)..."

# Poll the task directly rather than scanning the asset's execution history:
# a queued run has no execution yet, and the task's own status tells us
# whether it is still waiting on the agent.
$taskPollQuery = @'
query PollTask($taskId: ID!) {
  task(id: $taskId) {
    status
    executions {
      nodes {
        id
        status
        exitCode
        errorMessage
        startedAt
        updatedAt
        durationMilliseconds
        # The script's console output lives on the concrete ScriptTaskExecution
        # type (the base TaskExecution interface only exposes download/link).
        ... on ScriptTaskExecution {
          output {
            runAs
            stdOutput
            stdError
            outputs {
              name
              variable
              type
              value { stringValue booleanValue intValue floatValue dateTimeValue }
            }
          }
        }
      }
    }
  }
}
'@

$pendingStatuses = 'IN_PROGRESS', 'PENDING', 'QUEUED', 'SCHEDULED', 'CREATED', 'RUNNING', 'STARTED'
# Execution statuses that mean the device hasn't started the script yet.
$notStartedStatuses = 'PENDING', 'QUEUED', 'SCHEDULED', 'CREATED'
$spinChars = '|', '/', '-', '\'
$spin = 0
$startTime = Get-Date
$timedOut = $false
$exec = $null
$taskStatus = $null
$execDone = $false
$taskDone = $false
$pollFailures = 0
$maxPollFailures = 5

$pollingStopped = $false
try {
  while ($true) {
    Start-Sleep -Seconds $PollIntervalSeconds

    # A transient API failure (429, 5xx, network blip) shouldn't abandon a run
    # that is still executing — retry, and only give up after repeated failures.
    try {
      $pollData = Invoke-GraphQL -Query $taskPollQuery -Variables @{ taskId = $taskId } -ToleratePartialData
      $pollFailures = 0
    }
    catch {
      $pollFailures++
      Write-Host ""
      Write-Host "    [warn] Status check failed ($pollFailures/$maxPollFailures): $($_.Exception.Message)" -ForegroundColor Yellow
      if ($pollFailures -ge $maxPollFailures) {
        $pollingStopped = $true
        throw "Gave up after $maxPollFailures failed status checks in a row. The task may still be running. $resumeHint"
      }
      $pollData = $null
    }

    if ($pollData) {
      $nodes = @()
      if ($pollData.task) {
        $taskStatus = $pollData.task.status
        if ($pollData.task.executions) { $nodes = @($pollData.task.executions.nodes | Where-Object { $_ }) }
      }
      $exec = $nodes | Select-Object -First 1
    }

    $elapsed = [int](New-TimeSpan -Start $startTime -End (Get-Date)).TotalSeconds

    # Done when the execution reaches a terminal state, or the task itself ends
    # (e.g. cancelled, or expired before the agent picked it up) while its
    # execution never started. A task that ends while its script is still
    # running is watched until the execution catches up, unless it was cancelled.
    $execDone = $exec -and $exec.status -notin $pendingStatuses
    $taskEnded = $taskStatus -and $taskStatus -notin $pendingStatuses
    $taskDone = $taskEnded -and (-not $exec -or $exec.status -in $notStartedStatuses -or $taskStatus -match 'CANCEL')
    if ($execDone -or $taskDone) {
      Write-Host "`r    Finished in ${elapsed}s.                                                  "
      $pollingStopped = $true
      break
    }

    $phase = if ($exec) { "Running on $($asset.name) [$($exec.status)]" }
    else { "Queued — waiting for $($asset.name) to pick up the task [$taskStatus]" }
    Write-Host "`r    $($spinChars[$spin % 4]) $phase...  (${elapsed}s elapsed)  " -NoNewline
    $spin++

    if ($elapsed -ge $effectiveWait) {
      Write-Host ""
      if ($exec) {
        Write-Warning "Execution still $($exec.status) after ${elapsed}s. The run may still be in progress on the device."
      }
      else {
        Write-Warning "The device never picked up the task within ${elapsed}s (task status: $taskStatus). Check the agent's script-run service on $($asset.name)."
      }
      Write-Host "    $resumeHint" -ForegroundColor Yellow
      $timedOut = $true
      $pollingStopped = $true
      break
    }
  }
}
finally {
  # Also reached on Ctrl+C: the task keeps running on the device, so say how to pick it back up.
  if (-not $pollingStopped) { Write-Host "`n    Stopped watching. $resumeHint" -ForegroundColor Yellow }
}

# ===========================================================================
# Results
# ===========================================================================
$runStatus = if ($execDone) { $exec.status } elseif ($taskDone) { $taskStatus } elseif ($exec) { $exec.status } elseif ($timedOut) { 'PENDING' } elseif ($taskStatus) { $taskStatus } else { 'UNKNOWN' }

$exitCodeDisplay = if ($exec -and $null -ne $exec.exitCode) { $exec.exitCode } else { 'N/A' }
$startedDisplay = if ($exec) { $exec.startedAt } else { 'N/A' }
$updatedDisplay = if ($exec) { $exec.updatedAt } else { 'N/A' }
$durationDisplay = if ($exec -and $null -ne $exec.durationMilliseconds) { "$([int]$exec.durationMilliseconds) ms" } else { 'N/A' }

$errText = if ($exec -and $exec.PSObject.Properties['errorMessage'] -and $exec.errorMessage) { $exec.errorMessage } else { '' }

$stdOut = ''
$stdErr = ''
$outputVars = @()
if ($exec -and $exec.PSObject.Properties['output'] -and $exec.output) {
  $scriptOutput = $exec.output
  if ($scriptOutput.PSObject.Properties['stdOutput'] -and $scriptOutput.stdOutput) { $stdOut = $scriptOutput.stdOutput }
  if ($scriptOutput.PSObject.Properties['stdError'] -and $scriptOutput.stdError) { $stdErr = $scriptOutput.stdError }
  if ($scriptOutput.PSObject.Properties['outputs'] -and $scriptOutput.outputs) { $outputVars = @($scriptOutput.outputs) }
}

Write-Host "`n========================================" -ForegroundColor White
Write-Host "  RESULT" -ForegroundColor White
Write-Host "========================================" -ForegroundColor White

$statusColor = if ($runStatus -eq 'SUCCEEDED') { 'Green' }
elseif ($timedOut) { 'Yellow' }
else { 'Red' }

Write-Host "  Status:    " -NoNewline
Write-Host $runStatus -ForegroundColor $statusColor
Write-Host "  Device:    $($asset.name)"
Write-Host "  Script:    $($selectedScript.name)"
Write-Host "  Exit Code: $exitCodeDisplay"
Write-Host "  Duration:  $durationDisplay"
Write-Host "  Started:   $startedDisplay"
Write-Host "  Updated:   $updatedDisplay"

if ($outputVars.Count -gt 0) {
  Write-Host "`n  Output variables:" -ForegroundColor White
  foreach ($ov in $outputVars) {
    $v = $ov.value
    $shown = if ($v) {
      $v.stringValue, $v.booleanValue, $v.intValue, $v.floatValue, $v.dateTimeValue |
        Where-Object { $null -ne $_ } | Select-Object -First 1
    }
    else { '' }
    Write-Host "    - $($ov.variable) = $shown"
  }
}

if ($stdOut) {
  Write-Host "`n  ----- Script Output (stdout) --------------------" -ForegroundColor White
  Write-Host $stdOut
  Write-Host "  -------------------------------------------------" -ForegroundColor White
}

if ($stdErr) {
  Write-Host "`n  ----- Script Errors (stderr) --------------------" -ForegroundColor Red
  Write-Host $stdErr -ForegroundColor Red
  Write-Host "  -------------------------------------------------" -ForegroundColor Red
}

if ($errText) {
  Write-Host "`n  Execution error: $errText" -ForegroundColor Red
}

Write-Host "========================================`n" -ForegroundColor White

# Return a structured result for pipeline use
[PSCustomObject]@{
  TaskId = $taskId
  TaskName = $effectiveName
  Status = $runStatus
  ExitCode = if ($exec) { $exec.exitCode } else { $null }
  DurationMs = if ($exec) { $exec.durationMilliseconds } else { $null }
  StdOutput = $stdOut
  StdError = $stdErr
  ErrorMessage = $errText
  StartedAt = $startedDisplay
  UpdatedAt = $updatedDisplay
  DeviceName = $asset.name
  ScriptName = $selectedScript.name
  ExecutionId = if ($exec) { $exec.id } else { $null }
  TimedOut = $timedOut
}
