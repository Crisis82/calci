//! Command-line arguments and shell completion scripts.

use clap::{Parser, ValueEnum, ValueHint};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, Parser)]
#[command(
    name = "calci",
    version,
    about = "TUI markdown pager with search, syntax highlighting, links, and math rendering",
    disable_version_flag = true
)]
pub(crate) struct Cli {
    /// Markdown file path. Uses dashboard if omitted.
    #[arg(value_hint = ValueHint::FilePath)]
    pub(crate) file: Option<PathBuf>,

    /// Use a specific color palette file
    #[arg(long = "color", value_name = "FILE", value_hint = ValueHint::FilePath)]
    pub(crate) color: Option<PathBuf>,

    /// Disable pager UI (prints rendered plain text)
    #[arg(short = 'p', long = "plain")]
    pub(crate) plain: bool,

    /// Print shell completion script (bash|zsh|fish)
    #[arg(short = 'c', long, value_name = "SHELL")]
    pub(crate) completion: Option<CompletionShell>,

    /// Print version
    #[arg(short = 'v', long = "version")]
    pub(crate) version: bool,
}

pub(crate) fn print_completion(shell: CompletionShell) {
    match shell {
        CompletionShell::Bash => {
            print!("{}", bash_completion_script());
        }
        CompletionShell::Zsh => {
            print!("{}", zsh_completion_script());
        }
        CompletionShell::Fish => {
            print!("{}", fish_completion_script());
        }
    }
}

fn bash_completion_script() -> &'static str {
    r#"_calci__md_entries() {
    local base
    base="${1:-.}"
    [ -d "$base" ] || return 0
    find "$base" -mindepth 1 -maxdepth 1 \
        \( -type f \( -iname '*.md' -o -iname '*.markdown' \) -o -type d \) \
        -printf '%P\n' 2>/dev/null
}

_calci__md_paths() {
    local cur dir_part prefix search_base
    cur="$1"
    dir_part="${cur%/*}"
    if [ "$dir_part" = "$cur" ]; then
        dir_part=""
        prefix=""
        search_base="."
    else
        prefix="$dir_part/"
        search_base="$dir_part"
    fi
    _calci__md_entries "$search_base" | while IFS= read -r item; do
        [ -z "$item" ] && continue
        if [ -d "${search_base%/}/$item" ]; then
            if find "${search_base%/}/$item" -type f \
                \( -iname '*.md' -o -iname '*.markdown' \) \
                -print -quit 2>/dev/null | grep -q .; then
                printf '%s%s/\n' "$prefix" "$item"
            fi
        else
            printf '%s%s\n' "$prefix" "$item"
        fi
    done
}

_calci() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts="-h -v -p -c --help --version --plain --completion --color"

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"; do
        case "${cmd},${i}" in
            ",$1")
                cmd="calci"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        calci)
            if [[ ${cur} == -* ]]; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --completion|-c)
                    COMPREPLY=($(compgen -W "bash zsh fish" -- "${cur}"))
                    return 0
                    ;;
                --color)
                    COMPREPLY=($(compgen -f -- "${cur}"))
                    return 0
                    ;;
                *)
                    ;;
            esac

            local oldifs
            if [ -n "${IFS+x}" ]; then
                oldifs="$IFS"
            fi
            IFS=$'\n'
            COMPREPLY=($(compgen -W "$(_calci__md_paths "${cur}")" -- "${cur}"))
            if [ -n "${oldifs+x}" ]; then
                IFS="$oldifs"
            fi
            if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                compopt -o filenames
            fi
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _calci -o nosort calci
else
    complete -F _calci calci
fi
"#
}

fn zsh_completion_script() -> &'static str {
    r#"#compdef calci

