#!/usr/bin/env bash

set -euo pipefail
shopt -s failglob

#########################################################################################################################

. ./init.sh

#########################################################################################################################

if [ "$FULL_FREQ_TXT" = full-freq.txt ]; then
    if [ "$USE_MIXED_FREQ" -a "$USE_MIXED_FREQ" != 0 ]; then
        echo "(0) 生成简繁混合字频表 full-freq.txt ，繁体字频权重=$USE_MIXED_FREQ ..."
        [ -f charAbsoluteFrequencySC.json ] || curl -O https://ceping.shurufa.app/data/charAbsoluteFrequencySC.json
        [ -f charAbsoluteFrequencyTC.json ] || curl -O https://ceping.shurufa.app/data/charAbsoluteFrequencyTC.json

        USE_MIXED_FREQ="$USE_MIXED_FREQ" perl -CSDA -Mautodie -Mutf8 -lE 'use JSON::PP; use POSIX; use List::Util qw/max/;
            sub read_file {
                open my $fh, "<", $_[0];
                binmode($fh);
                local $/;
                my $data = <$fh>;
                close $fh;
                return $data;
            }

            $r = $ENV{USE_MIXED_FREQ} + 0;
            $h = decode_json(read_file("charAbsoluteFrequencySC.json"));
            $h2 = decode_json(read_file("charAbsoluteFrequencyTC.json"));

            # 使用 max 以保持简体高频字顺序和繁体高频字各自的顺序
            for (keys %$h2) { $h->{$_} = max($h->{$_} // 0, $r * $h2->{$_}) }

            for (sort { $h->{$b} <=> $h->{$a} || $a cmp $b } %$h) {
                print "$_\t", ceil($h->{$_});
            }
        ' > full-freq.txt
    else
        echo '(0) 生成简体字频表 full-freq.txt ...'
        perl -CSDA -lanE 'print "$F[0]\t$F[1]"' beiyu-char-freq.txt > full-freq.txt
    fi
else
    echo "(0) 使用方案自行提供的字频表 $FULL_FREQ_TXT ..."
fi


echo '(1) 从 yuhao_charsets.lua 生成简繁常用字符集 chars.txt ...'
# 注意不要改变此文件内容，此文件也被用于生成 roots.txt 时判断字根是否常用字，
# 方案自行提供的算码字集可以放在 chars-$SCHEMA.txt 中。
perl -CSDA -lnE 'print if (/\[\[/ .. eof) && /^\p{Han}$/' yuhao_charsets.lua | LC_ALL=C sort -u > chars.txt
[ "$CHARS_TXT" = "chars.txt" ] || echo "    !!! 使用方案自行提供的算码字集 $CHARS_TXT ..."


echo "(2) 从字频表 $FULL_FREQ_TXT 生成算码字符集的字频表 freq.txt ..."
perl -CSDA -Mautodie -Mutf8 -lanE '
  BEGIN { open my $fh, $ENV{CHARS_TXT}; while (<$fh>) {chomp; $h{$_} = 1} }

  next unless defined $F[1] && $F[1] > 0;
  next unless exists $h{$F[0]};
  print "$F[0]\t$F[1]";
  delete $h{$F[0]};

  END {
      @a = sort keys %h;
      warn "    WARN: " .  scalar(@a) . " 个字符没有权重!\n" if @a != 0;
      for (@a) { print "$_\t0" }
  }' "$FULL_FREQ_TXT"  > freq.txt


echo '(3) 从宇浩星陈方案的大陆字形拆分表 yustar_chaifen.dict.yaml 生成算码字符集的拆分表 chaifen.txt 和 全字集拆分表 chaifen-all.txt ...'
perl -CSDA -Mautodie -Mutf8 -lanE '
  BEGIN {
    open my $fh, "freq.txt";
    while (<$fh>) {
      chomp;
      @a = split;
      $h{$a[0]} = $a[1];
    }

    # yustar_chaifen.dict.yaml 里不区分 𧾷 vs 足，⼟旁 vs 土，礻 vs 示，爫 vs 爪，牜 vs 牛，
    # 因此也不区分 ⺮  vs 竹，以让字根「⺮ 」不被视为无声母字根，提高这类字根的统一性，也
    # 提高竹字头二根字的编码空间。
    %root_mapping = qw(
        ⺮  竹
    );
  }

  if (! $ok) {
    next unless /^\.\.\./;
    $ok = 1;
  }
  next unless /^(\S)\t\[([^,]+)/;
  next unless exists $h{$1};
  $a = $1;
  @a = $2 =~ /\{[^\}]+\}|\S/g;
  @a = map { $root_mapping{$_} // $_ } @a;
  print "$a\t", join(" ", @a), "\t$h{$a}";
  delete $h{$a};
  END { @a = sort keys %h; die "No chaifen found for @a" if @a > 0}
  ' yustar_chaifen.dict.yaml | LC_ALL=C sort -t $'\t' -s -k3,3nr -k1,1 > chaifen.txt

for s in chaifen chaifen_tw; do
    f=yustar_$s.dict.yaml
    [ -f "$f" ] && perl -CSDA -Mautodie -Mutf8 -lanE '
    BEGIN {
        open my $fh, $ENV{FULL_FREQ_TXT};
        while (<$fh>) {
            chomp;
            @a = split;
            $h{$a[0]} = $a[1];
        }

        # yustar_chaifen.dict.yaml 里不区分 𧾷 vs 足，⼟旁 vs 土，礻 vs 示，爫 vs 爪，牜 vs 牛，
        # 因此也不区分 ⺮  vs 竹，以让字根「⺮ 」不被视为无声母字根，提高这类字根的统一性，也
        # 提高竹字头二根字的编码空间。
        %root_mapping = qw(
            ⺮  竹
        );
    }

    if (! $ok) {
        next unless /^\.\.\./;
        $ok = 1;
    }
    next unless /^(\S)\t\[([^,]+)/;
    $a = $1;
    @a = $2 =~ /\{[^\}]+\}|\S/g;
    @a = map { $root_mapping{$_} // $_ } @a;
    print "$a\t", join(" ", @a), "\t", $h{$a} // 0;
    ' "$f" | LC_ALL=C sort -t $'\t' -s -k3,3nr -k1,1 > "$s"-all.txt
done


echo '(4) 从 chaifen-all.txt 生成字根频率表 roots-freq.txt ...'
perl -CSDA -F'\t' -lanE '
  @a = split /\s+/, $F[1];
  $n += $F[2] * @a;
  for (@a) { $h{$_} += $F[2]; }
  END {
    for (sort { $h{$b} <=> $h{$a} || $a cmp $b } keys %h) {
      printf "%s\t%.8f\n", $_, 100 * $h{$_} / $n;
    }
  }' chaifen-all.txt > roots-freq.txt


echo '(5) 从万象拼音词典 chars.dict.yaml 生成字根读音 roots-pinyin.txt ...'
perl -CSDA -Mautodie -Mutf8 -lanE '
  BEGIN {
    open my $fh, "roots-freq.txt";
    while (<$fh>) {
      chomp;
      @a=split;
      $h{$a[0]}=1;
    }
  }
  next unless exists $h{$F[0]};
  @a = @F[1..$#F];
  %h2=();
  for (@a) {
    $h2{$1}=1 if /^([^\d\s]+)/i && $1 ne "无";
  }
  for (sort keys %h2) {
    next if "$F[0] $_" eq "車 jū";      # 使用更低频的 chē
    next if "$F[0] $_" eq "糸 mì";      # 使用更低频的 sī
    next if "$F[0] $_" =~ /^[長长] zh/; # 使用更低频的 cháng

    if ($F[0] eq "土" && $ENV{USE_PINYIN_DU_FOR_TU}) {
      print "土\tdù\t0";    # 使用「杜」dù 音
    } else {
      print "$F[0]\t$_\t", ($F[-1] =~ /^\d/ ? $F[-1] : "0");
    }
  }'  chars.dict.yaml | LC_ALL=C sort -u -k1,1 -k3,3nr -k2,2 > roots-pinyin.txt


echo '(6) 从 roots-pinyin.txt 修正并生成字根声码韵码表 roots.txt ...'
if [ "$ROOTS_TXT" != roots.txt ]; then
    echo "    !!! 使用方案自行提供的字根声码韵码表 $ROOTS_TXT ..."
    if [ -s "$ROOTS_TXT" ]; then
        echo "    !!! 已存在 ${ROOTS_TXT}，跳过初始化，如果需要重新初始化，请使用命令 ': > $ROOTS_TXT' 清空它。"
    else
        echo "    !!! 使用宇浩灵明字根初始化 $ROOTS_TXT ..."
        curl -s https://shurufa.app/zigen-ling.csv |
            perl -CSDA -Mutf8 -F, -lanE '
            BEGIN {
                open $fh, "yuhao-zigens.csv";
                while (<$fh>) {
                    next if $.==1;
                    chomp;
                    @a=split /,/;
                    $h{$a[2]} = $a[1];
                }

                $h{"竹"} = "竹";    # 替代 ⺮
            }

            next if $.==1;
            print $h{$F[0]}, "\t", substr($F[1], 1), "\t", join(" ", @F[2..$#F]);
            print "卄\t", substr($F[1], 1), "\t", join(" ", @F[2..$#F]) if $h{$F[0]} eq "艹";
            ' | LC_ALL=C sort -u -k2,2 -k1,1 > $ROOTS_TXT
    fi
else
perl -CSDA -Mautodie -Mutf8 -F'\t' -lanE 'use Unicode::Normalize;
  BEGIN {
    open my $fh, "roots-pinyin.txt";
    while (<$fh>) {
      @a = split;
      next if exists $h{$a[0]};
      $pinyin{$a[0]} = $a[1];

      $a[1] = NFKD($a[1]);
      $a[1] =~ s/\p{M}//g;

      $a[1] = "0e" if $a[1] eq "er";
      $a[1] = "nu" if $a[1] eq "nv";  # 女
      die "Invalid pinyin: $_\n" unless $a[1] =~ /^([^aeuio]).*?(?:[iu])?([aeuio])/;
      $h{$a[0]} = "$1$2";
    }
    close $fh;
    undef $fh;

    open $fh, "chars.txt";      # 常用字集，不是算码字集 $CHARS_TXT
    while (<$fh>) {
        chomp;
        $common_chars{$_} = 1;
    }

    $common_chars{"戸"} = 1;     # 特殊处理，保持 hu 音

    %fixes = (
      "冫"     => "0e",  # bi，与 二 归并
      "⺀"     => "0e",  # o, 与 二 归并
      "{飞右}" => "0e",  # o, 与 二 归并
      "屮"     => "ca",  # ce，取 cao
      "丶"     => "da",  # zu, 取 dian
      "乀"     => "da",  # fu, 与 丶 归并
      "土"     => $ENV{USE_PINYIN_DU_FOR_TU} ? "du" : "tu",    # tu, 音托时取 du
      "朩"     => "mu",  # de, 与 木 归并
      "丨"     => "su",  # gu, 取 shu
      "丆"     => "ca",  # ha, 与 厂 归并
      "乚"     => "yi",  # ha, 取 yi
      "車"     => "ce",  # ju, 取 che
      "巜"     => "ca",  # ka, 与 巛 归并
      "糸"     => "si",  # mi, 取 si
      "{丄丶}" => "sa",  # o，与 丄 归并
      "リ"     => "ba",  # o, 与 丷 归并
      "{周框}" => "ba",  # o, 与 勹 归并
      "⺊"     => "bo",  # o, 与 卜 归并
      "{即左}" => "ge",  # o, 与 艮 归并
      "{荒下}" => "ca",  # o, 与 川 归并
      "ᅲ"       => "ji",  # o, 与 丌 归并
      "⺽"     => "ju",  # o, 与 臼 归并
      "{奉下}" => "ka",  # o，与 㐄 归并
      "{贏框}" => "lo",  # o, 取 luo
      "{曾中}" => "ri",  # o, 与 日 归并
      "{横日}" => "ri",  # o, 与 日 归并
      "龶"     => "se",  # o, 取 sheng
      "{眉上}" => "si",  # o, 与 尸 归并
      "{豕下}" => "si",  # o, 与 豕 归并
      "⺶"     => "ya",  # o, 与 羊 归并
      "ナ"     => "zo",  # o, 取 zuo
      "𡗗"     => "di",  # pe, 取 di
      "丅"     => "di",  # xa, 与 丁 归并
      "ㄩ"     => "ka",  # yu, 取 kan
      "长"     => "ca",  # za, 取 chang
      "長"     => "ca",  # za, 取 chang
      "ㄗ"     => "je",  # zi, 与 卩 归并
      "艹"     => "na",  # ca, 与 卄 归并
      "彳"     => "i",   # ci, 在常用字里，但不取声母
      "亍"     => "u",   # cu, 在常用字里，但不取声母
      "咼"     => "a",   # ga, 在常用字里，但不取声母以与「骨」首笔分开
      "尢"     => "o",   # yo, 在常用字里，但不取声母以与「尤」首笔分开
    );
  }

  if (exists $fixes{$F[0]}) {
    $a = $fixes{$F[0]};
  } elsif (exists $h{$F[0]}) {
    $a = $h{$F[0]};
  } else {
    $a = "o";
  }

  # 非常用字字根不取声母
  if (! exists $common_chars{$F[0]}) {
      $a =~ s/[^aeuio]//;
  }

  my $consonant = substr($a, 0, 1);
  if ($ENV{OPTIMIZE_KEYS} !~ /$consonant/ && ($consonant = $ENV{"OPTIMIZE_KEY_$consonant"})) {
    $a =~ s/^./$consonant/;
  }

  print "$F[0]\t$a\t", length($a) > 1 ? $pinyin{$F[0]} : "";
' roots-freq.txt | LC_ALL=C sort -k2,2 -k1,1 > roots.txt
fi


echo '(7) 分析首根冲突情况，为选择飞键字根提供参考，写入 roots-fly-candidates.txt ...'
perl -CSDA -F'\t' -lanE '
  @a = split /\s+/, $F[1];
  next unless @a > 1;
  $a = shift @a;
  $h{"@a"}{$a} += $F[2];
  $n += $F[2];
  $freq{$a}[0]++;
  $freq{$a}[1]++;
  $freq{$a}[2] += $F[2];
  $freq{$a}[3] += $F[2];
  for (@a) { $freq{$_}[0] //= 0; $freq{$_}[1]++; $freq{$_}[3] += $F[2] }
  END {
    for (keys %h) {
      @a = sort keys %{ $h{$_} };
      next unless @a > 1;

      # 每一对冲突字根的冲突概率
      for ($i = 0; $i < $#a; ++$i) {
        for ($j = $i + 1; $j < @a; ++$j) {
          $a = 100 * $h{$_}{$a[$i]} / $n;
          $b = 100 * $h{$_}{$a[$j]} / $n;

          $s = "$a[$i] $a[$j]";
          if (exists $h2{$s}) {
              $h2{$s} = [$a[$i], $a + $h2{$s}[1], $a[$j], $b + $h2{$s}[3], $a + $b + $h2{$s}[4]];
          } else {
              $h2{$s} = [$a[$i], $a, $a[$j], $b, $a + $b];
          }
        }
      }

      # 每一个字根的整体冲突概率
      for $a (@a) {
        $h3{$a} += $h{$_}{$a};
      }
    }

    # 按每一对字根的冲突概率从高往低，每次挑出两字根中整体冲突概率高的字根安排到 AEUIO。
    $selected = 0;
    for (sort { $h2{$b}[4] <=> $h2{$a}[4] || $h2{$a}[0] cmp $h2{$b}[0] || $h2{$a}[2] cmp $h2{$b}[2] } keys %h2) {
      next if $h2{$_}[4] < 0.01;
      last if $selected >= 50;

      $p = $h2{$_}[0];
      $q = $h2{$_}[2];
      $a = 100 * $h3{$p} / $n;
      $b = 100 * $h3{$q} / $n;

      $s = "SKIP";
      if (exists $h4{$p}) {         # $p 已被选中
          $h5{$q}{$p} = 1 unless exists $h4{$q};
      } elsif (exists $h4{$q}) {    # $q 已被选中
          $h5{$p}{$q} = 1 unless exists $h4{$p};
      } else {
          if ($a > $b) {
            if (! exists $h5{$p} || scalar keys %{ $h5{$p} } <= 4) {  # AEUIO 只能容纳 5 个互相冲突的根
              $s = "SELECT";
              $h4{$p} = 1;
              $h5{$q}{$p} = 1 unless exists $h4{$q};
              ++$selected;
            }
            $s = sprintf "$s %s/%.2f/%d/%d/%.2f/%.2f", $p, $a, @{ $freq{$p} }[0, 1], 100 * $freq{$p}[2] / $n, 100 * $freq{$p}[3] / $n;
            $s .= " ?? " . join(" ", map { sprintf "%s/%.2f", $_, 100 * $h3{$_} / $n } sort keys %{ $h5{$p} }) if exists $h5{$p};    # 之前高冲突的字根又被选上了
          } else {
            if (! exists $h5{$q} || scalar keys %{ $h5{$q} } <= 4) {  # AEUIO 只能容纳 5 个互相冲突的根
              $s = "SELECT";
              $h4{$q} = 1;
              $h5{$p}{$q} = 1 unless exists $h4{$p};
              ++$selected;
            }
            $s = sprintf "$s %s/%.2f/%d/%d/%.2f/%.2f", $q, $b, @{ $freq{$q} }[0, 1], 100 * $freq{$q}[2] / $n, 100 * $freq{$q}[3] / $n;
            $s .= " ?? " . join(" ", map { sprintf "%s/%.2f", $_, 100 * $h3{$_} / $n } sort keys %{ $h5{$q} }) if exists $h5{$q};    # 之前高冲突的字根又被选上了
          }
      }

      printf "%s\t%.2f\t%s\t%.2f\t%.2f (%.2f : %.2f) ## %s\n", @{ $h2{$_} }, $a, $b, $s;
    }
  }' chaifen.txt > roots-fly-candidates.txt


echo '(8) 生成码灵输入文件 input-fixed.txt, 大码约束 ...'
[ "$ROOTS_CLUSTER_TXT" = roots-cluster.txt ] || echo "    !!! 使用方案自行提供的字根聚类表 $ROOTS_CLUSTER_TXT ..."
perl -CSDA -F'\t' -Mautodie -Mutf8 -MList::Util=sum -lanE '
  BEGIN {
    open my $fh, "roots-freq.txt";
    while (<$fh>) {
      chomp;
      @a = split;
      $freq{$a[0]} = $a[1];
    }
    undef $h;

    open $fh, $ENV{ROOTS_CLUSTER_TXT};
    while (<$fh>) {
      next if /^\s*#/ || /^\s*$/;
      next if /^\s*一\s+f/i && $ENV{ALL_ROOT_KEYS} !~ /f/i;     # 对 F 用作 B 区的方案，跳过宇码传统的「一」大码 F 约束
      chomp;
      @a = split /\t/, $_, 2;
      die "Invalid line in $ENV{ROOTS_CLUSTER_TXT}: $_\n" if $a[0] =~ /[a-z]/ || $a[1] =~ /[^a-z\s]/;
      $a[0] =~ s/^\s*|\s*$//g;
      $a[1] =~ s/^\s*|\s*$//g;
      @b = sort split /\s+/, $a[0];
      $a = sum(map { $freq{$_} // die "ERROR: Unknown root $_ in $ENV{ROOTS_CLUSTER_TXT}" } @b);
      if ($a >= $ENV{TOP_ROOT_FREQ}) {
        $a[1] ||= $ENV{TOP_ROOT_KEYS};
      } elsif ($a >= $ENV{HOT_ROOT_FREQ}) {
        $a[1] ||= $ENV{HOT_ROOT_KEYS};
      } else {
        $a[1] ||= $ENV{ALL_ROOT_KEYS};
      }
      $a[1] = join(" ", split /\s*/, $a[1]);
      printf "# freq=%.8f\n", $a;
      print join(" ", map { "$_.A" } @b), "\t$a[1]";
      for (@b) {
        die "ERROR: root $_ in multiple clusters, check $ENV{ROOTS_CLUSTER_TXT}!\n" if exists $h{$_};
        $h{$_} = 1;
      }
    }
  }

  next if $h{$F[0]};
  $a = $freq{$F[0]};
  if ($a >= $ENV{TOP_ROOT_FREQ}) {
    $b = $ENV{TOP_ROOT_KEYS};
  } elsif ($a >= $ENV{HOT_ROOT_FREQ}) {
    $b = $ENV{HOT_ROOT_KEYS};
  } else {
    $b = $ENV{ALL_ROOT_KEYS};
  }
  printf "# freq=%.8f\n", $a;
  print "$F[0].A\t", join(" ", split /\s*/, $b);
' "$ROOTS_TXT" > input-fixed.txt


echo '(9) 添加码灵输入文件 input-fixed.txt, 声码和韵码约束 ...'
perl -CSDA -F'\t' -Mautodie -Mutf8 -lanE 'use Unicode::Normalize;
  BEGIN {
    if (! $ENV{USE_VOWEL}) {
      # 修正数据错误
      %stroke_overrides = (
        "艹"        => "1",
        "山"        => "2",
        "水"        => "2",
        "{冂丶}"    => "2",
        "{曾中}"    => "2",
        "卯"        => "3",
        "冖"        => "4",
        "忄"        => "4",
        "乃"        => "5",
        "乙"        => "5",
        "{纟上}"    => "6",
      );

      open my $fh, "yuhao-zigens.csv";
      while (<$fh>) {
        next if $. == 1;
        chomp;
        @a = split /,/;
        $b = substr($a[4], 0, 1);
        $b = "5" if $b eq "6" && $ENV{USE_STROKE_5_FOR_6};
        die "Conflict strokes: $_ vs. previously $strokes{$a[1]}\n" if exists $strokes{$a[1]} && $strokes{$a[1]} ne $b;
        $strokes{$a[1]} = $b unless exists $strokes{$a[1]} || exists $stroke_overrides{$a[1]};
      }
      close $fh;

      while (my ($k, $v) = each %stroke_overrides) {
        $v = "5" if $v eq "6" && $ENV{USE_STROKE_5_FOR_6};
        $strokes{$k} = $v;
      }
    }
  }

  # 潇明声码约束
  if ($ENV{SCHEMA} eq "xiaoming") {
    %stroke_mapping = qw( 1 d 2 k 3 f 4 j 5 i 6 e );
    if (length($F[1]) > 1) {
        push @{ $xiaoming_consonants{ substr($F[1], 0, 1) } }, $F[0];
    } else {
        print "$F[0].S\t", $stroke_mapping{ $strokes{ $F[0] } };
    }
    print "$F[0].Y\t", $stroke_mapping{ $strokes{ $F[0] } };
    next;

    END {
        for (sort keys %xiaoming_consonants) {
            print "# $_";
            print join(" ", map { "$_.S" } @{ $xiaoming_consonants{$_} }), "\t", join(" ", split /\s*/, "dfjkei");
        }
    }
  }

  # 声码约束
  if (length($F[1]) > 1) {
    $a = substr($F[1], 0, 1);

    if ($ENV{SCHEMA} eq "yaoling") {
        push @{ $yaoling_consonants{$a} }, $F[0];    # 妖灵的声母重新映射到声码，并且对应固定的韵码
    } else {
        if ($ENV{OPTIMIZE_KEYS} =~ /$a/) {
            push @{ $consonants{$a} }, $F[0];
        } else {
            print "$F[0].S\t", $a;
        }
    }
  } else {
    print "$F[0].S\t", "v" if $ENV{SCHEMA} eq "moqing";   # 魔卿的无声母字根使用 v 作为声码
  }

  # 韵码约束
  if ($ENV{USE_VOWEL}) {    # 使用韵母作为字根的补码
    if ($ENV{SCHEMA} eq "yaoling") {        # 妖灵的声母重新映射到声码，并且对应固定的韵码
        if (length($F[1]) > 1) {
            # 同声母的大根的韵码固定，后面跟声码约束一起输出
        } else {
            print "$F[0].Y\t", substr($F[1], -1);   # 小根复用灵明的韵码
        }
    } elsif ($ENV{SCHEMA} eq "yueling") {   # 月灵的韵码仿日月的映射
        my $pinyin = $F[2] // "";

        $pinyin =~ s/^\s+//;                # 去掉开头的空白和后面的注释
        $pinyin =~ s/\s+.*$//;              # 后面的注释
        $pinyin = NFKD($pinyin);            # 展开音调
        $pinyin =~ s/\p{M}//g;              # 去掉音调
        $pinyin =~ s/^[^aeuio]+//;          # 去掉开头的声母
        $pinyin = "" if $pinyin !~ /^[aeuio][a-z]*$/;   # 检查是不是合法韵母

        if (length($pinyin) > 1) {          # 只映射长度大于 1 的韵母
            push @{ $yueling_vowels{$pinyin} }, $F[0];
        } else {
            print "$F[0].Y\t", substr($F[1], -1);   # 直接用设置好的韵码
        }
    } else {
        print "$F[0].Y\t", substr($F[1], -1) unless $ENV{SCHEMA} eq "moqing";   # 魔卿是双编字根方案
    }
  } else {                  # 使用首笔作为字根的补码
    die "No stroke found for $F[0]!\n" unless exists $strokes{$F[0]};

    my $stroke = $strokes{$F[0]};

    # 对五个笔画，考虑声母在左右时使用替代映射以提高手感
    if (length($F[1]) > 1) {
        my $a = substr($F[1], 0, 1);

        if ($ENV{OPTIMIZE_KEYS} =~ /$a/) {
            warn "    WARN: Optimizing vowel $F[0].Y for mapping consonant $a\n";
            push @{ $Y{ $a } },  $F[0];     # 同声母使用同样的韵码
            next;
        } else {
            if ($stroke =~ /3/ && $F[1] =~ /[qwrtsdfgzxcvb]/) {
                $stroke += 5;   # 左声母 + 撇
            } elsif ($stroke =~ /[124]/ && $F[1] =~ /[yphjklnm]/) {
                $stroke += 5;   # 右声母 + 横竖点
            } elsif ($stroke =~ /5/ && $F[1] =~ /[qwrtsdfgzxcvb]/) {
                $stroke = "A";  # 左声母 + 折
            }
        }
    }

    push @{ $Y{$stroke} },  $F[0];
  }

  END {
    for (sort keys %consonants) {
        print "# $_";
        print join(" ", map { "$_.S" } @{ $consonants{$_} }), "\t", join(" ", split /\s*/, $ENV{ALL_ROOT_KEYS});
    }

    for (sort keys %yaoling_consonants) {
        print "# $_";
        print join(" ", map { "$_.S" } @{ $yaoling_consonants{$_} }), "\t", join(" ", split /\s*/, $ENV{ALL_ROOT_KEYS});
        print join(" ", map { "$_.Y" } @{ $yaoling_consonants{$_} }), "\t", join(" ", split /\s*/, $ENV{B_AREA_KEYS});
    }

    for (sort keys %yueling_vowels) {
        print "# $_";
        print join(" ", map { "$_.Y" } @{ $yueling_vowels{$_} }), "\t", join(" ", split /\s*/, $ENV{B_AREA_KEYS});
    }

    # 按拆分里字根首笔使用情况以及字频加权统计，字根首笔折(5)和竖(2)少，横(1)、撇(3)、点(4) 多
    %stroke_mapping = qw( 1 o 2 u 3 e 4 i 5 a   6 e 7 e 8 i 9 e A u );      # 首根笔画时，退火算法多次选择此映射
    %stroke_constraint = qw( 1 eio 2 au 3 eio 4 eio 5 au    6 eio 7 eio 8 eio 9 eio A au );

    for my $stroke (sort keys %Y) {
      @a = sort @{ $Y{$stroke} };
      if ($stroke =~ /^[1-9A]$/ && $ENV{OPTIMIZE_KEYS} =~ /$stroke/) {
          die "No constraint found for $stroke!\n" unless exists $stroke_constraint{$stroke};
          print join(" ", map { "$_.Y" } @a), "\t", join(" ", split /\s*/, $stroke_constraint{$stroke});
      } elsif ($stroke =~ /^[0qwrtypsdfghjklzxcvbnm]$/) {   # 待映射的声母的韵母约束
          print join(" ", map { "$_.Y" } @a), "\t", join(" ", split /\s*/, $ENV{B_AREA_KEYS});
      } else {
          die "No mapping found for $stroke!\n" unless exists $stroke_mapping{$stroke};
          print join("\n", map { "$_.Y\t$stroke_mapping{$stroke}" } @a);
      }
    }
  }
' "$ROOTS_TXT" >> input-fixed.txt


echo '(10) 添加码灵输入文件 input-fixed.txt, 飞键约束 ...'
perl -CSDA -lanE '
  next if /^\s*#/ || /^\s*#/;
  print "$F[0].U\t", "a e u i o";
' roots-fly.txt >> input-fixed.txt


echo '(11) 生成空的 input-roots.txt, 所有字根已定义于 input-fixed.txt ...'
: > input-roots.txt


echo '(12) 生成码灵输入文件 input-division.txt ...'
perl -CSDA -F'\t' -Mautodie -Mutf8 -lanE '
  BEGIN {
    open my $fh, $ENV{ROOTS_TXT};
    while (<$fh>) {
      chomp;
      @a = split;
      $h{$a[0]} = $a[1];
    }
    undef $fh;

    open $fh, "roots-fly.txt";
    while (<$fh>) {
      next if /^\s*#/ || /^\s*#/;
      chomp;
      s/\s+//g;
      $h2{$_} = 1;
    }
  }

  @a = split /\s+/, $F[1];
  @b = ();

  for (@a) { die "Bad root $_\n" unless exists $h{$_} || exists $h2{$_}; }

  if ($ENV{ENCODE_RULE} eq "yuling") {          # 使用宇浩灵明单字编码规则
    push @b, "$a[0].A";
    push @b, "$a[0].S" if length($h{$a[0]}) > 1;
    push @b, "$a[0].Y" if @a == 1;

    if (@a > 1) {
      for ($i = 1; $i < @a; ++$i) {
        next if @a > 3 && $i == 2 && length($h{$a[0]}) > 1;
        push @b, "$a[$i].A";
      }

      push @b, "$a[-1].S" if length($h{$a[-1]}) > 1;
      push @b, "$a[-1].Y";
    }
  } elsif ($ENV{ENCODE_RULE} eq "xiaoming") {   # 使用潇明单字编码规则
    push @b, "$a[0].A", "$a[0].S";
    if (@a == 1) {
        push @b, "$a[0].Y";
    } elsif (@a == 2) {
        push @b, "$a[1].A", "$a[1].S";
    } elsif (@a == 3) {
        push @b, "$a[1].A", "$a[2].A", "$a[2].S";
    } else {
        push @b, "$a[1].A", "$a[2].A", "$a[-1].A";
    }
  } elsif ($ENV{ENCODE_RULE} eq "moqing") {    # 使用魔卿单字编码规则
    if (@a == 1) {
        push @b, "$a[0].A", "$a[0].S", "$a[0].S";
    } elsif (@a == 2) {
        push @b, "$a[0].A", "$a[1].A", "$a[1].S";
    } else {
        push @b, "$a[0].A", "$a[1].A", "$a[-1].A";
    }
  } else {                                      # 使用魔灵单字编码规则
    for (@a) { push @b, "$_.A" }
    $b[0] = "$a[0].U" if exists $h2{$a[0]};
    if (@a == 2) {
      # 回头码: A1A2S2S1Y1
      $a = $a[-1];
      push @b, "$a.S" if length($h{$a}) > 1;
      $a = $a[0];
    } else {
      $a = $a[-1];
    }
    push @b, "$a.S" if length($h{$a}) > 1;
    push @b, "$a.Y";
  }

  @b = @b[0 .. ($ENV{MAX_CODE_LEN} - 1)] if @b > $ENV{MAX_CODE_LEN};

  print "$F[0]\t", join(" ", @b), "\t$F[2]";
' chaifen.txt > input-division.txt

CONFIG_TOML=config.toml
[ -f "config-$SCHEMA.toml" ] && CONFIG_TOML="config-$SCHEMA.toml"
if sed -ne '/^\s*\[weights.simple_code\]/,/^\s*\[/p' "$CONFIG_TOML" | grep -qi '^\s*enabled\s*=\s*true'; then
    echo '(13) 替换 input-fixed.txt 和 input-division.txt 中的 .A, .S, .Y  为 .0, .1, .2 以让码灵能计算简码 ...'
    perl -CSDA -i -lpE 's/(\S)\.A/\1.0/g;  s/(\S)\.S/\1.1/g;  s/(\S)\.Y/\1.2/g' input-fixed.txt input-division.txt
fi
