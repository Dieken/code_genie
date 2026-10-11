#!/usr/bin/env bash

#
# 生成全码表，批量测评，转换结果到 TSV，可以用 DuckDB 查询
#
# Usage:
#   ./evaluate-xiaoming-mabiao-to-tsv.sh xiaoming-output/output-* > all-ceping.tsv
#
if [ "${SHOW_DUCKDB_SQL:-}" ]; then
    cat <<END
SELECT min(#1) thread,

-- equivalence: simplified 1 or 2, mixed, traditional by full code
#40 es1f,
#48 es2f,
#52 emf,
#56 etf,

-- equivalence: simplified 1 or 2, mixed, traditional by simplified code
#39 es1s,
#47 es2s,
#51 ems,
#55 ets,

-- collision rate: simplified 1 or 2, mixed, traditional by full code
#19 rs1f,
#27 rs2f,
#31 rmf,
#35 rtf,

-- collision rate: simplified 1 or 2, mixed, traditional by simplified code
#20 rs1s,
#28 rs2s,
#32 rms,
#36 rts,

-- collision count: simplified, traditional by full code
#215 ctf,
#222 csf,

-- collision count: simplified, traditional by simplified code
#220 cts,
#227 css,

-- key hot pressure, by full code
#69 a,
#70 b,
#71 c,
#72 d,
#73 e,
#74 f,
#75 g,
#76 h,
#77 i,
#78 j,
#79 k,
#80 l,
#81 m,
#82 n,
#83 o,
#84 p,
#85 q,
#86 r,
#87 s,
#88 t,
#89 u,
#90 v,
#91 w,
#92 x,
#93 y,

-- key hot pressure, by simplified code
#105 a0,
#106 b0,
#107 c0,
#108 d0,
#109 e0,
#110 f0,
#111 g0,
#112 h0,
#113 i0,
#114 j0,
#115 k0,
#116 l0,
#117 m0,
#118 n0,
#119 o0,
#120 p0,
#121 q0,
#122 r0,
#123 s0,
#125 t0,
#126 u0,
#127 v0,
#128 w0,
#129 x0,
#130 y0

FROM read_csv('all-ceping.tsv', header=true, delim='\t')
WHERE es1f <= 1.24 AND es2f <= 1.24 AND a <= 0.04 AND g >= 0.03 AND p <= 0.025 AND s >= 0.035
GROUP BY ALl;
ORDER BY a, p, q, g DESC, b, y, c
END
    exit 0
fi


set -euo pipefail

mabiao() {
    local output=$(realpath "$1")

    for thread in $output/thread-*; do
        echo "$thread/mabiao.tsv" >&2
        [ -f "$thread/mabiao.tsv" ] || SCHEMA=xiaoming ./generate-root-chart.sh "$thread"
    done
}

ceping() {
    local output=$(realpath "$1")

    for thread in $output/thread-*; do
        echo "$thread/ceping.txt" >&2
        [ -f "$thread/ceping.txt" ] ||
            pnpm --dir "$YUHAO_ASSESS" exec tsx src/cli/index.ts \
                "$thread/mabiao.tsv"  \
                --scheme $(realpath "$YUHAO_ASSESS_DATA"/schemes/xiaoming.json) \
                --format table --output "$thread/ceping.txt"
    done
}


[ "${1:-}" ] || {
    echo "Usage: $0 xiaoming-output/output-*" >&2
    exit 1
}

: "${YUHAO_ASSESS:=../../yuhao-assess}"
: "${YUHAO_ASSESS_DATA:=../../yuhao-assess-data}"

[ -f "$YUHAO_ASSESS/src/cli/index.ts" ] || YUHAO_ASSESS=../../yuhao-assess/src
[ -f "$YUHAO_ASSESS/src/cli/index.ts" ] || YUHAO_ASSESS=yuhao-assess
[ -f "$YUHAO_ASSESS/src/cli/index.ts" ] || YUHAO_ASSESS=.
[ -f "$YUHAO_ASSESS/src/cli/index.ts" ] || {
    echo "Cannot find yuhao-assess. Please set YUHAO_ASSESS environment variable." >&2
    echo "You may download it from https://github.com/forfudan/yuhao-assess" >&2
    exit 1
}

[ -d "$YUHAO_ASSESS_DATA/schemes" ] || YUHAO_ASSESS_DATA=../../yuhao-assess-data/src
[ -d "$YUHAO_ASSESS_DATA/schemes" ] || YUHAO_ASSESS_DATA=yuhao-assess-data
[ -d "$YUHAO_ASSESS_DATA/schemes" ] || YUHAO_ASSESS_DATA=.
[ -d "$YUHAO_ASSESS_DATA/schemes" ] || {
    echo "Cannot find yuhao-assess-data. Please set YUHAO_ASSESS_DATA environment variable." >&2
    echo "You may download it from https://github.com/forfudan/yuhao-assess-data" >&2
    exit 1
}

for output in "$@"; do
    mabiao "$output"
    ceping "$output"
done 1>&2

for output in "$@"; do
    grep -E '^測評結果.*\t[0-9.]+$' $output/thread-*/ceping.txt
done |
    sed -E 's/.*\/?output-//; s/測評結果\.//' |
    perl -CSDA -Mutf8 -nE '
        chomp;
        @F = split /\/ceping.txt:|\t/;
        $h{$F[0]}{$F[1]} = $F[2];
        $columns{$F[1]}=1;
        END {
            $n=1;
            print "$n.thread\t", join("\t", map { ++$n; "$n.$_" } sort keys %columns), "\n";
            for my $thread (sort keys %h) {
                $n1 = 0;
                $n2 = 0;

                for my $col (keys %columns) {
                    if ($col =~ /鍵位熱力\.全碼\./) {
                        $n1 += $h{$thread}{$col};
                    } elsif ($col =~ /鍵位熱力\.簡碼\./) {
                        $n2 += $h{$thread}{$col};
                    }
                }

                for my $col (keys %columns) {
                    if ($col =~ /鍵位熱力\.全碼\./) {
                        $h{$thread}{$col} /= $n1;
                    } elsif ($col =~ /鍵位熱力\.簡碼\./) {
                        $h{$thread}{$col} /= $n2;
                    }
                }

                print "$thread";
                for my $col (sort keys %columns) {
                    my $val = $h{$thread}{$col} // 0;

                    if ($col =~ /動態選重率/) {
                        $val *= 100;
                    }

                    $val = sprintf "%.3f", $val if $val =~ /\./;
                    print "\t", $val;
                }
                print "\n";
            }
        }'

