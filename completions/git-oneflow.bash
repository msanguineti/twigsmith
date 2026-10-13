# Bash completion for git-oneflow and gof. Source this file in an interactive
# Bash shell; it does not depend on the bash-completion package.

# Read shell words without evaluating shell input. Readline splits words at
# characters such as '=' and '@', which are valid inside a Git branch name.
# Retain the whole literal word for matching and its replacement suffix for
# Readline. The caller owns these dynamically scoped locals.
_git_oneflow_words() {
    local line=${COMP_LINE:0:COMP_POINT} char next word='' active=0 i
    words=() quote='' suffix=0
    for ((i=0; i<${#line}; i++)); do
        char=${line:i:1}
        if [[ $quote == "'" ]]; then
            if [[ $char == "'" ]]; then quote=''; else word+=$char; fi
        elif [[ $quote == '"' ]]; then
            case $char in
                '"') quote='' ;;
                '\')
                    next=${line:i+1:1}
                    if [[ $next == '$' || $next == '`' || $next == '"' || $next == '\' ]]; then
                        word+=$next; ((i+=1))
                    else word+=$char; fi ;;
                *) word+=$char ;;
            esac
        else
            case $char in
                "'"|'"') quote=$char; active=1 ;;
                '\') word+=${line:i+1:1}; ((i+=1)); active=1 ;;
                ' '|$'\t'|$'\n')
                    if ((active)); then words+=("$word"); fi
                    word=''; active=0; suffix=0 ;;
                ';'|'&'|'|'|'('|')'|'<'|'>')
                    words=(); word=''; active=0; suffix=0 ;;
                *)
                    word+=$char; active=1
                    if [[ $COMP_WORDBREAKS == *"$char"* ]]; then
                        suffix=${#word}
                        # Readline keeps '@' in its replacement fragment.
                        [[ $char != '@' ]] || ((suffix-=1))
                    fi ;;
            esac
        fi
    done
    words+=("$word")
    cur=$word
}

