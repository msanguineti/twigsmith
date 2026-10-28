# Bash completion for twigsmith and tgi. Source this file in an interactive
# Bash shell; it does not depend on the bash-completion package.

# Read shell words without evaluating shell input. Readline splits words at
# characters such as '=' and '@', which are valid inside a Git branch name.
# Retain the whole literal word for matching and its replacement suffix for
# Readline. The caller owns these dynamically scoped locals.
_twigsmith_words() {
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

_twigsmith_add() {
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

_twigsmith() {
    local -a words=()
    local LC_ALL=C
    local cur quote suffix candidate executable i
    COMPREPLY=()
    # Readline replaces text before the cursor and retains the token suffix.
    # Decline a completion inside a token instead of changing its literal value.
    local following=${COMP_LINE:COMP_POINT:1}
    case $following in
        ''|' '|$'\t'|$'\n') ;;
        *) return 0 ;;
    esac
    _twigsmith_words
    # Ignore assignments or wrappers before Bash's completed executable.
    for ((i=0; i<${#words[@]}-1; i++)); do
        if [[ ${words[i]} == "$1" ]]; then words=("${words[@]:i}"); break; fi
    done
    executable=$(type -P twigsmith) || executable=$(type -P tgi) || return 0
    while IFS= read -r candidate; do
        _twigsmith_add "$candidate"
    done < <(command "$executable" __complete --shell bash --index "$((${#words[@]}-1))" -- "${words[@]}" 2>/dev/null)
    return 0
}

complete -o noquote -F _twigsmith twigsmith tgi
