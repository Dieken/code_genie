#!/usr/bin/env bash

set -euo pipefail

YULING="${1:-靈明輸入法_v3.12.0-beta.20260410.105121}"
MOLING="${2:-output-20260506-000635}"
YUSTAR="${3:-星陳輸入法_v3.11.0}"

. ./init.sh

echo "使用灵明方案 \"$YULING\"、星陈方案 \"$YUSTAR\" 和魔灵码表 \"$MOLING\""

[ -d "$YULING/schema" -a -e "$MOLING/output-combined.txt" ] || {
    echo "ERROR: 指定目录错误！"
    echo
    echo "Usage: $0 靈明輸入法RIME方案目录 魔灵算码输出目录 [星陳輸入法RIME方案目录]"
    exit 1
}

echo "(1) 确保魔灵码表存在"
[ -e "$MOLING/mabiao.tsv" ] || ./generate-root-chart.sh "$MOLING"

echo "(2) 重命名靈明文件"
find "$YULING" -name 'yuling*' | while read f; do
    f2="$(dirname $f)/$(basename $f | sed -e 's/yuling/moling/g')"
    echo "Renaming $f to $f2 ..."
    mv $f $f2
done

echo "(3) 替换文件中的「靈明」和「yuling」字样"
perl -CSDA -Mutf8 -i -pE 's/yuling/moling/g; s/(宇浩.*)?靈明/魔靈/g' $(find "$YULING" -name 'moling*') \
    "$YULING"/schema/default.custom.yaml "$YULING"/readme.txt

echo "(4) 删除五灵方案"
rm -f "$YULING"/schema/moling_extreme* "$YULING"/schema/yuhao/moling.five*

