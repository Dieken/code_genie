#!/usr/bin/env bash

set -euo pipefail
shopt -s failglob


echo '(1) 从 yuhao_charsets.lua 生成简繁常用字符集 chars.txt ...'
perl -CSDA -lnE 'print if (/\[\[/ .. eof) && /^\p{Han}$/' yuhao_charsets.lua | LC_ALL=C sort -u > chars.txt


echo '(2) 从北语字频 简体字频表-2.5b.txt 生成简繁常用字符集的字频表 freq.txt ...'
perl -CSDA -Mautodie -Mutf8 -lanE 'BEGIN { open my $fh, "chars.txt"; while (<$fh>) {chomp; $h{$_} = 1} }
  next unless defined $F[1] && $F[1] > 0;
  next unless exists $h{$F[0]};
  print "$F[0]\t$F[1]";
  delete $h{$F[0]};
  END {
      @a = sort keys %h;
      warn "    WARN: " .  scalar(@a) . " 个字符没有权重!\n" if @a != 0;
      for (@a) { print "$_\t0" }
  }' 简体字频表-2.5b.txt  > freq.txt


echo '(3) 从宇浩星陈方案的大陆字形拆分表 yustar_chaifen.dict.yaml 生成简繁常用字符集的拆分表 chaifen.txt ...'
perl -CSDA -Mautodie -lanE 'BEGIN { open my $fh, "freq.txt"; while (<$fh>) { chomp; @a = split; $h{$a[0]} = $a[1] } }
  if (! $ok) {
    next unless /^\.\.\./;
    $ok = 1;
  }
  next unless /^(\S)\t\[([^,]+)/;
  next unless exists $h{$1};
  $a = $1;
  @a = $2 =~ /\{[^\}]+\}|\S/g;
  print "$a\t", join(" ", @a), "\t$h{$a}";
  delete $h{$a};
  END { @a = sort keys %h; die "No chaifen found for @a" if @a > 0}
  ' yustar_chaifen.dict.yaml | LC_ALL=C sort -t $'\t' -s -k3,3nr -k1,1 > chaifen.txt


echo '(4) 从 chaifen.txt 生成字根频率表 roots-freq.txt ...'
perl -CSDA -F'\t' -lanE '
  @a = split /\s/, $F[1];
  $n += $F[2] * @a;
  for (@a) { $h{$_} += $F[2]; }
  END {
    for (sort { $h{$b} <=> $h{$a} } keys %h) {
      printf "%s\t%.8f\n", $_, 100 * $h{$_} / $n;
    }
  }' chaifen.txt > roots-freq.txt


echo '(5) 从万象拼音词典 chars.dict.yaml 生成字根读音 roots-pinyin.txt ...'
perl -CSDA -Mautodie -Mutf8 -lanE 'use Unicode::Normalize;
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
    $_ = NFKD($_);
    s/\p{M}//g;
    $h2{$1}=1 if /^([a-z]+)/i;
  }
  for (sort keys %h2) {
    print "$F[0]\t$_\t", ($F[-1] =~ /^\d/ ? $F[-1] : "0");
  }'  chars.dict.yaml | LC_ALL=C sort -u -k1,1 -k3,3nr -k2,2 > roots-pinyin.txt


echo '(6) 从 roots-pinyin.txt 修正并生成字根声码韵码表 roots.txt ...'
perl -CSDA -Mautodie -Mutf8 -F'\t' -lanE '
  BEGIN {
    open my $fh, "roots-pinyin.txt";
    while (<$fh>) {
      @a = split;
      next if exists $h{$a[0]};
      $a[1] = "je" if $a[1] eq "er";  # https://shurufa.app/docs/ling.html#%E4%B8%BA%E4%BB%80%E4%B9%88%E9%9B%B6%E5%A3%B0%E6%AF%8D%E7%9A%84%E5%A3%B0%E7%A0%81%E6%98%AF-j
      $a[1] = "nu" if $a[1] eq "nv";
      die "Invalid pinyin: $_\n" unless $a[1] =~ /^([^aeuio]).*?(?:[iu])?([aeuio])/;
      $h{$a[0]} = "$1$2";
    }
    %fixes = (
      "冫"     => "je",  # bi，与 二 归并
      "⺀"     => "je",  # o, 与 二 归并
      "{飞右}" => "je",  # o, 与 二 归并
      "屮"     => "ca",  # ce，取 cao
      "丶"     => "da",  # zu, 取 dian
      "乀"     => "da",  # fu, 与 丶 归并
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
      "{荒下}" => "je",  # o, 与 儿 归并
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
    );
  }
  if (exists $fixes{$F[0]}) {
    $a = $fixes{$F[0]};
  } elsif (exists $h{$F[0]}) {
    $a = $h{$F[0]};
  } else {
    $a = "o";
  }
  $a =~ s/^z/v/;  # https://shurufa.app/docs/ling.html#%E4%B8%BA%E4%BB%80%E4%B9%88%E4%B8%8D%E7%94%A8-z-%E9%94%AE
  $a =~ s/^q/k/;  # https://shurufa.app/docs/ling.html#%E4%B8%BA%E4%BB%80%E4%B9%88%E5%A3%B0%E7%A0%81%E4%B8%8D%E7%94%A8-q-%E9%94%AE
  $a =~ s/^y/d/ unless $ENV{OPTIMIZE_Y};    # 经过多轮优化试探，映射 Y 到 D 最好
  print "$F[0]\t$a";
