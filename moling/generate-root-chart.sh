#!/usr/bin/env bash

set -euo pipefail
shopt -s failglob

. ./init.sh

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

SOURCE="$1/source"
[ -d "$SOURCE" ] || SOURCE="$1/../source"
[ -d "$SOURCE" ] || {
  echo "ERROR: source directory not found in $1/source or $1/../source" >&2
  exit 1
}

echo "Writing $1/roots.tsv ..."
if grep -Eq '^\S+\.[UASY]\t' "$1/output-keymap.txt"; then
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
else
  perl -CSDA -lanE '
  next if /^\s*#/;
  print "$F[0]\t$F[1]";
  ' "$1/output-keymap.txt" > "$1/roots.tsv"
fi

echo "Writing $1/roots-mapping.tsv ..."
perl -CSDA -F, -lanE '
    next if $. == 1 || exists $h{$F[1]};
    print "$F[1]\t$F[2]" if $F[1] ne $F[2] && $F[1] =~ /\{/;
    $h{$F[1]} = 1;
    ' yuhao-zigens.csv | LC_ALL=C sort -u > "$1/roots-mapping.tsv"

echo "Writing $1/zigen-moling.csv and $1/zigen-trainer-moling.json ..."
export ROOTS_TXT="$SOURCE/roots-$SCHEMA.txt"
[ -f "$ROOTS_TXT" ] || ROOTS_TXT="$SOURCE/roots.txt"
export ROOTS_CLUSTER_TXT="$SOURCE/roots-cluster-$SCHEMA.txt"
[ -f "$ROOTS_CLUSTER_TXT" ] || ROOTS_CLUSTER_TXT="$SOURCE/roots-cluster.txt"
perl -CSDA -Mautodie -Mutf8 -lanE 'use List::Util qw/uniqstr/; use JSON::PP;
  $roots{$F[0]} = lc($F[1]);

  END {
      %order = ();
      for $root (sort { $roots{$a} cmp $roots{$b} || $a cmp $b } keys %roots) {
          $order{$root} = scalar(keys(%order)) * 1000;
      }

      open my $fh, "yuhao-zigens.csv";
      while (<$fh>) {
          next if $. == 1;
          chomp;
          @a = split /,/;
          next if $a[1] eq "⺮";    # 用「竹」代替了
          $mapping{$a[1]}{$a[2]} = $.;
      }
      undef $fh;

      open $fh, $ENV{ROOTS_TXT};
      while (<$fh>) {
          chomp;
          @a = split;
          $pinyin{ $a[0] } = $a[2] // "";
      }
      undef $fh;

      open $fh, $ENV{ROOTS_CLUSTER_TXT};
      while (<$fh>) {
          next if /^\s*#/ || /^\s*$/;
          @a = split /\t/;
          @a = split /\s+/, $a[0];
          for ($i = 1; $i < @a; ++$i) {
              $order{$a[$i]} = $order{$a[0]} + $i;
          }
      }

      $a = scalar keys %roots;
      $b = scalar keys %mapping;
      die "Inconsistent number of roots: $a vs $b\n" if $a != $b;

      @zigen = ();      # for https://github.com/hch12907/zigen-trainer
      %strokes = qw(a 折 e 撇 u 竖 i 点 o 横);
      %classify = ( map({ $_ => "简" } split /\s+/, "乌 {纟上} 纟 钅 长 {鸟上} 鸟"),
                    map({ $_ => "繁" } split /\s+/, "來 僉 烏 {烏上} 見 貝 頁 車 {專上} 叀 門 風 飛 馬 {馬上} 鬥 魚 鳥 {鳥上} 鹵 齒 {龍右} {亞下} {亜下} {亞中} {黽中} {爲下}")
                  );

      print "font,ma,pinyin";
      @a = sort { $order{$a} <=> $order{$b} } keys %roots;
      for $r (@a) {
          @b = uniqstr sort { ($a eq $r ? -1 : $b eq $r ? 1 : 0) || $mapping{$r}{$a} <=> $mapping{$r}{$b} } keys %{ $mapping{$r} };
          for (@b) {
              next if exists $fonts{$_};

              print $_, ",", $roots{$r}, ",", $pinyin{$r};
              $fonts{$_} = 1;     # 艹 和 卄 的 font 都用的 艹，去重以避免在字根表中重复

              if ($order{$r} % 1000 == 0 && $_ eq $b[0]) {
                  push @zigen, {
                      "type"     => "类",
                      "groups"   => [{
                          "zigens"       => [ $_  ],
                          "code"         => $roots{$r},
                          "classify"     => $classify{$r} // "通",
                          "description"  => "$_  拼音：" . ($pinyin{$r} || "无") . "  首笔：" . $strokes{ substr($roots{$r}, -1) } . "(" . substr($roots{$r}, -1) . ")",
                      }],
                      #"description"  => join(" ", grep { $order{$_} >= int($order{$r} / 1000) * 1000 && $order{$_} < int(($order{$r} + 1000) / 1000) * 1000 } @a),
                      "description"   => "",
                  };
              } else {
                  my $last_code = $zigen[-1]{"groups"}[-1]{"code"};
                  if ($roots{$r} eq $last_code) {
                      push @{ $zigen[-1]{"groups"}[-1]{"zigens"} }, $_;
                  } else {
                      push @{ $zigen[-1]{"groups"} }, {
                          "zigens"       => [ $_  ],
                          "code"         => $roots{$r},
                          "classify"     => "通",
                          "description"  => "$_  拼音：" . ($pinyin{$r} || "无") . "  首笔：" . $strokes{ substr($roots{$r}, -1) } . "(" . substr($roots{$r}, -1) . ")",
                      };
                  }
              }
          }
      }

      for (qw(门門 幺厶 穴宀 人 口囗 心 火灬 耂土 糸 金 食 长長 鸟鳥 乌烏 鱼魚 马馬 车車 页頁 见見 贝貝 刀 立辛 丬 言 田甲 大夫 七戈 丿 艹卅)) {
          push @zigen, {
              "type"        => "混",
              "zigens"      => [ split //, $_ ],
              "description" => $_,
          };
      }

      print STDERR JSON::PP->new->canonical->pretty->encode(\@zigen);
  }
' "$1/roots.tsv" > "$1/zigen-moling.csv" 2>"$1/zigen-trainer-moling.json" || {
    echo
    cat "$1/zigen-trainer-moling.json"
    exit 1
}

echo "Writing $1/chaifen.tsv ..."
export CHAIFEN_TXT="$SOURCE/chaifen.txt"
export CHAIFEN_ALL_TXT="$SOURCE/chaifen-all.txt"
[ -f "$CHAIFEN_ALL_TXT" ] || {
    # 老的 ./optimize.sh 没有备份 ./chaifen-all.txt
    echo "    !!! use './chaifen-all.txt' for '$1', may be inconsistent!" >&2;
    CHAIFEN_ALL_TXT="chaifen-all.txt"
}
perl -CSDA -F'\t' -lanE '$F[1]=~s/\s+//g; print "$F[0]\t$F[1]"' "$CHAIFEN_ALL_TXT" > "$1/chaifen.tsv"

echo "Writing $1/mabiao.tsv ..."
perl -CSDA -Mautodie -Mutf8 -F'\t' -lanE '
  $roots{$F[0]} = lc($F[1]);

  END {
      if ($ENV{ENCODE_RULE} eq "xiaoming") {
          print "的\td";
          print "是\tj";
          print "不\tk";
          print "了\te";
          print "在\tf";
          print "我\ti";
      } else {
          print "不\tu";
          print "是\ti";
          print "我\to";
          print "的\te";
          print "了\ta";
      }

      open my $fh, $ENV{DISABLE_FULL_CHARSET} ? $ENV{CHAIFEN_TXT} : $ENV{CHAIFEN_ALL_TXT};
      while (<$fh>) {
          chomp;
          @a = split /\t/;
          @b = split /\s+/, $a[1];
          $s = "";

          if ($ENV{ENCODE_RULE} eq "yuling") {          # 使用宇浩灵明单字编码规则
              for (@b) { die "Unknown root $_\n" unless exists $roots{$_}; }

              $s .= substr($roots{$b[0]}, 0, 1);
              $s .= substr($roots{$b[0]}, 1, 1) if length($roots{$b[0]}) > 2;
              $s .= substr($roots{$b[0]}, -1) if @b == 1;

              if (@b > 1) {
                  for ($i = 1; $i < @b; ++$i) {
                      next if @b > 3 && $i == 2 && length($roots{$b[0]}) > 2;
                      $s .= substr($roots{$b[$i]}, 0, 1);
                  }

                  $s .= substr($roots{$b[-1]}, 1, 1) if length($roots{$b[-1]}) > 2;
                  $s .= substr($roots{$b[-1]}, -1);
              }
          } elsif ($ENV{ENCODE_RULE} eq "xiaoming") {   # 使用潇明单字编码规则
              for (@b) { die "Unknown root $_\n" unless exists $roots{$_}; }

              $s = substr($roots{$b[0]}, 0, 2);
              if (@b == 1) {
                  $s = $roots{$b[0]};
              } elsif (@b == 2) {
                  $s .= substr($roots{$b[1]}, 0, 2);
              } elsif (@b == 3) {
                  $s .= substr($roots{$b[1]}, 0, 1) . substr($roots{$b[2]}, 0, 2);
              } else {
                  $s .= substr($roots{$b[1]}, 0, 1) . substr($roots{$b[2]}, 0, 1) . substr($roots{$b[-1]}, 0, 1);
              }
          } else {                                      # 使用魔灵单字编码规则
              for (@b) {
                  die "Unknown root: $_ in $_\n" unless exists $roots{$_};
                  $s .= substr($roots{$_}, 0, 1);
              }

              if (@b == 2) {
                  $s .= substr($roots{$b[-1]}, 1, 1) if length($roots{$b[-1]}) > 2;
                  $r = $b[0];
              } else {
                  $r = $b[-1];
              }
              $s .= substr($roots{$r}, 1);
          }

          $s = substr($s, 0, $ENV{MAX_CODE_LEN}) if length($s) > $ENV{MAX_CODE_LEN};

          $len = length($s);
          if (exists $full_codes{$s}) {
              ++$len;                   # 非首选加一码，不考虑翻页，不递归考虑已设的简码
          } else {
              $full_codes{$s} = $a[0];  # 只记录首选
          }

          $chars{$a[0]} = { code => $s, len => $len, freq => $a[2],
                            y => substr($roots{$b[-1]}, -1), seq => $. };

          $chars{$a[0]}{y} = substr($roots{$b[-1]}, 1, 1) if $ENV{ENCODE_RULE} eq "xiaoming";
      }

      %short_chars = map { $_ => 1 } qw/不 是 我 的 了/;
      $short_chars{"在"} = 1 if $ENV{ENCODE_RULE} eq "xiaoming";

      %stroke_mapping = qw(e i i e a u);    # 不映射 u 和 o 到 e 以避免减少可用简码空间

      for $i (2 .. 3) {
          while (($k, $v) = each %chars) {
              $v->{score} = ($v->{len} - $i) * $v->{freq};
          }

        for $char (sort { $chars{$b}{score} <=> $chars{$a}{score} || $chars{$b}{freq} <=> $chars{$a}{freq} || $a cmp $b } keys %chars) {
            next if exists $short_chars{$char};

            $v = $chars{$char};
            next if $i >= length($v->{code}) || $v->{freq} < 1;

            $s = "";

            if ($ENV{PREFER_SPACE_SHORTCODE}) {
                $s = substr($v->{code}, 0, $i - 1) . "_" unless $ENV{ENCODE_RULE} eq "xiaoming" && $i > 2;  # 潇明只有 A_ 空格简
            }

            if (! $s || exists $short_codes{$s}) {
                if ($ENV{ENCODE_RULE} eq "xiaoming") {
                    if (length($v->{code}) == 3) {                  # 字根字
                        next unless $ENV{ENABLE_SPACE_SHORTCODE};
                        $s = substr($v->{code}, 0, 1) . "_";        # 只可能是二简 A_
                    } else {                                        # 二根及以上根字，全码长一定 >= 4
                        if ($i == 2) {                              # 二简 AA
                            $s = substr($v->{code}, 0, 1) . substr($v->{code}, 2, 1);
                        } elsif ($i == 3) {                         # 三简 ABB
                            $s = substr($v->{code}, 0, 2) . $v->{y};
                        } else {                                    # 只出到三简
                            next;
                        }
                    }
                } else {
                    $s = substr($v->{code}, 0, $i - 1) . $v->{y};   # 对二根字也取末根的韵码，不回头，以避开高频的部首首根
                }
            }

            if ($ENV{ENABLE_SHORTCODE_MAPPING}) {   # 默认不开启，会损害简码效率
                # 根据魔灵的笔画映射提升手感
                $s =~ s/([qwrtsdfgzxcvb])([ae])$/$1 . $stroke_mapping{$2}/e;
                $s =~ s/([yphjklnm])([i])$/$1 . $stroke_mapping{$2}/e;
            }

            if (exists $short_codes{$s}) {
                next unless $ENV{ENABLE_SPACE_SHORTCODE};
                next if $ENV{ENCODE_RULE} eq "xiaoming" && $i > 2;  # 潇明只有 A_ 空格简
                $s = substr($v->{code}, 0, $i - 1) . "_";
                next if exists $short_codes{$s};
            }

            # 抢占低频字的码位，有可能增大一点重码率！！！
            next if exists $full_codes{$s} && $chars{ $full_codes{$s} }{seq} <= $ENV{SIMPLE_PROTECT_TOP_N};

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

echo "Downloading https://shurufa.app/fonts/Yuniversus.woff to $1/Yuniversus.woff ..."
curl -L -z "$1/Yuniversus.woff" -o "$1/Yuniversus.woff" 'https://shurufa.app/fonts/Yuniversus.woff'

VER=$(date +%Y.%m.%d)
echo "Writing $1/moling-$VER.html ..."
export FULL_FREQ_TXT="$SOURCE/full-freq-$SCHEMA.txt"
[ -f "$FULL_FREQ_TXT" ] || FULL_FREQ_TXT="$SOURCE/full-freq.txt"
[ -f "$FULL_FREQ_TXT" ] || {
    FULL_FREQ_TXT="./full-freq-$SCHEMA.txt"
    [ -f "$FULL_FREQ_TXT" ] || FULL_FREQ_TXT="./full-freq.txt"
    # 老的 ./optimize.sh 没有备份 ./full-freq*.txt
    echo "    !!! use '$FULL_FREQ_TXT' for '$1', may be inconsistent!" >&2;
}
"$TYPER_ROOT/scripts/generate-roots-chart.pl" -e "$1/moling.js" -t "魔靈輸入法字根表 $VER" -c "$FULL_FREQ_TXT" \
  -f "$1/Yuniversus.woff" -r "$1/roots-mapping.tsv" \
  "$1/roots.tsv" "$1/chaifen.tsv" <(head -n 8000 "$FULL_FREQ_TXT" | awk '{print $1}') > "$1/moling-$VER.html"
