#!/usr/bin/env bash
# Rejects private names on added lines, new paths and commit messages. Hits are
# reported by location only, so the private term list never reaches a log.
#
# Pattern file (--patterns, $CCTUI_FORBIDDEN_TERMS_FILE, or
# ~/.config/cctui/forbidden-terms.txt), one entry per line:
#   # comment / blank      ignored
#   re:<perl regex>        case-insensitive unless the regex says (?-i)
#   lit:<text> or <text>   literal, case-insensitive
# A missing pattern file is a notice, not a failure.
#
#   --staged               staged diff
#   --message <file>       commit message (lines after the scissors are ignored)
#   --range <a>..<b>       every commit in b not in a: its diff and its message
#   --pre-push             refs read from git's pre-push stdin
#   --build <out> <terms> [<regexes>]
#                          write a pattern file from a plain term list and a
#                          plain regex list
set -euo pipefail

pattern_file="${CCTUI_FORBIDDEN_TERMS_FILE:-$HOME/.config/cctui/forbidden-terms.txt}"

usage() {
  echo "usage: $0 [--patterns <file>] --staged | --message <file> | --range <a>..<b> | --pre-push" >&2
  echo "       $0 --build <out> <terms> [<regexes>]" >&2
  exit 2
}

# Prints "location" for every "location<TAB>text" record whose text matches.
# Exit 1 on a hit, 2 on an invalid pattern. Never prints a pattern or the text.
# shellcheck disable=SC2016
matcher='
  $SIG{__WARN__} = sub {};
  my ($file) = @ARGV;
  open my $fh, "<:raw", $file or do { print STDERR "forbidden-terms: cannot read the pattern file\n"; exit 2 };
  my @res;
  my $n = 0;
  while (my $p = <$fh>) {
    $n++;
    $p =~ s/\r?\n\z//;
    next if $p =~ /^\s*(#|$)/;
    if ($p =~ s/^re://) {
      my $re = eval { qr/(?i)$p/ };
      unless ($re) { print STDERR "forbidden-terms: pattern on line $n of the pattern file is invalid\n"; exit 2 }
      push @res, $re;
    } else {
      $p =~ s/^lit://;
      push @res, qr/\Q$p\E/i;
    }
  }
  binmode STDIN, ":raw";
  my $hits = 0;
  while (my $rec = <STDIN>) {
    chomp $rec;
    my ($loc, $text) = split /\t/, $rec, 2;
    $text = "" unless defined $text;
    next unless grep { $text =~ $_ } @res;
    $loc = "(location withheld: its path is itself forbidden)" if grep { $loc =~ $_ } @res;
    print "$loc\n";
    $hits++;
  }
  exit($hits ? 1 : 0);
'

# Unified or combined (merge) diff on stdin -> "<prefix>path:line<TAB>added text".
added_lines() {
  awk -v pre="$1" '
    /^diff --git / { hunk = 0; np = 1; next }
    /^diff --cc / { hunk = 0; next }
    !hunk && /^\+\+\+ / { file = substr($0, 7); next }
    /^@@/ {
      hunk = 1; np = length($1) - 1
      split($(np + 2), a, ","); line = substr(a[1], 2) + 0; next
    }
    hunk {
      marks = substr($0, 1, np)
      if (marks ~ /-/ || marks ~ /\\/) next
      if (marks ~ /\+/) printf "%s%s:%d\t%s\n", pre, file, line, substr($0, np + 1)
      line++
    }
  '
}

paths() { awk -v pre="$1" '{ printf "%s%s\t%s\n", pre, $0, $0 }'; }

# Commit message on stdin. strip=1 drops git comment lines and the verbose diff.
message_lines() {
  awk -v pre="$1" -v strip="$2" '
    strip && /^# -+ >8 -+$/ { exit }
    strip && /^#/ { next }
    { printf "%scommit-message:%d\t%s\n", pre, NR, $0 }
  '
}

git_q() { git -c core.quotePath=false "$@"; }

staged_records() {
  git_q diff --cached --name-only --diff-filter=d | paths ""
  git_q diff --cached -U0 --no-color --no-ext-diff --diff-filter=d | added_lines ""
}

# Args are rev-list arguments selecting the commits to scan.
commit_records() {
  local sha short parents
  git rev-list --parents "$@" | while read -r sha parents; do
    short="${sha:0:12}:"
    if [ "$(wc -w <<<"$parents")" -gt 1 ]; then
      git_q diff-tree -p --cc -U0 --no-color --no-ext-diff --no-commit-id "$sha" | added_lines "$short"
    else
      git_q diff-tree -r --root --name-only --no-commit-id --diff-filter=d "$sha" | paths "$short"
      git_q diff-tree -p --root -U0 --no-color --no-ext-diff --no-commit-id --diff-filter=d "$sha" | added_lines "$short"
    fi
    git log -1 --format=%B "$sha" | message_lines "$short" 0
  done
}

range_records() {
  local a b
  case "$1" in
    *...*) a="${1%%...*}"; b="${1#*...}" ;;
    *..*) a="${1%%..*}"; b="${1#*..}" ;;
    *) echo "forbidden-terms: --range wants <a>..<b>" >&2; exit 2 ;;
  esac
  commit_records "${b:-HEAD}" --not "$a"
}