echo "(5) 替换拆分表 moling_chaifen*.dict.yaml"
for s in chaifen chaifen_tw; do
    f="$s-all.txt"
    [ -f "$f" ] && MOLING="$MOLING" CHAIFEN="$f" perl -CSDA -Mutf8 -Mautodie -F'\t' -i -lanE '
        BEGIN {
            open $fh, "$ENV{MOLING}/roots.tsv";
            while (<$fh>) {
                chomp;
                my @a = split;
                $roots{$a[0]} = $a[1];
            }
            undef $fh;

            open $fh, $ENV{CHAIFEN};
            while (<$fh>) {
                chomp;
                my @a = split /\t/;
                my @b = split /\s+/, $a[1];
                $chaifen{$a[0]} = \@b;
            }
        }

        if ($F[1] !~ /^\[/) {
            print;
            next;
        }

        $F[1] =~ s/[\[\]]//g;
        @a = split /,/, $F[1];
        if (!$a[0]) {   # no chaifen
            print;
            next;
        }

        if (! exists $chaifen{$F[0]}) {
            warn "WARN: unknown chaifen $_\n";
            next;
        }

        @b = @{ $chaifen{$F[0]} };
        $code = "";

        if ($ENV{ENCODE_RULE} eq "yuling") {    # 使用宇浩灵明单字编码规则
            for (@b) { die "Unknown root $_\n" unless exists $roots{$_}; }

            $code .= substr($roots{$b[0]}, 0, 1);
            $code .= substr($roots{$b[0]}, 1, 1) if length($roots{$b[0]}) > 2;
            $code .= substr($roots{$b[0]}, -1) if @b == 1;

            if (@b > 1) {
                for ($i = 1; $i < @b; ++$i) {
                    next if @b > 3 && $i == 2 && length($roots{$b[0]}) > 2;
                    $code .= substr($roots{$b[$i]}, 0, 1);
                }

                $code .= substr($roots{$b[-1]}, 1, 1) if length($roots{$b[-1]}) > 2;
                $code .= substr($roots{$b[-1]}, -1);
            }
        } else {                                # 使用魔灵单字编码规则
            for (@b) {
                die "Unknown root $_\n" unless exists $roots{$_};
                $code .= substr($roots{$_}, 0, 1);
            }
            $root = $b[-1];
            if (@b == 2) {
                $code .= substr($roots{$root}, 1, 1) if length($roots{$root}) > 2;
                $root = $b[0];
            }
            $code .= substr($roots{$root}, 1);
        }
        $code = substr($code, 0, 4) if length($code) > 4;

        print "$F[0]\t[",
            join("", @b), ",",
            $code, ",",
            join("-", map { $roots{$_} } @b), ",",
            join(",", @a[3 .. $#a]), "]";
    ' "$YULING/schema/moling_$s.dict.yaml"
done

echo "(6) 生成 moling.full.dict.yaml"
perl -CSDA -lnE 'print; if (/^\.\.\./) { print ""; exit 0 }' "$YULING/schema/yuhao/moling.full.dict.yaml" > "$YULING/schema/yuhao/moling.full.dict.yaml.new"
mv "$YULING/schema/yuhao/moling.full.dict.yaml.new" "$YULING/schema/yuhao/moling.full.dict.yaml"
chaifens=chaifen-all.txt
[ -f chaifen_tw-all.txt ] && chaifens="$chaifens chaifen_tw-all.txt"
MOLING="$MOLING" perl -CSDA -Mutf8 -Mautodie -F'\t' -lanE '
    BEGIN {
        open $fh, "$ENV{MOLING}/roots.tsv";
        while (<$fh>) {
            chomp;
            my @a = split;
            $roots{$a[0]} = lc($a[1]);
        }
        undef $fh;
    }

    @b = split /\s+/, $F[1];
    $code = "";

    if ($ENV{ENCODE_RULE} eq "yuling") {    # 使用宇浩灵明单字编码规则
        for (@b) { die "Unknown root $_\n" unless exists $roots{$_}; }

        $code .= substr($roots{$b[0]}, 0, 1);
        $code .= substr($roots{$b[0]}, 1, 1) if length($roots{$b[0]}) > 2;
        $code .= substr($roots{$b[0]}, -1) if @b == 1;

        if (@b > 1) {
            for ($i = 1; $i < @b; ++$i) {
                next if @b > 3 && $i == 2 && length($roots{$b[0]}) > 2;
                $code .= substr($roots{$b[$i]}, 0, 1);
            }

            $code .= substr($roots{$b[-1]}, 1, 1) if length($roots{$b[-1]}) > 2;
            $code .= substr($roots{$b[-1]}, -1);
        }
    } else {                                # 使用魔灵单字编码规则
        for (@b) {
            die "Unknown root $_\n" unless exists $roots{$_};
            $code .= substr($roots{$_}, 0, 1);
        }
        $root = $b[-1];
        if (@b == 2) {
            $code .= substr($roots{$root}, 1, 1) if length($roots{$root}) > 2;
            $root = $b[0];
        }
        $code .= substr($roots{$root}, 1);
    }
    $code = substr($code, 0, 4) if length($code) > 4;

    next if exists $h{"$F[0]$code"};
    $h{"$F[0]$code"} = 1;

    print "$F[0]\t$code\t$F[2]";
' $chaifens | LC_ALL=C sort -s -k3,3nr >> "$YULING/schema/yuhao/moling.full.dict.yaml"

echo "(7) 生成 moling.pop.dict.yaml"
perl -CSDA -lnE 'print; if (/^\.\.\./) { print ""; exit 0 }' "$YULING/schema/yuhao/moling.pop.dict.yaml" > "$YULING/schema/yuhao/moling.pop.dict.yaml.new"
mv "$YULING/schema/yuhao/moling.pop.dict.yaml.new" "$YULING/schema/yuhao/moling.pop.dict.yaml"
# 假设了「的」的全码四码在全码表开头
perl -CSDA -Mutf8 -lanE 'exit(0) if length($F[1]) >= 4; print if /[aeuio]$/' "$MOLING/mabiao.tsv" >> "$YULING/schema/yuhao/moling.pop.dict.yaml"

echo "(8) 生成 moling.roots.dict.yaml"
perl -CSDA -lnE 'print; if (/^\.\.\./) { print ""; exit 0 }' "$YULING/schema/yuhao/moling.roots.dict.yaml" > "$YULING/schema/yuhao/moling.roots.dict.yaml.new"
mv "$YULING/schema/yuhao/moling.roots.dict.yaml.new" "$YULING/schema/yuhao/moling.roots.dict.yaml"
perl -CSDA -Mutf8 -F, -lanE '
    next if $. == 1;
    push @{ $h{substr($F[1], 0, 1)} }, $F[0];
    push @{ $h2{substr($F[1], 0, 1)}{substr($F[1], 1)} }, $F[0];
    END {
        print "魔靈字根編碼提示\t/ml";
        print "輸入對應大碼字母\t/ml";
        for (sort keys %h) {
            print join("", @{ $h{$_} }), "\t/ml$_";
        }

        for $a (sort keys %h2) {
            for (sort keys %{ $h2{$a} }) {
                print "+ $_ = ", join("", @{ $h2{$a}{$_} }), "\t/ml$a";
            }
        }
    }
' "$MOLING/zigen-moling.csv" >> "$YULING/schema/yuhao/moling.roots.dict.yaml"

echo "(9) 替换 moling*words*.dict.yaml"
if [ -d "$YUSTAR/schema" ]; then
    echo "使用星陈方案的词库"

    cp "$YUSTAR/schema/yuhao/yustar.words.dict.yaml" "$YULING/schema/yuhao/moling.words.dict.yaml"
    cp "$YUSTAR/schema/yuhao/yustar_sc.words.dict.yaml" "$YULING/schema/yuhao/moling_sc.words.dict.yaml"
    cp "$YUSTAR/schema/yuhao/yustar_tc.words.dict.yaml" "$YULING/schema/yuhao/moling_tc.words.dict.yaml"

    perl -CSDA -i -lpE 's/yustar((?:_[st]c)?\.words)/moling\1/' "$YULING"/schema/yuhao/moling{,_sc,_tc}.words.dict.yaml

    grep -Eq 'yuhao/moling.words\s*$' "$YULING/schema/moling.dict.yaml" ||
        perl -CSDA -i -lnE 'if (/yuhao\/moling_sc\.words\s*$/) { print "  - yuhao/moling.words" } print' "$YULING/schema/moling.dict.yaml"
else
    echo "没有发现星陈方案，只使用灵明方案的词库"
fi

MOLING="$MOLING" perl -CSDA -Mutf8 -Mautodie -F'\t' -i -lanE '
    BEGIN {
        open $fh, "$ENV{MOLING}/roots.tsv";
        while (<$fh>) {
            chomp;
            my @a = split;
            $roots{$a[0]} = lc($a[1]);
        }
        undef $fh;

        open $fh, "chaifen-all.txt";
        while (<$fh>) {
            chomp;
            my @a = split /\t/;
            my @b = split /\s+/, $a[1];
            $code = "";

            if ($ENV{ENCODE_RULE} eq "yuling") {    # 使用宇浩灵明单字编码规则
                for (@b) { die "Unknown root $_\n" unless exists $roots{$_}; }

                $chaifen{$a[0]} = \@b;

                $code .= substr($roots{$b[0]}, 0, 1);
                $code .= substr($roots{$b[0]}, 1, 1) if length($roots{$b[0]}) > 2;
                $code .= substr($roots{$b[0]}, -1) if @b == 1;

                if (@b > 1) {
                    for ($i = 1; $i < @b; ++$i) {
                        next if @b > 3 && $i == 2 && length($roots{$b[0]}) > 2;
                        $code .= substr($roots{$b[$i]}, 0, 1);
                    }

                    $code .= substr($roots{$b[-1]}, 1, 1) if length($roots{$b[-1]}) > 2;
                    $code .= substr($roots{$b[-1]}, -1);
                }
            } else {                                # 使用魔灵单字编码规则
                for (@b) {
                    die "Unknown root $_\n" unless exists $roots{$_};
                    $code .= substr($roots{$_}, 0, 1);
                }
                $root = $b[-1];
                if (@b == 2) {
                    $code .= substr($roots{$root}, 1, 1) if length($roots{$root}) > 2;
                    $root = $b[0];
                }
                $code .= substr($roots{$root}, 1);
            }
            $code = substr($code, 0, 4) if length($code) > 4;

            $codes{$a[0]} = $code;
        }
    }

    unless (/^\p{Han}/ && $F[1]) {
        print;
        next;
    }

    @a = split //, $F[0];
    for (@a) { die "Unknown char in $ARGV: $_\n" unless exists $codes{$_}; }

    if (@a == 2) {
        if ($ENV{ENCODE_RULE} eq "yuling" && $ENV{USE_YULING_NEW_WORD_RULE}) {  # 使用宇浩灵明二字词编码规则
            my $code = "";
            my $seq = $chaifen{ $a[0] };
            if (@$seq == 1) {   # 首字是单根字
                $code .= substr($roots{ $seq->[0] }, 0, length($roots{ $seq->[0] }) - 1 );  # 只取 AS，不要 Y
            } else {
                $code .= substr($roots{ $seq->[0] }, 0, 1);     # 首根大码
                $code .= substr($roots{ $seq->[1] }, 0, 1);     # 次根大码
            }

            $seq = $chaifen{ $a[1] };
            $code .= substr($roots{ $seq->[0] }, 0, 1);     # 首根大码
            $code .= substr($roots{ $seq->[1] }, 0, 1) if @$seq > 1;    # 次根大码

            if (@$seq == 3) {       # 三根字
                $code .= substr($roots{ $seq->[2] }, 0, 1); # 三根大码
            } elsif (@$seq > 3) {   # 四根及以上根字，首根是小根，取三根大码，否则取末根大码
                $code .= length($roots{ $seq->[0] }) < 3 ? substr($roots{ $seq->[2] }, 0, 1) : substr($roots{ $seq->[-1] }, 0, 1);
            }

            $code .= substr($roots{ $seq->[-1] }, 1);       # 末根 SY

            print "$F[0]\t", substr($code, 0, 4);
        } else {                                    # 使用魔灵二字词编码规则
            if (length($codes{$a[0]}) == 2) {
                warn "Ignore word $F[0] because full code of $a[0] is two letters.\n";
                next;
            }

            print "$F[0]\t", substr($codes{$a[0]}, 0, 2), substr($codes{$a[1]}, 0, 2);
        }
    } elsif (@a == 3) {
        print "$F[0]\t", substr($codes{$a[0]}, 0, 1), substr($codes{$a[1]}, 0, 1), substr($codes{$a[2]}, 0, 2);
    } elsif (@a >= 4) {
        print "$F[0]\t", substr($codes{$a[0]}, 0, 1), substr($codes{$a[1]}, 0, 1), substr($codes{$a[2]}, 0, 1), substr($codes{$a[-1]}, 0, 1);
    }
' "$YULING"/schema/yuhao/moling*words*.dict.yaml

echo "(10) 生成 moling.quick.dict.yaml"
perl -CSDA -lnE 'print; if (/^\.\.\./) { print ""; exit 0 }' "$YULING/schema/yuhao/moling.quick.dict.yaml" > "$YULING/schema/yuhao/moling.quick.dict.yaml.new"
mv "$YULING/schema/yuhao/moling.quick.dict.yaml.new" "$YULING/schema/yuhao/moling.quick.dict.yaml"

perl -CSDA -lanE '
    next unless $F[0] =~ /^\p{Han}{2,}$/ && $F[1] =~ /^[a-z]+$/;
    $s = substr($F[1], 0, 1);
    $s = substr($F[1], 0, 2) if $shortcodes{$s};
    next if $shortcodes{$s};
    print "$F[0]\t$s";
    $shortcodes{$s} = 1;
    ' \
    $(D="$YULING/schema" perl -lnE 'print "$ENV{D}/$1.dict.yaml" if /^\s+-\s+(yuhao\/(\S*_sc\.words\S*))/ && $2 !~ /private|extended/' "$YULING"/schema/moling.dict.yaml) \
    | LC_ALL=C sort -k2,2 -k1,1 >> "$YULING/schema/yuhao/moling.quick.dict.yaml"

echo "(11) 生成 mabiao/*/*.txt"
rm -f "$YULING"/mabiao/*/*.txt

perl -CSDA -lnE 'next unless /\t/; next if exists $h{$_}; $h{$_} = 1; print' \
    $(D="$YULING/schema" perl -lnE 'print "$ENV{D}/$1.dict.yaml" if /^\s+-\s+(yuhao\/(\S+))/ && $2 !~ /private|extended/' "$YULING"/schema/moling.dict.yaml) \
    > "$YULING/mabiao/chartab/魔靈.txt"

perl -CSDA -lanE 'print "$F[1] $F[0]"' "$YULING/mabiao/chartab/魔靈.txt" > "$YULING/mabiao/baidu/魔靈.txt"
perl -CSDA -lanE 'print "$F[1]\t$F[0]"' "$YULING/mabiao/chartab/魔靈.txt" > "$YULING/mabiao/dazhu/魔靈.txt"
cp "$YULING/mabiao/dazhu/魔靈.txt" "$YULING/mabiao/duoduo/魔靈.txt"
