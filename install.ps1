# Install tiny — a terminal knowledge manager. Windows.
#
#   irm https://raw.githubusercontent.com/mattsaund/tiny/main/install.ps1 | iex
#
# The same two ways as install.sh: piped from the web, where it fetches the
# source itself, or run from inside a checkout, where it builds what is there.
#
# Override anything with the environment:
#   $env:TINY_REPO    git URL to clone
#   $env:TINY_REF     branch or tag (default: main)
#   $env:TINY_PREFIX  where the binary lands (default: %USERPROFILE%\.local\bin)

$ErrorActionPreference = 'Stop'

# Cargo says everything on stderr — every `Compiling` line included. With
# `Stop` in force, a native command's stderr merged into the pipeline is an
# ErrorRecord that terminates the script, so a perfectly ordinary build would
# die on its first line of output. PowerShell 7.3 added a switch for exactly
# this; older versions never had the problem, and setting a variable they do
# not know is harmless.
$PSNativeCommandUseErrorActionPreference = $false

$repo   = if ($env:TINY_REPO)   { $env:TINY_REPO }   else { 'https://github.com/mattsaund/tiny.git' }
$ref    = if ($env:TINY_REF)    { $env:TINY_REF }    else { 'main' }
$prefix = if ($env:TINY_PREFIX) { $env:TINY_PREFIX } else { Join-Path $HOME '.local\bin' }

function Have($name) { [bool](Get-Command $name -ErrorAction SilentlyContinue) }
function Die($message) { Write-Host "install: $message" -ForegroundColor Red; exit 1 }

# Whether it is worth drawing anything that moves. Redirected to a file or run
# from a CI job, a bar redrawn with carriage returns is one unreadable line.
$moving = -not [Console]::IsOutputRedirected

$source = $null
$sourceIsTemp = $false
$log = $null

