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

echo "Writing $1/mabiao.tsv ..."
perl -CSDA -Mutf8 -F'\t' -lanE '
  $roots{$F[0]} = lc($F[1]);

  END {
      print "不\tu";
      print "是\ti";
      print "我\to";
      print "的\te";
      print "了\ta";

      open my $fh, "chaifen.txt";
      while (<$fh>) {
          chomp;
          @a = split /\t/;
          @b = split /\s+/, $a[1];
          $s = "";

          for (@b) { $s .= substr($roots{$_}, 0, 1); }
          if (@b == 2) {
              $s .= substr($roots{$b[-1]}, 1, 1) if length($roots{$b[-1]}) > 2;
              $r = $b[0];
          } else {
              $r = $b[-1];
          }
          $s .= substr($roots{$r}, 1);
          $s = substr($s, 0, 4) if length($s) > 4;

          $len = length($s);
          if (exists $full_codes{$s}) {
              ++$len;                   # 非首选加一码，不考虑翻页，不递归考虑已设的简码
          } else {
              $full_codes{$s} = $a[0];  # 只记录首选
          }

          $chars{$a[0]} = { code => $s, len => $len, freq => $a[2],
                            y => substr($roots{$b[-1]}, -1), seq => $. };
      }

      %short_chars = map { $_ => 1 } qw/不 是 我 的 了/;

      for $i (2 .. 3) {
          while (($k, $v) = each %chars) {
              $v->{score} = ($v->{len} - $i) * $v->{freq};
          }

        for $char (sort { $chars{$b}{score} <=> $chars{$a}{score} || $chars{$b}{freq} <=> $chars{$a}{freq} || $a cmp $b } keys %chars) {
            next if exists $short_chars{$char};

            $v = $chars{$char};
            next if $i >= length($v->{code});
            $s = substr($v->{code}, 0, $i - 1) . $v->{y};

            next if exists $short_codes{$s};
            next if exists $full_codes{$s} && (! exists $roots{ $full_codes{$s} } || length($roots{ $full_codes{$s} }) == 3);
            $short_codes{$s} = 1;
            $short_chars{$char} = 1;
            print "$char\t$s";
        }
      }

      for $char (sort { $chars{$a}{seq} <=> $chars{$b}{seq} } keys %chars) {
          print "$char\t$chars{$char}{code}";
      }
  }
' "$1/roots.tsv" > "$1/mabiao.tsv"

echo "Writing $1/moling.js ..."
"$TYPER_ROOT/scripts/turn-roots-chaifen-mabiao-into-js.pl" "$1/roots.tsv" "$1/chaifen.tsv" "$1/mabiao.tsv" > "$1/moling.js"

VER=$(date +%Y.%m.%d.%H%M)
echo "Writing $1/moling-$VER.html ..."
"$TYPER_ROOT/scripts/generate-roots-chart.pl" -e "$1/moling.js" -t "魔靈輸入法字根表 $VER" -c 简体字频表-2.5b.txt \
  "$1/roots.tsv" "$1/chaifen.tsv" <(head -n 6000 简体字频表-2.5b.txt | awk '{print $1}') > "$1/moling-$VER.html"

