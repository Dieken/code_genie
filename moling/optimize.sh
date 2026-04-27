#!/usr/bin/env bash

: ${CODE_GENIE:=../target/release/code_genie}
: ${DRYRUN:=false}

which caffeinate >/dev/null && CAFFEINATE="caffeinate -imsu" || CAFFEINATE=
[ "$DRYRUN" = true ] && DRYRUN=echo || DRYRUN=

TS=$(date +%Y%m%d-%H%M%S)
LOG=optimize-$TS.log
BAK=output-source-$TS
mkdir "$BAK"

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
   prepare-inputs.sh \
   roots-cluster.txt \
   roots-fly.txt \
   roots-freq.txt \
   roots-pinyin.txt \
   roots.txt \
   "$BAK/"

echo -n "一句话备注： "
read comment

date
echo "Running './prepare-input.sh' and 'code_genie optimize', writing log to $LOG ..."
{
    date

    echo '检查优化相关环境变量：---->'
    env | grep -E 'USE_VOWEL|OPTIMIZE_'
    echo '<--------------------------'

    [ -e ../.git ] && {
        echo -n "GIT version: "
        git describe || true
        git status
        echo
    }

    set -x
    ./prepare-inputs.sh && time $DRYRUN $CAFFEINATE $CODE_GENIE optimize "$@"
    set +x

    date
} >$LOG 2>&1

OUT=$(grep '^输出目录:' $LOG | sed -e 's/.* //')
[ "$OUT" -a -d "$OUT" ] || {
    echo "ERROR: can't find output directory '$OUT'" >&2
    exit 1
}

echo "$comment" > "$OUT/COMMENT.txt"
mv $LOG "$OUT/"
mv $BAK "$OUT/source"

tail -n 23 "$OUT/$LOG"
echo
cat "$OUT/summary.txt"
echo
date

