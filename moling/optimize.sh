#!/usr/bin/env bash

set -euo pipefail
shopt -s failglob


: "${PROFILE:=release}"
: "${CODE_GENIE:=../target/$PROFILE/code_genie}"
: "${DRYRUN:=false}"


[ "${USE_YAOLING_RULE:-}" = 1 ] && export USE_YULING_RULE=1 USE_VOWEL=1
[ "${USE_YUELING_RULE:-}" = 1 ] && export USE_YULING_RULE=1 USE_VOWEL=1


which caffeinate >/dev/null 2>&1 && CAFFEINATE="caffeinate -imsu" || CAFFEINATE=
[ "$DRYRUN" = true ] && DRYRUN=echo || DRYRUN=

if [ "$PROFILE" = profiling ]; then
    which samply >/dev/null 2>&1 || cargo install samply
    SAMPLY_RECORD="samply record"
else
    SAMPLY_RECORD=
fi


usage() {
    cat <<EOF
用法:
  $0 [resume] [code_genie 参数...]   运行 code_genie optimize（或 resume）
  $0 help | -h | --help              显示本帮助

子命令:
  (默认)         运行 code_genie optimize
  resume        第一个参数为 resume 时运行 code_genie resume（其余参数透传）

参数:
  -d, --dir, --output-dir <DIR>
                输出目录。未指定时默认 output-<时间戳>，并自动追加 -d <DIR>；
                resume 复用既有 <DIR>。其余参数原样透传给 code_genie。

环境变量:
  CODE_GENIE    code_genie 可执行文件路径（默认 ../target/release/code_genie）
  DRYRUN        =true 时只打印将执行的命令，不真正运行（默认 false）
  NO_PREPARE    =1 时跳过 ./prepare-inputs.sh（默认执行）
  USE_YAOLING_RULE / USE_YUELING_RULE / USE_YULING_RULE / USE_VOWEL / OPTIMIZE_KEYS
                方案相关开关，原样记录到 COMMENT.txt 并影响 prepare-inputs.sh

行为:
  - 输入一句话备注，追加写入 <DIR>/COMMENT.txt（用 >> 保留历史，便于 resume 复用）。
  - 首次写入 <DIR>/source/ 备份输入文件；<DIR>/source 已存在则跳过备份。
  - 日志写入 <DIR>/optimize-<时间戳>.log。

示例:
  $0                        # 全新优化，输出到 output-<时间戳>
  NO_PREPARE=1 $0 -d out1   # 跳过 prepare，输出/复用 out1
  $0 resume -d out1         # 从 out1 断点续算
  $0 --seed-dir out1        # 复用 out1/thread-NN/output-keymap.txt 作为退火初始按键分配
  $0 --seed-keymap out1/output-keymap.txt   # 复用 out1/output-keymap.txt 作为退火初始按键分配
EOF
}


# (0) help：第一个参数为 -h/--help/help 时显示帮助并退出（先于任何副作用）
case "${1:-}" in
    -h|--help|help)
        usage
        exit 0
        ;;
esac


# (1) 子命令：第一个参数为 resume 时运行 `code_genie resume`，否则 `code_genie optimize`
if [ "${1:-}" = resume ]; then
    SUBCMD=resume
    shift
else
    SUBCMD=optimize
fi


# (2) 从参数中解析 -d / --dir / --output-dir 的值；未指定则默认 output-$TS 并追加 -d $OUT
TS=$(date +%Y%m%d-%H%M%S)
ARGS=("$@")
OUT=
i=0
while [ $i -lt ${#ARGS[@]} ]; do
    case "${ARGS[$i]}" in
        -d|--dir|--output-dir)
            OUT="${ARGS[$((i + 1))]}"
            ;;
        --dir=*|--output-dir=*)
            OUT="${ARGS[$i]#*=}"
            ;;
    esac
    i=$((i + 1))
done
if [ -z "$OUT" ]; then
    OUT="output-$TS"
    ARGS+=(-d "$OUT")
fi

mkdir -p "$OUT"


# (3) 备注：输入一句话备注，追加写入 COMMENT.txt；如果 resume 复用 $OUT 则备注会追加写入 COMMENT.txt（而不是覆盖），以保留之前的备注。
read -e -p "一句话备注： " comment
comment="USE_VOWEL=${USE_VOWEL:-} USE_YULING_RULE=${USE_YULING_RULE:-} USE_YAOLING_RULE=${USE_YAOLING_RULE:-} USE_YUELING_RULE=${USE_YUELING_RULE:-} OPTIMIZE_KEYS=${OPTIMIZE_KEYS:-} $0 $SUBCMD $@ : $comment"
echo "$comment" >> "$OUT/COMMENT.txt"


# (4) 备份目录直接用 $OUT/source；已存在（如 resume 复用 $OUT）则跳过备份。
BAK="$OUT/source"
if [ -d "$BAK" ]; then
    echo "备份目录 $BAK 已存在，跳过输入文件备份！"
else
    mkdir -p "$BAK"
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
fi

LOG="$OUT/optimize-$TS.log"
date
echo "Running './prepare-inputs.sh' and 'code_genie $SUBCMD', writing log to $LOG ..."
{
    date

    echo '检查优化相关环境变量：---->'
    env | grep -E 'USE_|OPTIMIZE_|ENABLE_|DISABLE_|NO_'
    echo '<--------------------------'

    [ -e ../.git ] && {
        echo -n "GIT version: "
        git describe || true
        git status
        echo
    }

    set -x
    if [ "${NO_PREPARE:-}" = 1 ]; then
        echo "NO_PREPARE=1，跳过 ./prepare-inputs.sh"
    else
        echo "NO_PREPARE 不为 1，执行 ./prepare-inputs.sh"
        ./prepare-inputs.sh
    fi

    time $DRYRUN $CAFFEINATE $SAMPLY_RECORD $CODE_GENIE $SUBCMD "${ARGS[@]}"
    set +x

    date
} >"$LOG" 2>&1

tail -n 32 "$LOG"
echo
[ -f "$OUT/summary.txt" ] && cat "$OUT/summary.txt"
echo
date
