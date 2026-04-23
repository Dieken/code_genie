#!/usr/bin/env bash

: ${CODE_GENIE:=../target/release/code_genie}
: ${DRYRUN:=false}

which caffeinate >/dev/null && CAFFEINATE="caffeinate -imsu" || CAFFEINATE=
[ "$DRYRUN" = true ] && DRYRUN=echo || DRYRUN=

LOG=optimize-$(date +%Y%m%d-%H%M%S).log

echo -n "一句话备注： "
read comment

date
echo "Running './prepare-input.sh' and 'code_genie optimize', writing log to $LOG ..."
set -x
{
    date
    ./prepare-inputs.sh && $DRYRUN time $CAFFEINATE $CODE_GENIE "$@" optimize
    date
} >$LOG 2>&1
set +x

OUT=$(grep '^输出目录:' $LOG | sed -e 's/.* //')
[ "$OUT" -a -d "$OUT" ] || {
    echo "ERROR: can't find output directory '$OUT'" >&2
    exit 1
}

echo "$comment" > "$OUT/COMMENT.txt"
mv $LOG "$OUT/"

# backup configuration for later review
cp batch-test-weights.txt \
   chaifen.txt \
   chars.txt \
   config.toml \
   freq.txt \
   input-division.txt \
   input-fixed.txt \
   input-roots.txt \
   key_distribution.txt \
   pair_equivalence.txt \
   roots-cluster.txt \
   roots-fly.txt \
   roots-freq.txt \
   roots-pinyin.txt \
   roots.txt \
   "$OUT/"

tail -n 23 "$OUT/$LOG"
echo
cat "$OUT/summary.txt"
echo
date

