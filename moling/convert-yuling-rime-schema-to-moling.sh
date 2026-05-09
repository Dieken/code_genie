#!/usr/bin/env bash

set -euo pipefail

YU="${1:-靈明輸入法_v3.12.0-beta.20260410.105121}"
MO="${2:-output-20260506-000635}"

echo "使用灵明方案 \"$YU\" 和魔灵码表 \"$MO\""

[ -d "$YU/schema" -a -e "$MO/output-combined.txt" ] || {
    echo "ERROR: 指定目录错误！"
    echo
    echo "Usage: $0 靈明輸入法RIME方案目录 魔灵算码输出目录"
    exit 1
}

echo "(1) 确保魔灵码表存在"
[ -e "$MO/mabiao.tsv" ] || ./generate-root-chart.sh "$MO"

echo "(2) 重命名靈明文件"
find "$YU" -name 'yuling*' | while read f; do
    f2="$(dirname $f)/$(basename $f | sed -e 's/yuling/moling/g')"
    echo "Renaming $f to $f2 ..."
    mv $f $f2
done

echo "(3) 替换文件中的「靈明」和「yuling」字样"
perl -CSDA -Mutf8 -i -pE 's/yuling/moling/g; s/(宇浩.*)?靈明/魔靈/g' $(find "$YU" -name 'moling*') \
    "$YU"/schema/default.custom.yaml "$YU"/readme.txt

echo "(4) 删除五灵方案"
rm -f "$YU"/schema/moling_extreme* "$YU"/schema/yuhao/moling.five*

echo "(5) 替换拆分表 moling_chaifen*.dict.yaml"
for s in chaifen chaifen_tw; do
    f="$s-all.txt"
    [ -f "$f" ] && MO="$MO" CHAIFEN="$f" perl -CSDA -Mutf8 -Mautodie -F'\t' -i -lanE '
        BEGIN {
            open $fh, "$ENV{MO}/roots.tsv";
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
        $code = substr($code, 0, 4) if length($code) > 4;

        print "$F[0]\t[",
            join("", @b), ",",
            $code, ",",
            join("-", map { $roots{$_} } @b), ",",
            join(",", @a[3 .. $#a]), "]";
    ' "$YU/schema/moling_$s.dict.yaml"
done

echo "(6) 生成 moling.full.dict.yaml"
perl -CSDA -lnE 'print; if (/^\.\.\./) { print ""; exit 0 }' "$YU/schema/yuhao/moling.full.dict.yaml" > "$YU/schema/yuhao/moling.full.dict.yaml.new"
mv "$YU/schema/yuhao/moling.full.dict.yaml.new" "$YU/schema/yuhao/moling.full.dict.yaml"
chaifens=chaifen-all.txt
[ -f chaifen_tw-all.txt ] && chaifens="$chaifens chaifen_tw-all.txt"
MO="$MO" perl -CSDA -Mutf8 -Mautodie -F'\t' -lanE '
    BEGIN {
        open $fh, "$ENV{MO}/roots.tsv";
        while (<$fh>) {
            chomp;
            my @a = split;
            $roots{$a[0]} = lc($a[1]);
        }
        undef $fh;
    }

    @b = split /\s+/, $F[1];
    $code = "";
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
    $code = substr($code, 0, 4) if length($code) > 4;

    next if exists $h{"$F[0]$code"};
    $h{"$F[0]$code"} = 1;

    print "$F[0]\t$code\t$F[2]";
' $chaifens | LC_ALL=C sort -s -k3,3nr >> "$YU/schema/yuhao/moling.full.dict.yaml"

echo "(7) 生成 moling.pop.dict.yaml"
perl -CSDA -lnE 'print; if (/^\.\.\./) { print ""; exit 0 }' "$YU/schema/yuhao/moling.pop.dict.yaml" > "$YU/schema/yuhao/moling.pop.dict.yaml.new"
mv "$YU/schema/yuhao/moling.pop.dict.yaml.new" "$YU/schema/yuhao/moling.pop.dict.yaml"
# 假设了「的」的全码四码在全码表开头
perl -CSDA -Mutf8 -lanE 'exit(0) if length($F[1]) == 4; print if /[aeuio]$/' "$MO/mabiao.tsv" >> "$YU/schema/yuhao/moling.pop.dict.yaml"

echo "(8) 生成 moling.quick.dict.yaml"
perl -CSDA -lnE 'print; if (/^\.\.\./) { print ""; exit 0 }' "$YU/schema/yuhao/moling.quick.dict.yaml" > "$YU/schema/yuhao/moling.quick.dict.yaml.new"
mv "$YU/schema/yuhao/moling.quick.dict.yaml.new" "$YU/schema/yuhao/moling.quick.dict.yaml"

echo "(9) 生成 moling.roots.dict.yaml"
perl -CSDA -lnE 'print; if (/^\.\.\./) { print ""; exit 0 }' "$YU/schema/yuhao/moling.roots.dict.yaml" > "$YU/schema/yuhao/moling.roots.dict.yaml.new"
mv "$YU/schema/yuhao/moling.roots.dict.yaml.new" "$YU/schema/yuhao/moling.roots.dict.yaml"
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
' "$MO/zigen-moling.csv" >> "$YU/schema/yuhao/moling.roots.dict.yaml"

echo "(10) 替换 moling*words*.dict.yaml"
MO="$MO" perl -CSDA -Mutf8 -Mautodie -F'\t' -i -lanE '
    BEGIN {
        open $fh, "$ENV{MO}/roots.tsv";
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
        if (length($codes{$a[0]}) == 2) {
            warn "Ignore word $F[0] because full code of $a[0] is two letters.\n";
            next;
        }

        print "$F[0]\t", substr($codes{$a[0]}, 0, 2), substr($codes{$a[1]}, 0, 2);
    } elsif (@a == 3) {
        print "$F[0]\t", substr($codes{$a[0]}, 0, 1), substr($codes{$a[1]}, 0, 1), substr($codes{$a[2]}, 0, 2);
    } elsif (@a >= 4) {
        print "$F[0]\t", substr($codes{$a[0]}, 0, 1), substr($codes{$a[1]}, 0, 1), substr($codes{$a[2]}, 0, 1), substr($codes{$a[-1]}, 0, 1);
    }
' "$YU"/schema/yuhao/moling*words*.dict.yaml

echo "(11) 生成 mabiao/*/*.txt"
rm -f "$YU"/mabiao/*/*.txt

grep -h '\t' "$YU"/schema/yuhao/moling.{quick,pop,full}.dict.yaml \
          "$YU"/schema/yuhao/moling{_sc.words_essence,.words_essence,_sc.words,_tc.words}.dict.yaml \
          "$YU"/schema/yuhao/yuhao.symbols.dict.yaml > "$YU/mabiao/chartab/魔靈.txt"

perl -CSDA -lanE 'print "$F[1] $F[0]"' "$YU/mabiao/chartab/魔靈.txt" > "$YU/mabiao/baidu/魔靈.txt"
perl -CSDA -lanE 'print "$F[1]\t$F[0]"' "$YU/mabiao/chartab/魔靈.txt" > "$YU/mabiao/dazhu/魔靈.txt"
cp "$YU/mabiao/dazhu/魔靈.txt" "$YU/mabiao/duoduo/魔靈.txt"
