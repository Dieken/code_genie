#!/usr/bin/env bash

: ${CODE_GENIE:=../target/release/code_genie}
: ${DRYRUN:=false}

[ "${USE_YAOLING_RULE:-}" = 1 ] && export USE_YULING_RULE=1 USE_VOWEL=1
[ "${USE_YUELING_RULE:-}" = 1 ] && export USE_YULING_RULE=1 USE_VOWEL=1

which caffeinate >/dev/null 2>&1 && CAFFEINATE="caffeinate -imsu" || CAFFEINATE=
[ "$DRYRUN" = true ] && DRYRUN=echo || DRYRUN=


read -e -p "一句话备注： " comment
comment="USE_VOWEL=$USE_VOWEL USE_YULING_RULE=$USE_YULING_RULE USE_YAOLING_RULE=$USE_YAOLING_RULE USE_YUELING_RULE=$USE_YUELING_RULE OPTIMIZE_KEYS=$OPTIMIZE_KEYS $0 $@ : $comment"


TS=$(date +%Y%m%d-%H%M%S)
LOG=optimize-$TS.log
BAK=output-source-$TS
mkdir "$BAK"

# backup configuration for later review
cp batch-test-weights.txt \
   chaifen.txt \
   charAbsoluteFrequency*.json \
   chars.txt \
   config.toml \
   freq.txt \
   full-freq.txt \
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

date
echo "Running './prepare-inputs.sh' and 'code_genie optimize', writing log to $LOG ..."
{
    date

    echo '检查优化相关环境变量：---->'
    env | grep -E 'USE_|OPTIMIZE_|ENABLE_|DISABLE_'
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

