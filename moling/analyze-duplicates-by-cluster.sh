#!/usr/bin/env bash
#
# 评估 roots-cluster.txt 中每一行聚类单独可能带来的重码
#

set -euo pipefail
shopt -s failglob

no_cluster=$(./analyze-duplicates-by-cluster.pl --cluster "" |
    grep -E '动重|静重|组数' |
    sed -e 's/:/\t/' |
    LC_ALL=C sort)

grep -v '^#' roots-cluster.txt |
    grep -v '^\s*$' |
    sed -e 's/\t.*//' |
    while read cluster; do
        echo -ne "$cluster\t";
        join -1 1 -2 1 -t $'\t' \
            <(echo "$no_cluster") \
            <(./analyze-duplicates-by-cluster.pl --cluster "$cluster" |
              grep -E '动重|静重|组数' |
              sed -e 's/:/\t/' |
              LC_ALL=C sort) |
        sed -e 's/%//g; s/ //g' |
        perl -CSDA -Mutf8 -lanE '
            $h{$F[0]} = $F[2] - $F[1];
            END {
                print join "\t", map {
                    @a = $_ =~ /(\d).(.)/;
                    $_ =~ /动/ ? sprintf("%s:\t%.4f", "$a[1]$a[0]", $h{$_})
                               : "$a[1]$a[0]:\t$h{$_}";
                    } sort keys %h;
            }';
    done |
    sort -t $'\t' -k3,3n   -k7,7n \
                  -k9,9n   -k13,13n \
                  -k15,15n -k19,19n \
                  -k21,21n -k25,25n \
                  -k27,27n -k31,31n
