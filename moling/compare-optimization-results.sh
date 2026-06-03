#!/usr/bin/env bash

set -euo pipefail
shopt -s failglob

which tabulate >/dev/null && TABULATE="tabulate -f plain" || TABULATE=cat

: "${YUHAO_ASSESS:=../../yuhao-assess}"

[ -f "$YUHAO_ASSESS/src/cli/index.ts" ] || YUHAO_ASSESS=../../yuhao-assess/src
[ -f "$YUHAO_ASSESS/src/cli/index.ts" ] || YUHAO_ASSESS=../../yuhao-assess/cli
[ -f "$YUHAO_ASSESS/src/cli/index.ts" ] || YUHAO_ASSESS=yuhao-assess
[ -f "$YUHAO_ASSESS/src/cli/index.ts" ] || YUHAO_ASSESS=.
[ -f "$YUHAO_ASSESS/src/cli/index.ts" ] || {
    echo "Cannot find yuhao-assess. Please set YUHAO_ASSESS environment variable." >&2
    echo "You may get it from https://github.com/Dieken/yuhao-assess/tree/cli" >&2
    exit 1
}

for f in */thread*/output-combined.txt; do
    d=$(dirname $f)
    [ -f $d/evaluation.txt ] && continue
    date
    echo $f
    npm --prefix="$YUHAO_ASSESS" run cli -- --scheme moling.jsonc $f --format=table --output $d/evaluation.txt
    echo
done

for f in */*/evaluation.txt; do
    perl -CSDA -MFile::Basename -Mutf8 -lnE '
        @F = split /\t/, $_, 2;
        $h{$F[0]} = $F[1];
        END {
            print join("\t",
                       dirname($ARGV),
                       map { /([^\.]+\.[^\.]+)$/ && "$1 $h{$_}" } qw(
                           測評結果.靜態重碼分析.通用規範.全碼重碼字數
                           測評結果.靜態重碼分析.常用國字.全碼重碼字數
                           測評結果.動態選重分析.北語簡體動態選重率.全碼
                           測評結果.動態選重分析.知乎簡體動態選重率.全碼
                           測評結果.動態選重分析.繁簡聯合動態選重率.全碼
                           測評結果.動態選重分析.臺標繁體動態選重率.全碼
                           測評結果.動態選重分析.北語簡體動態選重率原序.全碼
                           測評結果.動態選重分析.知乎簡體動態選重率原序.全碼
                           測評結果.動態選重分析.繁簡聯合動態選重率原序.全碼
                           測評結果.動態選重分析.臺標繁體動態選重率原序.全碼
                           測評結果.速度當量分析.北語簡體字頻全碼速度當量
                           測評結果.速度當量分析.知乎簡體字頻全碼速度當量
                           測評結果.速度當量分析.繁簡聯合字頻全碼速度當量
                           測評結果.速度當量分析.臺標繁體字頻全碼速度當量
                       )
                   );
        }' $f
done > all-results.txt

perl -CSDA -lanE 'print "$F[0] @F[1..4] @F[13..28]" if
     $F[14] <= 0.00031 && $F[16] <= 0.00028 &&
     $F[18] < 0.0020 && $F[20] < 0.0020 &&
     $F[22] < 1.270 && $F[24] < 1.270' all-results.txt |
    perl -CSDA -lanE 'print "@F[0, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20]"' | sort -k4,4n | $TABULATE