_git_oneflow_add() {
    local candidate=$1 reply char encoded='' i
    [[ $candidate == "$cur"* ]] || return 0
    # Only replace the fragment Readline is completing, not the prefix before a
    # word break. Quote for the active shell quote context; never eval ref names.
    reply=${candidate:suffix}
    case $quote in
        "'") reply=${reply//\'/\'\\\'\'} ;;
        '"')
            reply=${reply//\\/\\\\}; reply=${reply//\$/\\\$}
            reply=${reply//\`/\\\`}; reply=${reply//\"/\\\"}
            # History expansion is active inside double quotes. Put each '!' in
            # an escaped unquoted fragment, then resume the double quote.
            reply=${reply//!/\"\\!\"} ;;
        *)
            for ((i=0; i<${#reply}; i++)); do
                char=${reply:i:1}
                case $char in
                    ' '|'\'|"'"|'"'|'$'|'`'|'!'|'&'|';'|'|'|'<'|'>'|'('|')'|'{'|'}'|'['|']'|'*'|'?'|'~'|'#') encoded+='\' ;;
                esac
                encoded+=$char
            done
            reply=$encoded ;;
    esac
    local previous
    for previous in "${COMPREPLY[@]}"; do [[ $previous != "$reply" ]] || return 0; done
    COMPREPLY+=("$reply")
}

_git_oneflow_git() {
    GIT_TERMINAL_PROMPT=0 LC_ALL=C command git --no-optional-locks --no-lazy-fetch \
        -c core.fsmonitor=false "$@" 2>/dev/null
}

_git_oneflow_refs() {
    local kind=$1 ref symbolic name mapping source destination capture key entry
    local -a keys=() mappings=()
    # Invert configured fetch mappings instead of guessing a remote's name from
    # a tracking-ref path. Custom destinations and remote names remain literal.
    if [[ $kind == branches ]]; then
        while IFS= read -r -d '' entry; do
            keys+=("${entry%%$'\n'*}")
            mappings+=("${entry#*$'\n'}")
        done < <(_git_oneflow_git config --null --get-regexp '^remote\..*\.fetch$')
    fi
    while IFS=$'\t' read -r ref symbolic; do
        [[ -z $symbolic ]] || continue
        case $kind:$ref in
            branches:refs/heads/*) _git_oneflow_add "${ref#refs/heads/}" ;;
            tags:refs/tags/*) _git_oneflow_add "${ref#refs/tags/}" ;;
            sources:refs/heads/*) _git_oneflow_add "${ref#refs/heads/}" ;;
            sources:refs/tags/*) _git_oneflow_add "${ref#refs/tags/}" ;;
            sources:refs/remotes/*) _git_oneflow_add "${ref#refs/remotes/}" ;;
            branches:refs/remotes/*)
                local i j excluded
                for ((i=0; i<${#mappings[@]}; i++)); do
                    mapping=${mappings[i]#+}
                    [[ $mapping != '^'* && $mapping == refs/heads/*:* ]] || continue
                    source=${mapping%%:*}; destination=${mapping#*:}
                    capture=''
                    if [[ $destination == *'*'* ]]; then
                        local before=${destination%%\**} after=${destination#*\*}
                        [[ $ref == "$before"*"$after" && ${#ref} -ge $((${#before}+${#after})) ]] || continue
                        capture=${ref:${#before}:${#ref}-${#before}-${#after}}
                        name=${source/\*/"$capture"}
                    else
                        [[ $ref == "$destination" && $source != *'*'* ]] || continue
                        name=$source
                    fi
                    excluded=0
                    for ((j=0; j<${#mappings[@]}; j++)); do
                        [[ ${keys[j]} == "${keys[i]}" && ${mappings[j]} == '^'* ]] || continue
                        local negative=${mappings[j]#^}
                        if [[ $negative == *'*'* ]]; then
                            [[ $name != "${negative%%\**}"*"${negative#*\*}" ]] || excluded=1
                        else [[ $name != "$negative" ]] || excluded=1; fi
                    done
                    ((excluded)) || _git_oneflow_add "${name#refs/heads/}"
                done ;;
        esac
    done < <(_git_oneflow_git for-each-ref --format='%(refname)%09%(symref)' refs/heads refs/remotes refs/tags)
}

_git_oneflow() {
    local -a words=()
    local LC_ALL=C
    local cur quote suffix command pending='' positional=0 word option i
    COMPREPLY=()
    _git_oneflow_words
    # Words before the executable (for example an environment assignment) do not
    # participate in its grammar. Bash supplies the completed executable as $1.
    for ((i=0; i<${#words[@]}-1; i++)); do
        if [[ ${words[i]} == "$1" ]]; then words=("${words[@]:i}"); break; fi
    done
    if ((${#words[@]} == 2)); then
        for word in start s save v finish f status config -h --help -V --version; do _git_oneflow_add "$word"; done
        return 0
    fi
    command=${words[1]-}
    case $command in
        start|s) option='-h --help --policy --from --from-tag --target' ;;
        save|v) option='-h --help --publish-only --staged --wip --stash --message' ;;
        finish|f) option='-h --help --mode --integration --checkout --cleanup -m --message --merge-back --tag --tag-message --sign-tag --update --history-edit --allow-rewrite --no-rewrite --skip-checks --show-credentials' ;;
        config) option='-h --help -y --yes --oneflow --oneflow-hosted --marker --format' ;;
        status) option='-h --help' ;;
        *) return 0 ;;
    esac
    for ((i=2; i<${#words[@]}-1; i++)); do
        word=${words[i]}
        if [[ -n $pending ]]; then pending=''; continue; fi
        case $command:$word in
            start:--policy|s:--policy|start:--from|s:--from|start:--from-tag|s:--from-tag|start:--target|s:--target|save:--message|v:--message|config:--format|config:--marker|finish:--mode|f:--mode|finish:--integration|f:--integration|finish:--checkout|f:--checkout|finish:--cleanup|f:--cleanup|finish:-m|f:-m|finish:--message|f:--message|finish:--merge-back|f:--merge-back|finish:--tag|f:--tag|finish:--tag-message|f:--tag-message|finish:--update|f:--update|finish:--history-edit|f:--history-edit)
                pending=$word ;;
            *:--help|*:-h|config:show) return 0 ;;
            start:*|s:*) [[ $word == -* ]] || positional=1 ;;
        esac
    done
    case $command:$pending in
        start:--policy|s:--policy) option='ordinary release hotfix' ;;
        start:--from|s:--from) _git_oneflow_refs sources; return 0 ;;
        start:--from-tag|s:--from-tag) _git_oneflow_refs tags; return 0 ;;
        start:--target|s:--target|finish:--merge-back|f:--merge-back|config:--marker)
            _git_oneflow_refs branches; return 0 ;;
        config:--format) option='toml yaml json' ;;
        finish:--mode|f:--mode) option='review integrate' ;;
        finish:--integration|f:--integration) option='ff-only merge squash' ;;
        finish:--checkout|f:--checkout) option='target work' ;;
        finish:--cleanup|f:--cleanup) option='keep local both' ;;
        finish:--update|f:--update) option='rebase merge none' ;;
        finish:--history-edit|f:--history-edit) option='none interactive' ;;
        *:--message|*:-m|*:--tag|*:--tag-message) return 0 ;;
        *)
            if [[ $command == start || $command == s ]] && (( !positional )) && [[ $cur != -* ]]; then
                _git_oneflow_refs branches
                [[ -z $cur ]] || return 0
            fi
            if [[ $command == config && ${#words[@]} == 3 ]]; then _git_oneflow_add show; fi ;;
    esac
    for word in $option; do _git_oneflow_add "$word"; done
    return 0
}

complete -o noquote -F _git_oneflow git-oneflow gof