pre_push_records() {
  local zero local_sha remote_sha
  zero="0000000000000000000000000000000000000000"
  [ -t 0 ] && return 0
  while read -r _ local_sha _ remote_sha; do
    case "$local_sha" in "" | "$zero") continue ;; esac
    if [ "$remote_sha" = "$zero" ] || ! git cat-file -e "$remote_sha" 2>/dev/null; then
      commit_records "$local_sha" --not --remotes
    else
      commit_records "$local_sha" --not "$remote_sha"
    fi
  done
}

build() {
  [ $# -ge 2 ] || usage
  local out="$1" terms="$2" regexes="${3:-}"
  umask 077
  mkdir -p "$(dirname "$out")"
  {
    echo "# built by scripts/check-forbidden-terms.sh --build"
    sed -e 's/\r$//' -e '/^[[:space:]]*$/d' -e 's/^/lit:/' "$terms"
    if [ -n "$regexes" ]; then
      sed -e 's/\r$//' -e '/^[[:space:]]*$/d' -e 's/^/re:/' "$regexes"
    fi
  } >"$out"
  : | perl -e "$matcher" "$out" || { rm -f "$out"; exit 2; }
  echo "forbidden-terms: wrote $(grep -c -v '^#' "$out") patterns to $out"
}

mode=""
arg=""
while [ $# -gt 0 ]; do
  case "$1" in
    --patterns) [ $# -ge 2 ] || usage; pattern_file="$2"; shift 2 ;;
    --staged | --pre-push) mode="$1"; shift ;;
    --message | --range) [ $# -ge 2 ] || usage; mode="$1"; arg="$2"; shift 2 ;;
    --build) shift; build "$@"; exit 0 ;;
    *) usage ;;
  esac
done
[ -n "$mode" ] || usage

if [ ! -s "$pattern_file" ]; then
  echo "forbidden-terms: no pattern file at $pattern_file, skipping"
  exit 0
fi

export LC_ALL=C
records() {
  case "$mode" in
    --staged) staged_records ;;
    --message) message_lines "" 1 <"$arg" ;;
    --range) range_records "$arg" ;;
    --pre-push) pre_push_records ;;
  esac
}

rc=0
hits=$(records | perl -e "$matcher" "$pattern_file") || rc=$?
if [ "$rc" -eq 0 ]; then
  exit 0
fi
if [ "$rc" -ne 1 ] || [ -z "$hits" ]; then
  echo "forbidden-terms: check failed (exit $rc)" >&2
  exit 2
fi
echo "✖ forbidden term at:"
awk '{ print "  " $0 }' <<<"$hits"
echo "  A private name from the forbidden-terms list landed here; use a neutral name (acme, widgets, /home/alice)."
exit 1
