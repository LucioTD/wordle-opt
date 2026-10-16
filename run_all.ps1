# Full pipeline for Windows PowerShell (5.1 and 7). Resumable: rerun after an
# interruption and finished work is skipped (shortlists also resume per opener).
#
#   powershell -ExecutionPolicy Bypass -File .\run_all.ps1 -Level Quick
#   powershell -ExecutionPolicy Bypass -File .\run_all.ps1            # Full (default)
#   powershell -ExecutionPolicy Bypass -File .\run_all.ps1 -Level Everything
#
# Quick      : beam searches only, normal + hard mode
# Full       : + EXACT search on the top openers per objective and the top fixed pairs
# Everything : + EXACT search on the whole auto shortlist
param([ValidateSet("Quick","Full","Everything")][string]$Level = "Full")

# Native tools (cargo, our binary) print progress to stderr. Windows PowerShell
# would treat that as an error under "Stop", so we use Continue and check exit
# codes ourselves.
$ErrorActionPreference = "Continue"
Set-Location $PSScriptRoot
New-Item -ItemType Directory -Force results | Out-Null
$log = Join-Path $PSScriptRoot "results\run_log.txt"
$B = Join-Path $PSScriptRoot "target\release\wordle-opt.exe"

function Run([string]$exe, [string[]]$argList) {
    $t = Get-Date
    Write-Host ("==> " + $exe + " " + ($argList -join " ")) -ForegroundColor Green
    & $exe @argList 2>&1 | ForEach-Object {
        $line = "$_"
        Write-Host $line
        Add-Content -Path $log -Value $line
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED (exit $LASTEXITCODE). Rerun the script to resume." -ForegroundColor Red
        exit 1
    }
    Write-Host ("    done in {0:N1} min" -f ((Get-Date) - $t).TotalMinutes) -ForegroundColor DarkGray
}
function Has([string]$f) { Test-Path (Join-Path "results" $f) }
function Wb([string[]]$a) { Run $B $a }

Run "cargo" @("build", "--release")
Run "cargo" @("test", "--release")
Wb @("build-matrix")

foreach ($u in "full","nyt2309","unused","historical") {
    if (-not (Has "rank_first_$u.json")) { Wb @("rank-first","--universe",$u,"--p3","0") }
}
foreach ($u in "full","unused","historical") {
    if (-not (Has "pairs_$u.json")) { Wb @("rank-pairs","--universe",$u,"--top","100") }
}

# ---- Normal mode, both objectives: shortlist -> wider beam -> exact ----
foreach ($mode in "expected","le4") {
    Wb @("shortlist","--universe","full","--auto","10","--beam","30","--mode",$mode)
    Wb @("shortlist","--universe","full","--from","shortlist_full_${mode}_b30.json","--top","12","--beam","200","--mode",$mode)
    if ($Level -eq "Full") {
        Wb @("shortlist","--universe","full","--from","shortlist_full_${mode}_b200.json","--top","5","--beam","0","--mode",$mode)
    }
    if ($Level -eq "Everything") {
        Wb @("shortlist","--universe","full","--auto","10","--beam","0","--mode",$mode)
    }
    if ($Level -eq "Quick") { $src = "shortlist_full_${mode}_b200.json"; $beam = "200" }
    else { $src = "shortlist_full_${mode}_exact.json"; $beam = "0" }
    if (-not (Has "tree_full_$mode.json")) {
        Wb @("solve-tree","--universe","full","--first","best:$src","--beam",$beam,"--mode",$mode,"--json","tree_full_$mode.json")
    }
}

# ---- Fixed pairs played out with the best follow-up ----
if ($Level -eq "Quick") { $pb = "200" } else { $pb = "0" }
Wb @("pair-trees","--universe","full","--top","5","--beam",$pb,"--mode","expected")

# ---- Hard mode (slower: guesses depend on the previous feedback) ----
Wb @("shortlist","--universe","full","--auto","6","--beam","30","--mode","expected","--hard")
if ($Level -eq "Quick") { $hsrc = "shortlist_full_expected-hard_b30.json"; $hb = "30" }
else {
    Wb @("shortlist","--universe","full","--from","shortlist_full_expected-hard_b30.json","--top","8","--beam","100","--mode","expected","--hard")
    Wb @("shortlist","--universe","full","--from","shortlist_full_expected-hard_b100.json","--top","3","--beam","0","--mode","expected","--hard")
    $hsrc = "shortlist_full_expected-hard_exact.json"; $hb = "0"
}
if (-not (Has "tree_full_expected-hard.json")) {
    Wb @("solve-tree","--universe","full","--first","best:$hsrc","--beam",$hb,"--mode","expected","--hard","--json","tree_full_expected-hard.json")
}

# ---- Other universes ----
foreach ($u in "unused","historical") {
    Wb @("shortlist","--universe",$u,"--auto","6","--beam","100","--mode","expected")
    if ($Level -eq "Quick") { $src = "shortlist_${u}_expected_b100.json" }
    else {
        Wb @("shortlist","--universe",$u,"--from","shortlist_${u}_expected_b100.json","--top","3","--beam","0","--mode","expected")
        $src = "shortlist_${u}_expected_exact.json"
    }
    if (-not (Has "tree_${u}_expected.json")) {
        Wb @("solve-tree","--universe",$u,"--first","best:$src","--beam",$pb,"--mode","expected","--json","tree_${u}_expected.json")
    }
}

# ---- Validation: replay every saved tree against every answer it covers ----
foreach ($t in "tree_full_expected.json","tree_full_le4.json","tree_full_expected-hard.json") {
    foreach ($a in "full","nyt2309","unused","historical","history-seq") { Wb @("simulate","--tree",$t,"--against",$a) }
}
Wb @("simulate","--tree","tree_unused_expected.json","--against","unused")
Wb @("simulate","--tree","tree_historical_expected.json","--against","historical")

Wb @("report")
Write-Host "All done. Open RESULTS.md" -ForegroundColor Green
