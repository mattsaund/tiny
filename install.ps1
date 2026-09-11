# Install tiny - a terminal knowledge manager. Windows.
#
#   irm https://raw.githubusercontent.com/mattsaund/tiny/main/install.ps1 | iex
#
# Downloads the ready-made tiny.exe from the latest GitHub release. Only when
# there is none to download - no release published yet, or a branch asked for
# by name - does it build from source, which on Windows needs Rust and the
# Visual Studio Build Tools; it offers to install both.
#
# Override anything with the environment:
#   $env:TINY_REPO         git URL (default: https://github.com/mattsaund/tiny.git)
#   $env:TINY_REF          branch or tag to build (default: main, which downloads)
#   $env:TINY_PREFIX       where tiny.exe lands (default: %USERPROFILE%\.local\bin)
#   $env:TINY_FROM_SOURCE  set to anything to build instead of downloading
#
# Three rules this file keeps, each because breaking it broke an install:
#
#   * It is plain ASCII. Windows PowerShell 5.1 reads a local .ps1 in the ANSI
#     code page, and the last byte of a UTF-8 em dash is, in that code page, a
#     curly double quote - which PowerShell takes as the end of a string.
#
#   * It never sets $ErrorActionPreference to Stop. 5.1 turns every line a
#     native program writes to stderr into an error record once the stream is
#     redirected, and under Stop into a terminating one. git says "Cloning
#     into..." on stderr and cargo says everything there, so Stop ended the
#     script on the first line of perfectly ordinary output. Failures are
#     checked where they happen instead: $LASTEXITCODE after every program, and
#     -ErrorAction Stop on the few cmdlets whose failure matters.
#
#   * It never calls exit. Piped through iex, this runs in the caller's own
#     session, and an exit there closes the window the user typed into - along
#     with the message saying what went wrong.