try {
    # --- rust ---------------------------------------------------------------

    if (-not (Have 'cargo')) {
        $cargoHome = Join-Path $HOME '.cargo\bin'
        if (Test-Path (Join-Path $cargoHome 'cargo.exe')) {
            $env:PATH = "$cargoHome;$env:PATH"
        }
        else {
            Write-Host 'tiny is written in Rust, and cargo is not installed.'
            Write-Host 'Install the Rust toolchain now? It goes in ~\.rustup and ~\.cargo,'
            Write-Host 'and `rustup self uninstall` removes it again.'
            $reply = Read-Host '  [Y/n]'
            if ($reply -match '^[Nn]') { Die 'cargo is required — see https://rustup.rs' }

            $init = Join-Path ([IO.Path]::GetTempPath()) 'rustup-init.exe'
            Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile $init
            & $init -y --profile minimal --default-toolchain stable
            if ($LASTEXITCODE -ne 0) { Die 'rustup failed' }
            Remove-Item $init -ErrorAction SilentlyContinue
            $env:PATH = "$cargoHome;$env:PATH"
        }
    }
    if (-not (Have 'cargo')) { Die 'cargo still not on PATH' }

    # Building tree-sitter's grammars needs a C compiler. Saying so here beats
    # a screen of linker errors twenty seconds in.
    if (-not (Have 'cl') -and -not (Have 'gcc') -and -not (Have 'clang')) {
        Write-Host 'note: no C compiler found. tiny bundles tree-sitter grammars,' -ForegroundColor Yellow
        Write-Host '      which are C. Install the Visual Studio Build Tools with the' -ForegroundColor Yellow
        Write-Host '      "Desktop development with C++" workload if the build fails.' -ForegroundColor Yellow
    }

    # --- source -------------------------------------------------------------

    $manifest = Join-Path (Get-Location) 'Cargo.toml'
    if ((Test-Path $manifest) -and (Select-String -Path $manifest -Pattern 'name = "tiny"' -Quiet)) {
        $source = (Get-Location).Path
        Write-Host "building from $source"
    }
    else {
        if (-not (Have 'git')) { Die 'git is required to fetch the source' }
        $source = Join-Path ([IO.Path]::GetTempPath()) ("tiny-" + [Guid]::NewGuid().ToString('N'))
        $sourceIsTemp = $true
        Write-Host "fetching $repo ($ref) ... " -NoNewline
        # Discarded rather than merged: what matters is the exit status, and a
        # merged stderr is a stream of ErrorRecords to no purpose.
        & git clone --depth 1 --branch $ref $repo $source 2>$null | Out-Null
        if ($LASTEXITCODE -ne 0) { Write-Host ''; Die "could not clone $repo" }
        Write-Host 'done'
    }

    # --- build --------------------------------------------------------------

    New-Item -ItemType Directory -Force -Path $prefix | Out-Null

    # How many crates the bar counts against. Asking cargo rather than writing
    # a number down means the bar stays right when the dependencies change; an
    # old cargo that does not know the command leaves this at zero, which turns
    # the bar into a count.
    Write-Host 'building — this takes a minute the first time'
    $total = 0
    $tree = & cargo tree --manifest-path (Join-Path $source 'Cargo.toml') `
        -e normal,build --prefix none --no-dedupe 2>$null
    if ($LASTEXITCODE -eq 0 -and $tree) {
        $total = ($tree | Where-Object { $_.Trim() } | Sort-Object -Unique).Count
    }

    $width = 28
    $done = 0
    function Draw($count, $what) {
        if (-not $moving) { return }
        $label = if ($what.Length -gt 30) { $what.Substring(0, 30) } else { $what.PadRight(30) }
        if ($total -le 0) {
            Write-Host ("`r  {0} crates  {1}" -f $count, $label) -NoNewline
            return
        }
        $percent = [Math]::Min(100, [int]($count * 100 / $total))
        $filled = [int]($percent * $width / 100)
        $bar = ('#' * $filled) + ('.' * ($width - $filled))
        Write-Host ("`r  [{0}] {1,3}%  {2}" -f $bar, $percent, $label) -NoNewline
    }

    # Cargo says what it is doing a line at a time, and the log keeps the whole
    # of it for the failure case, where what went wrong matters more than how
    # far it got.
    $log = Join-Path ([IO.Path]::GetTempPath()) ("tiny-build-" + [Guid]::NewGuid().ToString('N') + '.log')
    $root = Split-Path -Parent $prefix

    # Belt as well as braces for the stderr problem above: whatever version of
    # PowerShell this is, an ordinary build line must not end the script.
    $wasStopping = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & cargo install --path $source --bin tiny --root $root --force 2>&1 | ForEach-Object {
        $line = "$_"
        Add-Content -Path $log -Value $line
        $trimmed = $line.TrimStart()
        if ($trimmed.StartsWith('Compiling ')) {
            $done++
            $name = ($trimmed -split '\s+')[1]
            # tiny is always the last crate: everything it depends on has to
            # exist first. So its arrival means the only work left is the link,
            # however few `Compiling` lines came before it.
            if ($name -eq 'tiny' -and $total -gt 0) { Draw ($total - 1) 'linking tiny' }
            else { Draw $done "compiling $name" }
        }
        elseif ($trimmed.StartsWith('Downloaded ')) { Draw $done 'fetching crates' }
        elseif ($trimmed.StartsWith('Updating ')) { Draw $done 'updating the crate index' }
        elseif ($trimmed.StartsWith('Finished ')) { Draw $total 'installing' }
    }
    $built = $LASTEXITCODE -eq 0
    $ErrorActionPreference = $wasStopping
    if ($moving) { Write-Host '' }

    if (-not $built) {
        Write-Host ''
        if (Test-Path $log) { Get-Content $log -Tail 30 | Write-Host }
        Die 'build failed — the output above says why'
    }

    $binary = Join-Path $prefix 'tiny.exe'
    if (-not (Test-Path $binary)) { Die "expected a binary at $binary" }

    # --- PATH ---------------------------------------------------------------

    Write-Host ''
    Write-Host "installed $binary"
    $onPath = ($env:PATH -split ';') -contains $prefix
    if ($onPath) {
        Write-Host 'run it with:      tiny ~\notes'
        Write-Host 'remove it with:   tiny --uninstall'
    }
    else {
        Write-Host "$prefix is not on your PATH. Add it for good with:"
        Write-Host ''
        Write-Host "  [Environment]::SetEnvironmentVariable('PATH', `"`$env:PATH;$prefix`", 'User')"
        Write-Host ''
        Write-Host 'then open a new terminal and run:  tiny ~\notes'
        Write-Host ''
        Write-Host 'to remove tiny later:  tiny --uninstall'
    }
}
finally {
    if ($sourceIsTemp -and $source -and (Test-Path $source)) {
        Remove-Item -Recurse -Force $source -ErrorAction SilentlyContinue
    }
    if ($log -and (Test-Path $log)) {
        Remove-Item -Force $log -ErrorAction SilentlyContinue
    }
}