_calci_md_paths() {
  local -a files dirs
  local cur_dir item
  cur_dir="${PREFIX%/*}"
  if [[ "$PREFIX" == */* ]]; then
    cur_dir="${PREFIX%/*}"
  else
    cur_dir="."
  fi
  local base_prefix=""
  if [[ "$PREFIX" == */* ]]; then
    base_prefix="${PREFIX%/*}/"
  fi
  if [[ -d "$cur_dir" ]]; then
    for item in "$cur_dir"/*(N); do
      if [[ -f "$item" ]]; then
        if [[ "$item" == *.md || "$item" == *.markdown || "$item" == *.MD || "$item" == *.MARKDOWN ]]; then
          files+=("${base_prefix}${item:t}")
        fi
      elif [[ -d "$item" ]]; then
        if [[ -n "$(find "$item" -type f \( -iname '*.md' -o -iname '*.markdown' \) -print -quit 2>/dev/null)" ]]; then
          dirs+=("${base_prefix}${item:t}/")
        fi
      fi
    done
  fi
  (( ${#dirs[@]} )) && compadd -Q -S '' -- "${dirs[@]}"
  (( ${#files[@]} )) && compadd -Q -- "${files[@]}"
  (( ${#dirs[@]} + ${#files[@]} > 0 ))
}

_calci() {
  _arguments -s -S \
    '-h[Print help]' \
    '--help[Print help]' \
    '-v[Print version]' \
    '--version[Print version]' \
    '--color[Use palette file path]:file:_files' \
    '-p[Disable pager UI (prints rendered plain text)]' \
    '--plain[Disable pager UI (prints rendered plain text)]' \
    '-c[Print shell completion script (bash|zsh|fish)]:shell:(bash zsh fish)' \
    '--completion[Print shell completion script (bash|zsh|fish)]:shell:(bash zsh fish)' \
    '::file: _calci_md_paths' && return 0

  case "$state" in
    *)
      return 1
      ;;
  esac
}

if [[ "$funcstack[1]" == "_calci" ]]; then
  _calci "$@"
else
  compdef _calci calci
fi
"#
}

fn fish_completion_script() -> &'static str {
    r#"function __calci_md_paths
    set -l token (commandline -ct)
    set -l dir .
    set -l prefix ""
    if string match -q "*/*" -- $token
        set dir (string replace -r '/[^/]*$' '' -- $token)
        if test -z "$dir"
            set dir "."
        end
        set prefix "$dir/"
    end
    if test -d "$dir"
        for p in $dir/*
            if test -f "$p"
                if string match -qr '\.(md|markdown)$' -- (string lower -- "$p")
                    echo "$prefix"(basename "$p")
                end
            else if test -d "$p"
                if test -n (find "$p" -type f \( -iname '*.md' -o -iname '*.markdown' \) -print -quit 2>/dev/null)
                    echo "$prefix"(basename "$p")/
                end
            end
        end
    end
end

complete -c calci -s c -l completion -d 'Print shell completion script (bash|zsh|fish)' -r -f -a "bash zsh fish"
complete -c calci -l color -d 'Use palette file path' -r -f -a '(__fish_complete_path)'
complete -c calci -s p -l plain -d 'Disable pager UI (prints rendered plain text)'
complete -c calci -s h -l help -d 'Print help'
complete -c calci -s v -l version -d 'Print version'
complete -c calci -f -a '(__calci_md_paths)' -k
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_generation_runs() {
        assert!(bash_completion_script().contains("_calci"));
        assert!(zsh_completion_script().contains("#compdef calci"));
        assert!(fish_completion_script().contains("complete -c calci"));
        assert!(bash_completion_script().contains("opts=\"-h -v -p -c"));
        assert!(bash_completion_script().contains("--color"));
        assert!(zsh_completion_script().contains("--color[Use palette file path]"));
        assert!(fish_completion_script().contains("complete -c calci -l color"));
        assert!(zsh_completion_script().contains("-p[Disable pager UI"));
        assert!(zsh_completion_script().contains("compadd -Q -S '' --"));
        assert!(fish_completion_script().contains("complete -c calci -s p -l plain"));
    }

    #[test]
    fn completion_shell_limits() {
        assert_eq!(CompletionShell::value_variants().len(), 3);
    }
}