& {
    $ErrorActionPreference = 'Continue'

    # GitHub only speaks TLS 1.2 and up, and Windows PowerShell 5.1 on an older
    # Windows does not offer it unless asked.
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    function Have([string]$Name) {
        [bool](Get-Command $Name -ErrorAction SilentlyContinue)
    }

    # --- the ready-made binary ----------------------------------------------

    # The release workflow's zip for this machine, unpacked into $Prefix.
    # Returns whether it worked. Every way it can fail - no release yet, no
    # network, a repository that is not on GitHub - is a reason to build
    # instead, not a reason to stop.
    function Install-Prebuilt([string]$Repo, [string]$Prefix) {
        $slug = $Repo -replace '^https://github\.com/', '' -replace '\.git$', ''
        if ($slug -match '://' -or $slug -notmatch '^[^/]+/[^/]+$') { return $false }

        # One Windows build, for x64. ARM64 Windows runs it under emulation.
        $asset = 'tiny-x86_64-pc-windows-msvc.zip'
        $url = "https://github.com/$slug/releases/latest/download/$asset"
        $work = Join-Path ([IO.Path]::GetTempPath()) ('tiny-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Force -Path $work | Out-Null
        try {
            Write-Host 'downloading tiny ... ' -NoNewline
            $zip = Join-Path $work $asset
            # The progress bar Invoke-WebRequest draws in 5.1 slows a download
            # down by an order of magnitude.
            $ProgressPreference = 'SilentlyContinue'
            try {
                Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $zip -ErrorAction Stop
            }
            catch {
                Write-Host 'none published yet, so building from source instead'
                return $false
            }
            Expand-Archive -Path $zip -DestinationPath $work -Force -ErrorAction Stop
            $exe = Get-ChildItem -Path $work -Filter 'tiny.exe' -Recurse | Select-Object -First 1
            if (-not $exe) {
                Write-Host 'the download had no tiny.exe in it, so building instead'
                return $false
            }
            New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
            Copy-Item -Path $exe.FullName -Destination (Join-Path $Prefix 'tiny.exe') -Force -ErrorAction Stop
            Write-Host 'done'
            return $true
        }
        finally {
            Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
        }
    }

    # --- building from source -----------------------------------------------

    # Whether the MSVC build tools are installed.
    #
    # Not Get-Command cl: the compiler is never on PATH outside a Visual Studio
    # developer prompt, even when it is installed - which is why that check
    # told everyone they had no compiler. vswhere is how Rust itself finds it.
    function Test-Msvc {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        if (-not (Test-Path $vswhere)) { return $false }
        $found = & $vswhere -latest -products * `
            -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
            -property installationPath 2>$null
        return [bool]$found
    }

    # The Build Tools, offered before Rust rather than after: rustup looks for
    # them as it installs, and finding them already there is what stops it
    # pulling in the whole of Visual Studio on its own account.
    function Confirm-Msvc {
        if (Test-Msvc) { return }
        Write-Host ''
        Write-Host 'Building tiny from source on Windows needs the Visual Studio Build Tools:'
        Write-Host 'the C/C++ compiler and linker Rust uses. They are free, a few gigabytes,'
        Write-Host 'install in the background, and ask for an administrator prompt.'
        $reply = Read-Host 'Install them now? [Y/n]'
        if ($reply -match '^[Nn]') {
            throw 'the Build Tools are needed to build from source: https://visualstudio.microsoft.com/visual-cpp-build-tools/'
        }
        if (-not (Have 'winget')) {
            throw 'winget is not here to install them. Get the Build Tools from https://visualstudio.microsoft.com/visual-cpp-build-tools/ ("Desktop development with C++"), then run this again'
        }
        Write-Host 'installing the Build Tools - this takes a while ...'
        & winget install --id Microsoft.VisualStudio.2022.BuildTools --exact `
            --accept-package-agreements --accept-source-agreements `
            --override '--passive --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
        # winget's exit code is not a reliable answer - it is non-zero for
        # "already installed" - so ask the same question again instead.
        if (-not (Test-Msvc)) {
            throw 'the Build Tools did not install; see the message above, or install them by hand and run this again'
        }
    }

    function Confirm-Cargo {
        if (Have 'cargo') { return }
        $cargoBin = Join-Path $HOME '.cargo\bin'
        if (Test-Path (Join-Path $cargoBin 'cargo.exe')) {
            $env:PATH = "$cargoBin;$env:PATH"
            return
        }
        Write-Host ''
        Write-Host 'Building tiny needs Rust, and cargo is not installed. It goes in'
        Write-Host '~\.rustup and ~\.cargo, and "rustup self uninstall" removes it again.'
        $reply = Read-Host 'Install it now? [Y/n]'
        if ($reply -match '^[Nn]') { throw 'cargo is needed to build from source: https://rustup.rs' }
        $init = Join-Path ([IO.Path]::GetTempPath()) 'rustup-init.exe'
        $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest -UseBasicParsing -Uri 'https://win.rustup.rs/x86_64' -OutFile $init -ErrorAction Stop
        & $init -y --profile minimal --default-toolchain stable
        $code = $LASTEXITCODE
        Remove-Item $init -ErrorAction SilentlyContinue
        if ($code -ne 0) { throw 'rustup failed; the output above says why' }
        $env:PATH = "$cargoBin;$env:PATH"
        if (-not (Have 'cargo')) { throw 'cargo is still not on PATH' }
    }

    # One line that moves: [#####.......]  42%  compiling syntect
    function Write-Bar([int]$Count, [int]$Total, [string]$What) {
        $width = 28
        $label = if ($What.Length -gt 30) { $What.Substring(0, 30) } else { $What.PadRight(30) }
        if ($Total -le 0) {
            Write-Host ("`r  {0} crates  {1}" -f $Count, $label) -NoNewline
            return
        }
        $percent = [Math]::Min(100, [int]($Count * 100 / $Total))
        $filled = [int]($percent * $width / 100)
        $bar = ('#' * $filled) + ('.' * ($width - $filled))
        Write-Host ("`r  [{0}] {1,3}%  {2}" -f $bar, $percent, $label) -NoNewline
    }

    function Build-Tiny([string]$Source, [string]$Prefix, [string]$Log) {
        New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
        Write-Host 'building - this takes a minute the first time'

        # How many crates the bar counts against. Asking cargo rather than
        # writing a number down keeps the bar right when the dependencies
        # change; if the question fails the bar counts instead of filling.
        $total = 0
        $tree = & cargo tree --manifest-path (Join-Path $Source 'Cargo.toml') `
            -e 'normal,build' --prefix none --no-dedupe 2>$null
        if ($LASTEXITCODE -eq 0 -and $tree) {
            $total = @($tree | Where-Object { $_.Trim() } | Sort-Object -Unique).Count
        }

        $moving = -not [Console]::IsOutputRedirected
        $done = 0
        # cargo's --root puts the binary in <root>\bin, so it is given the parent.
        $root = Split-Path -Parent $Prefix

        # Cargo says what it is doing a line at a time, on stderr. Merged into
        # the pipeline, each line arrives here as it is written; the log keeps
        # all of it for the failure case, where what went wrong matters more
        # than how far it got.
        & cargo install --path $Source --bin tiny --root $root --force 2>&1 | ForEach-Object {
            $line = "$_"
            Add-Content -Path $Log -Value $line
            if (-not $moving) { return }
            $trimmed = $line.TrimStart()
            $what = $null
            $count = $done
            if ($trimmed.StartsWith('Compiling ')) {
                $done++
                $name = ($trimmed -split '\s+')[1]
                # tiny is always the last crate, since everything it depends on
                # has to exist first. Its arrival means only the link is left.
                if ($name -eq 'tiny' -and $total -gt 0) {
                    $count = $total - 1
                    $what = 'linking tiny'
                }
                else {
                    $count = $done
                    $what = "compiling $name"
                }
            }
            elseif ($trimmed.StartsWith('Downloaded ')) { $what = 'fetching crates' }
            elseif ($trimmed.StartsWith('Updating ')) { $what = 'updating the crate index' }
            elseif ($trimmed.StartsWith('Finished ')) {
                $count = $total
                $what = 'installing'
            }
            if ($what) { Write-Bar $count $total $what }
        }
        $built = $LASTEXITCODE -eq 0
        if ($moving) { Write-Host '' }
        if (-not $built) {
            Write-Host ''
            if (Test-Path $Log) { Get-Content $Log -Tail 30 | ForEach-Object { Write-Host $_ } }
            throw 'the build failed; the output above says why'
        }
    }

    function Install-FromSource([string]$Repo, [string]$Ref, [string]$Prefix, [bool]$Checkout) {
        Confirm-Msvc
        Confirm-Cargo

        $source = $null
        $temp = $false
        $log = Join-Path ([IO.Path]::GetTempPath()) ('tiny-build-' + [Guid]::NewGuid().ToString('N') + '.log')
        try {
            if ($Checkout) {
                $source = (Get-Location).Path
                Write-Host "building from $source"
            }
            else {
                if (-not (Have 'git')) { throw 'git is needed to fetch the source' }
                $source = Join-Path ([IO.Path]::GetTempPath()) ('tiny-' + [Guid]::NewGuid().ToString('N'))
                $temp = $true
                Write-Host "fetching $Repo ($Ref) ... " -NoNewline
                # Discarded, not merged: git narrates cloning on stderr, and
                # what matters is only whether it worked.
                & git clone --quiet --depth 1 --branch $Ref $Repo $source 2>$null | Out-Null
                if ($LASTEXITCODE -ne 0) {
                    Write-Host ''
                    throw "could not clone $Repo ($Ref)"
                }
                Write-Host 'done'
            }
            Build-Tiny -Source $source -Prefix $Prefix -Log $log
        }
        finally {
            if ($temp -and $source -and (Test-Path $source)) {
                Remove-Item -Recurse -Force $source -ErrorAction SilentlyContinue
            }
            if (Test-Path $log) { Remove-Item -Force $log -ErrorAction SilentlyContinue }
        }
    }

    # --- PATH ---------------------------------------------------------------

    # Put $Prefix on the user's PATH, for good and for this window.
    #
    # The *user* PATH is read and written, not the combined one: writing the
    # combined PATH back would copy every system-wide entry into the user's own.
    function Add-ToPath([string]$Prefix) {
        $user = [Environment]::GetEnvironmentVariable('PATH', 'User')
        if (-not $user) { $user = '' }
        $parts = @($user -split ';' | Where-Object { $_ })
        if ($parts -notcontains $Prefix) {
            [Environment]::SetEnvironmentVariable('PATH', (@($parts + $Prefix) -join ';'), 'User')
            Write-Host "added $Prefix to your PATH, so new terminals will find tiny"
        }
        if (@($env:PATH -split ';') -notcontains $Prefix) {
            $env:PATH = "$env:PATH;$Prefix"
        }
    }

    # --- the whole of it ----------------------------------------------------

    function Install-Tiny {
        $repo = if ($env:TINY_REPO) { $env:TINY_REPO } else { 'https://github.com/mattsaund/tiny.git' }
        $ref = if ($env:TINY_REF) { $env:TINY_REF } else { 'main' }
        $prefix = if ($env:TINY_PREFIX) { $env:TINY_PREFIX } else { Join-Path $HOME '.local\bin' }
        $binary = Join-Path $prefix 'tiny.exe'

        # Windows locks a program while it runs, so a running tiny cannot be
        # replaced. Better said now than half way through.
        if (Get-Process -Name tiny -ErrorAction SilentlyContinue) {
            throw 'tiny is running. Close it, then run this again'
        }

        # Running from inside a checkout means "build this", so a checkout
        # never downloads. Nor does naming a ref: a release is a tag, and
        # a branch today is not what the last tag was.
        $checkout = (Test-Path 'Cargo.toml') -and
            (Select-String -Path 'Cargo.toml' -Pattern 'name = "tiny"' -Quiet)
        $installed = $false
        if (-not $checkout -and -not $env:TINY_FROM_SOURCE -and $ref -eq 'main') {
            $installed = Install-Prebuilt -Repo $repo -Prefix $prefix
        }
        if (-not $installed) {
            Install-FromSource -Repo $repo -Ref $ref -Prefix $prefix -Checkout $checkout
        }
        if (-not (Test-Path $binary)) { throw "expected a binary at $binary" }

        Add-ToPath -Prefix $prefix
        Write-Host ''
        Write-Host "installed $binary"
        Write-Host 'run it with:      tiny ~\notes'
        Write-Host 'remove it with:   tiny --uninstall'
    }

    try {
        Install-Tiny
    }
    catch {
        Write-Host ''
        Write-Host "install: $($_.Exception.Message)" -ForegroundColor Red
        # Not exit, which would close the window. A failed exit code is still
        # there for anything that checks - a CI job, a script.
        $global:LASTEXITCODE = 1
    }
}
