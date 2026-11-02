# rusttyterm PowerShell integration (SHELLINT-014, issue #38).
# Dot-sourced with `pwsh -NoExit -Command ". <this file>"`.
$rtEsc = [char]27
$rtBel = [char]7

function global:Prompt {
    $cwd = $PWD.ProviderPath -replace '\\', '/'
    "$rtEsc]133;D:$LASTEXITCODE$rtBel" +
    "$rtEsc]7;file://${env:COMPUTERNAME}$cwd$rtBel" +
    "$rtEsc]133;A$rtBel" +
    "PS $PWD> "
}

if (Get-Module -ListAvailable PSReadLine) {
    Import-Module PSReadLine -ErrorAction SilentlyContinue
    Set-PSReadLineKeyHandler -Chord Enter -ScriptBlock {
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
        # The command was accepted; its output follows: 133;B + 133;C.
        [Console]::Write("$rtEsc]133;B$rtBel$rtEsc]133;C$rtBel")
    }
}
