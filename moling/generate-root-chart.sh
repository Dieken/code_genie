#!/usr/bin/env bash

set -euo pipefail
shopt -s failglob

: "${TYPER_ROOT:="$HOME/home/typer"}"

[ -d "$TYPER_ROOT/scripts" ] || TYPER_ROOT=typer
[ -d "$TYPER_ROOT/scripts" ] || TYPER_ROOT=../../typer
[ -d "$TYPER_ROOT/scripts" ] || {
  echo "ERROR: typer scripts not found, checkout it from https://github.com/Dieken/typer/ and set environment variable TYPER_ROOT." >&2
  exit 1
}

[ $# = 0 ] && {
  echo "Usage: $0 output-TIMESTAMP" >&2
  exit 1
}

echo "Writing $1/roots.tsv ..."
perl -CSDA -lanE '
  next unless /^(\S+)\.([UASY])/;
  $h{$1}{$2} = lc($F[1]);
  END {
    while (($k, $v) = each %h) {
      push @roots, [$k, "$v->{U}$v->{S}$v->{Y}"] if exists $v->{U};   # 飞键字根
      die "No ASY root found for $k!\n" unless exists $v->{A};
      push @roots, [$k, "$v->{A}$v->{S}$v->{Y}"];                     # 正常字根
    }

    @roots = sort {
      $b->[1] =~ /[aeuio]/ <=> $a->[1] =~ /[aeuio]/ ||
      $a->[1] cmp $b->[1] ||
      $a->[0] cmp $b->[0]
    } @roots;

    for (@roots) { print "$_->[0]\t", ucfirst($_->[1]); }
  }
  ' "$1/output-keymap.txt" > "$1/roots.tsv"

echo "Writing $1/chaifen.tsv ..."
perl -CSDA -F'\t' -lanE '$F[1]=~s/\s//g; print "$F[0]\t$F[1]"' chaifen.txt > "$1/chaifen.tsv"

echo "Writing $1/moling.js ..."
"$TYPER_ROOT/scripts/turn-roots-chaifen-mabiao-into-js.pl" "$1/roots.tsv" "$1/chaifen.tsv" "$1/output-combined.txt" > "$1/moling.js"

VER=$(date +%Y.%m.%d.%H%M)
echo "Writing $1/moling-$VER.html ..."
"$TYPER_ROOT/scripts/generate-roots-chart.pl" -e "$1/moling.js" -t "魔靈輸入法字根表 $VER" -c 简体字频表-2.5b.txt \
  "$1/roots.tsv" "$1/chaifen.tsv" <(head -n 6000 简体字频表-2.5b.txt | awk '{print $1}') > "$1/moling-$VER.html"

