//! Shell adapters transport literal words to the common completion engine.
//! Input and candidates are data: shell evaluation is reserved for installing
//! the trusted script, never for reconstructing the command being completed.

pub fn script(shell: &str) -> Result<String, String> {
    match shell {
        "bash" => Ok(include_str!("../completions/git-oneflow.bash").to_owned()),
        "zsh" => Ok(ZSH.to_owned()),
        "fish" => Ok(FISH.to_owned()),
        "powershell" => Ok(POWERSHELL.to_owned()),
        _ => Err(format!("unsupported completion shell: {shell}")),
    }
}

const ZSH: &str = r#"# git-oneflow and gof completion
# Register after the user has initialized Zsh's normal completion system.
if (( $+functions[compdef] )); then
_git_oneflow() {
    local executable candidate
    local -a literal candidates
    executable=$(whence -p git-oneflow) || executable=$(whence -p gof) || return 0
    [[ -n $executable ]] || return 0
    # (Q) removes shell quoting without evaluating substitutions or parameters.
    literal=("${(@Q)words[1,CURRENT]}")
    literal[CURRENT]=${(Q)PREFIX}
    candidates=("${(@f)$(command "$executable" __complete --shell zsh --index "$((CURRENT-1))" -- "${literal[@]}" 2>/dev/null)}")
    (( ${#candidates} )) && compadd -- "${candidates[@]}"
    return 0
}
compdef _git_oneflow git-oneflow gof
fi
"#;

const FISH: &str = r#"# git-oneflow and gof completion
# Decode quoting as literal text. commandline -x expands variables and globs;
# this scanner intentionally performs neither expansion nor substitution.
function __git_oneflow_complete
    set -l line (commandline -pc)
    set -l words
    set -l word ''
    set -l quote ''
    set -l active 0
    set -l i 1
    set -l length (string length -- "$line")
    while test $i -le $length
        set -l char (string sub -s $i -l 1 -- "$line")
        if test "$quote" = "'"
            if test "$char" = "'"
                set quote ''
            else if test "$char" = '\\'
                set -l next (string sub -s (math $i + 1) -l 1 -- "$line")
                if contains -- "$next" "'" '\\'
                    set word "$word$next"
                    set i (math $i + 1)
                else
                    set word "$word$char"
                end
            else
                set word "$word$char"
            end
        else if test "$quote" = '"'
            if test "$char" = '"'
                set quote ''
            else if test "$char" = '\\'
                set -l next (string sub -s (math $i + 1) -l 1 -- "$line")
                if contains -- "$next" '$' '"' '\\'
                    set word "$word$next"
                    set i (math $i + 1)
                else
                    set word "$word$char"
                end
            else
                set word "$word$char"
            end
        else
            switch "$char"
                case "'" '"'
                    set quote "$char"
                    set active 1
                case '\\'
                    set -l next (string sub -s (math $i + 1) -l 1 -- "$line")
                    set word "$word$next"
                    set i (math $i + 1)
                    set active 1
                case ' ' \t \n
                    if test $active = 1
                        set -a words "$word"
                    end
                    set word ''
                    set active 0
                case '*'
                    set word "$word$char"
                    set active 1
            end
        end
        set i (math $i + 1)
    end
    set -a words "$word"
    set -l executable (command -s git-oneflow)
    if test -z "$executable"
        set executable (command -s gof)
    end
    test -n "$executable"; or return 0
    command "$executable" __complete --shell fish --index (math (count $words) - 1) -- $words 2>/dev/null
end
complete -c git-oneflow -f -a '(__git_oneflow_complete)'
complete -c gof -f -a '(__git_oneflow_complete)'
"#;

const POWERSHELL: &str = r#"# git-oneflow and gof completion (PowerShell 7.3 or newer)
if ($PSVersionTable.PSVersion -ge [version]'7.3') {
Register-ArgumentCompleter -Native -CommandName git-oneflow,gof,git-oneflow.exe,gof.exe -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    # Local scope preserves the user's preference while retaining empty words
    # and embedded quotes in the backend's literal native argument vector.
    $PSNativeCommandArgumentPassing = 'Standard'
    $executable = Get-Command git-oneflow -CommandType Application -ErrorAction Ignore | Select-Object -First 1
    if (-not $executable) {
        $executable = Get-Command gof -CommandType Application -ErrorAction Ignore | Select-Object -First 1
    }
    if (-not $executable) { return }
    # Parse only the prefix at the cursor. AST values are decoded literal tokens;
    # evaluating input here would run substitutions and other incomplete code.
    $length = [Math]::Max(0, [Math]::Min($commandAst.Extent.Text.Length, $cursorPosition - $commandAst.Extent.StartOffset))
    $prefix = $commandAst.Extent.Text.Substring(0, $length)
    $tokens = $null; $parseErrors = $null
    $parsed = [System.Management.Automation.Language.Parser]::ParseInput($prefix, [ref]$tokens, [ref]$parseErrors)
    $pipeline = $parsed.EndBlock.Statements | Select-Object -First 1
    if (-not ($pipeline -is [System.Management.Automation.Language.PipelineAst])) { return }
    $command = $pipeline.PipelineElements | Select-Object -First 1
    if (-not ($command -is [System.Management.Automation.Language.CommandAst])) { return }
    $words = [System.Collections.Generic.List[string]]::new()
    foreach ($element in $command.CommandElements) {
        if ($element -is [System.Management.Automation.Language.StringConstantExpressionAst] -or
            $element -is [System.Management.Automation.Language.ExpandableStringExpressionAst]) {
            $words.Add($element.Value)
        } elseif ($element -is [System.Management.Automation.Language.CommandParameterAst]) {
            $words.Add($element.Extent.Text)
        } else {
            # Preserve expressions as text, never resolve their runtime values.
            $words.Add($element.Extent.Text)
        }
    }
    if ($words.Count -eq 0) { return }
    if ($cursorPosition -gt $commandAst.Extent.EndOffset -or $command.CommandElements[-1].Extent.EndOffset -lt $prefix.Length) { $words.Add('') }
    $query = @('__complete', '--shell', 'powershell', '--index', [string]($words.Count - 1), '--') + $words.ToArray()
    & $executable.Source @query 2>$null | ForEach-Object {
        $candidate = [string]$_
        # PowerShell single-quoted literals escape a quote by doubling it.
        $insertion = "'" + $candidate.Replace("'", "''") + "'"
        [System.Management.Automation.CompletionResult]::new($insertion, $candidate, 'ParameterValue', $candidate)
    }
}
# PowerShell falls back to filenames when a native completer returns no values.
# Wrap the host entry point, preserving its parameter metadata and implementation,
# and remove only that fallback for literal git-oneflow/gof argument contexts.
# TabExpansion2 overrides are a documented, unsupported host customization.
$__gitOneflowPriorCommand = Get-Command TabExpansion2 -CommandType Function -ErrorAction Ignore
if ($__gitOneflowPriorCommand -and -not $__gitOneflowPriorCommand.ScriptBlock.PSObject.Properties['__gitOneflowProviderFilter']) {
    $__gitOneflowPriorBlock = $__gitOneflowPriorCommand.ScriptBlock
    $__gitOneflowMetadata = [System.Management.Automation.CommandMetadata]::new($__gitOneflowPriorCommand)
    $__gitOneflowBinding = [System.Management.Automation.ProxyCommand]::GetCmdletBindingAttribute($__gitOneflowMetadata)
    $__gitOneflowParameters = [System.Management.Automation.ProxyCommand]::GetParamBlock($__gitOneflowMetadata)
    $__gitOneflowOutputNames = @($__gitOneflowPriorCommand.OutputType | ForEach-Object { "'" + $_.Name.Replace("'", "''") + "'" })
    $__gitOneflowOutputAttribute = if ($__gitOneflowOutputNames.Count) { '[OutputType(' + ($__gitOneflowOutputNames -join ',') + ')]' } else { '' }
    $__gitOneflowBody = @'
    $delegateArguments = @{} + $PSBoundParameters
    & $__gitOneflowPriorBlock @delegateArguments | ForEach-Object {
        $completion = $_
        if (-not ($completion -is [System.Management.Automation.CommandCompletion])) {
            $completion
            return
        }
        $tree = $null; $source = $null; $cursor = 0
        if ($delegateArguments.ContainsKey('inputScript')) {
            $source = [string]$delegateArguments['inputScript']
            $cursor = if ($delegateArguments.ContainsKey('cursorColumn')) { [int]$delegateArguments['cursorColumn'] } else { $source.Length }
            $parseTokens = $null; $parseErrors = $null
            $tree = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$parseTokens, [ref]$parseErrors)
        } elseif ($delegateArguments.ContainsKey('ast') -and $delegateArguments.ContainsKey('positionOfCursor')) {
            $tree = $delegateArguments['ast']
            $source = $tree.Extent.Text
            $cursor = $delegateArguments['positionOfCursor'].Offset
        }
        $targetArgument = $false
        if ($tree -and $cursor -ge $tree.Extent.StartOffset -and $cursor -le $tree.Extent.EndOffset) {
            $commands = $tree.FindAll({ param($node) $node -is [System.Management.Automation.Language.CommandAst] }, $true) |
                Sort-Object { $_.Extent.StartOffset } -Descending
            foreach ($command in $commands) {
                if ($command.Extent.StartOffset -gt $cursor) { continue }
                if ($command.Extent.EndOffset -lt $cursor) {
                    $gap = $source.Substring($command.Extent.EndOffset - $tree.Extent.StartOffset, $cursor - $command.Extent.EndOffset)
                    if ($gap -notmatch '^(?:[ \t]|`(?:\r?\n))*$') { continue }
                }
                $name = $command.GetCommandName()
                if ($name) {
                    $name = [System.IO.Path]::GetFileName($name)
                    $targetArgument = ($name -iin @('gof', 'git-oneflow', 'gof.exe', 'git-oneflow.exe')) -and
                        $cursor -gt $command.CommandElements[0].Extent.EndOffset
                }
                if ($targetArgument) {
                    foreach ($redirection in $command.Redirections) {
                        if (-not ($redirection -is [System.Management.Automation.Language.FileRedirectionAst])) { continue }
                        $inTarget = $cursor -ge $redirection.Extent.StartOffset -and $cursor -le $redirection.Extent.EndOffset
                        if (-not $inTarget -and $cursor -gt $redirection.Extent.EndOffset -and
                            $redirection.Location -is [System.Management.Automation.Language.ErrorExpressionAst]) {
                            # An unfinished '> ' has an error location at the operator,
                            # with trailing whitespace outside its AST extent.
                            $gap = $source.Substring($redirection.Extent.EndOffset - $tree.Extent.StartOffset, $cursor - $redirection.Extent.EndOffset)
                            $inTarget = $gap -match '^(?:[ \t]|`(?:\r?\n))*$'
                        }
                        if ($inTarget) { $targetArgument = $false; break }
                    }
                }
                # The innermost active command owns completion at this cursor.
                break
            }
        }
        if (-not $targetArgument) {
            $completion
            return
        }
        $matches = [System.Collections.ObjectModel.Collection[System.Management.Automation.CompletionResult]]::new()
        foreach ($match in $completion.CompletionMatches) {
            if ($match.ResultType -ne 'ProviderItem' -and $match.ResultType -ne 'ProviderContainer') { $matches.Add($match) }
        }
        if ($matches.Count -eq $completion.CompletionMatches.Count) {
            $completion
        } else {
            [System.Management.Automation.CommandCompletion]::new($matches, -1, $completion.ReplacementIndex, $completion.ReplacementLength)
        }
    }
'@
    # The generated text consists only of trusted adapter code and metadata for
    # the existing function. Command input is passed as data to the saved delegate.
    $__gitOneflowWrapper = [scriptblock]::Create($__gitOneflowBinding + "`n" + $__gitOneflowOutputAttribute + "`nparam(" + $__gitOneflowParameters + ")`n" + $__gitOneflowBody).GetNewClosure()
    $__gitOneflowWrapper | Add-Member -NotePropertyName __gitOneflowProviderFilter -NotePropertyValue $true
    Set-Item -Path Function:global:TabExpansion2 -Value $__gitOneflowWrapper
}

}
"#;