' roots-freq.txt | LC_ALL=C sort -k2,2 -k1.1 > roots.txt


echo '(7) 分析首根冲突情况，为选择飞键字根提供参考 ...'
perl -CSDA -F'\t' -lanE '
  @a = split /\s/, $F[1];
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
    for (sort { $h2{$b}[4] <=> $h2{$a}[4] } keys %h2) {
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
            }
            $s = sprintf "$s %s/%.2f/%d/%d/%.2f/%.2f", $p, $a, @{ $freq{$p} }[0, 1], 100 * $freq{$p}[2] / $n, 100 * $freq{$p}[3] / $n;
            $s .= " ?? " . join(" ", map { sprintf "%s/%.2f", $_, 100 * $h3{$_} / $n } sort keys %{ $h5{$p} }) if exists $h5{$p};    # 之前高冲突的字根又被选上了
          } else {
            if (! exists $h5{$q} || scalar keys %{ $h5{$q} } <= 4) {  # AEUIO 只能容纳 5 个互相冲突的根
              $s = "SELECT";
              $h4{$q} = 1;
              $h5{$p}{$q} = 1 unless exists $h4{$p};
            }
            $s = sprintf "$s %s/%.2f/%d/%d/%.2f/%.2f", $q, $b, @{ $freq{$q} }[0, 1], 100 * $freq{$q}[2] / $n, 100 * $freq{$q}[3] / $n;
            $s .= " ?? " . join(" ", map { sprintf "%s/%.2f", $_, 100 * $h3{$_} / $n } sort keys %{ $h5{$q} }) if exists $h5{$q};    # 之前高冲突的字根又被选上了
          }
      }

      printf "%s\t%.2f\t%s\t%.2f\t%.2f (%.2f : %.2f) ## %s\n", @{ $h2{$_} }, $a, $b, $s;
    }
  }' chaifen.txt | grep SELECT | head -n 50 | cat -n || true  # ignore SIGPIPE


echo '(8) 生成码灵输入文件 input-fixed.txt, 大码约束 ...'
perl -CSDA -F'\t' -Mautodie -Mutf8 -MList::Util=sum -lanE '
  BEGIN {
    open my $fh, "roots-freq.txt";
    while (<$fh>) {
      chomp;
      @a = split;
      $freq{$a[0]} = $a[1];
    }
    undef $h;

    open $fh, "roots-cluster.txt";
    while (<$fh>) {
      next if /^\s*#/ || /^\s*$/;
      chomp;
      @a = split /\t/, $_, 2;
      @b = sort split /\s+/, $a[0];
      $a = sum(map { $freq{$_} } @b);
      if ($a >= 1.8) {
        $a[1] ||= "sdfghjkl";
      } elsif ($a >= 1) {
        $a[1] ||= "wr sdfghjkl vnm";
      } else {
        $a[1] ||= "qwrtyp sdfghjkl xcvbnm";
      }
      $a[1] = join(" ", split /\s*/, $a[1]);
      printf "# freq=%.8f\n", $a;
      print join(" ", map { "$_.A" } @b), "\t$a[1]";
      for (@b) { $h{$_} = 1 };
    }
  }

  next if $h{$F[0]};
  $a = $freq{$F[0]};
  if ($a >= 1.8) {
    $b = "sdfghjkl";
  } elsif ($a >= 1) {
    $b = "wr sdfghjkl vnm";
  } else {
    $b = "qwrtyp sdfghjkl xcvbnm";
  }
  printf "# freq=%.8f\n", $a;
  print "$F[0].A\t", join(" ", split /\s*/, $b);
' roots.txt > input-fixed.txt


echo '(9) 添加码灵输入文件 input-fixed.txt, 声码和韵码约束 ...'
perl -CSDA -F'\t' -Mautodie -Mutf8 -lanE '
  if (length($F[1]) > 1) {
    if (substr($F[1], 0, 1) eq "y") {
      push @a, $F[0];   # 声母 y 过于高频，需要映射
    } else {
      print "$F[0].S\t", substr($F[1], 0, 1) if length($F[1]) > 1;
    }
  }
  print "$F[0].Y\t", substr($F[1], -1);

  END {
    print join(" ", map { "$_.S" } @a), "\t", join(" ", split /\s*/, "wr sdfghjkl vnm") if @a > 0;
  }
' roots.txt >> input-fixed.txt


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
    open my $fh, "roots.txt";
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
      s/\s//g;
      $h2{$_} = 1;
    }
  }
  @a = split /\s/, $F[1];
  @b = ();
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
  @b = @b[0..3] if @b > 4;
  print "$F[0]\t", join(" ", @b), "\t$F[2]";
' chaifen.txt > input-division.txt

