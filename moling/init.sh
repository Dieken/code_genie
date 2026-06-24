#!/usr/bin/env bash

set -euo pipefail
shopt -s failglob

#########################################################################################################################

## 方案名
: "${SCHEMA:=${SCHEME:-moling}}"        # 兼容 SCHEME 变量，优先使用 SCHEMA

## 方案自定义配置
[ -f "init-$SCHEMA.sh" ] && . "init-$SCHEMA.sh"

## 方案默认配置
case "$SCHEMA" in
    # https://shurufa.app/ime/moling.html
    moling)     # 魔灵，@qq3qq, 四码自定码，25 键，大码聚类，小码(声码)为字根声母，补码(韵码)为字根首笔笔画，A1A2A3AzSzYz，仿灵明和星陈，二根字回头 A1A2S2S1Y1
        : "${ENCODE_RULE:=moling}"      # 单字编码规则
        : "${USE_VOWEL:=0}"             # 韵码取字根首笔笔画

        : "${USE_PINYIN_DU_FOR_TU:=1}"      # “土” 使用 du
        #: "${USE_PINYIN_VOU_FOR_KOU:=1}"    # “口” 使用 vou

        : "${OPTIMIZE_KEY_0=w}"         # 首根笔画时，多次退火优化都选择了 w
        : "${OPTIMIZE_KEY_y=k}"         # y 热力太高，首根笔画时，多次退火优化都选择映射到 k
        : "${OPTIMIZE_KEY_z=v}"         # 25 键方案，映射到 v，https://shurufa.app/docs/ling.html#%E4%B8%BA%E4%BB%80%E4%B9%88%E4%B8%8D%E7%94%A8-z-%E9%94%AE
        export OPTIMIZE_KEY_0 OPTIMIZE_KEY_y OPTIMIZE_KEY_z

        ;;

    moqing)     # 魔卿，@Litles，三码自定码，25 键，大码聚类，小码为字根声母，字根字 ASS，多根字 A1A2AzSz，仿潇湘
        : "${ENCODE_RULE:=moqing}"      # 单字编码规则
        : "${USE_VOWEL:=1}"             # 实际上魔卿是双编字根，并不用韵码，这里是为了跳过笔画处理

        : "${USE_MIXED_FREQ:=0}"        # 三码方案空间小，默认只优化简体 8105 通规字

        : "${OPTIMIZE_KEYS=0ei}"        # 优化零声母和 B 区用作声母的映射，e 在卿云是零声母，i 在卿云代表 yi
        : "${OPTIMIZE_KEY_z=v}"         # 25 键方案，映射到 v，https://shurufa.app/docs/ling.html#%E4%B8%BA%E4%BB%80%E4%B9%88%E4%B8%8D%E7%94%A8-z-%E9%94%AE
        export OPTIMIZE_KEY_z OPTIMIZE_KEYS

        : "${TOP_ROOT_KEYS:=sdfghjkl}"                  # 高频字根大码约束
        : "${HOT_ROOT_KEYS:=wru sdfghjkl vnm}"          # 中频字根大码约束
        : "${ALL_ROOT_KEYS:=qwrtyup sdfghjkl xcvbnm}"   # 低频字根大码约束
        : "${B_AREA_KEYS:=eaio}"                        # B 区小码 or 补码约束，实际在此脚本并没有用到，只用于后期简码分配

        MAX_CODE_LEN=3                                  # 最大码长为 3

        ;;

    # https://github.com/Dieken/code_genie/commit/741a1571b37505806e4058c2c6952935f6aa57a5
    xiaoming)   # 潇明，@恷子，五码自定码，25 键，大码聚类，小码映射字根声母，韵码为字根首笔笔画，单根字 ASY，多根字 A1S1A2A3AzSz，仿潇湘
        : "${ENCODE_RULE:=xiaoming}"    # 单字编码规则
        : "${USE_VOWEL:=0}"             # 韵码取字根首笔笔画

        : "${TOP_ROOT_KEYS:=asghl}"                     # 高频字根大码约束
        : "${HOT_ROOT_KEYS:=wruo asghl vnm}"            # 中频字根大码约束
        : "${ALL_ROOT_KEYS:=qwrtyuop asghl xcvbnm}"     # 低频字根大码约束
        : "${B_AREA_KEYS:=dfjkei}"                      # B 区小码 or 补码约束

        # 默认开启潇明的空格简码，并且优先空格简以跟码灵保持一致
        : ${ENABLE_SPACE_SHORTCODE:=1}                  # 启用空格简码
        : ${PREFER_SPACE_SHORTCODE:=1}                  # 优化使用空格简码

        MAX_CODE_LEN=5                                  # 最大码长为 5
        USE_STROKE_5_FOR_6=0                            # 区分笔画 5 和 6

        ;;

    # https://shurufa.app/ime/yaoling.html
    yaoling)    # 妖灵，@Evildoer, 四码自定码，25 键，大码聚类，小码映射字根声母，韵码为字根韵腹，单根字 ASY, 多根字 A1S1A2A3AzSzYz，仿灵明，分大小根、第三根跳根
        : "${ENCODE_RULE:=yuling}"      # 单字编码规则
        : "${USE_VOWEL:=1}"             # 韵码取韵母

        ROOTS_TXT="roots-$SCHEMA.txt"

        ;;

    # https://shurufa.app/ime/yueling.html
    yueling)    # 月灵，@枕月，四码自定码，25 键，大码聚类，小码为字根声母，韵码映射字根韵母，单根字 ASY，多根字 A1S1A2A3AzSzYz，仿灵明，分大小根，第三根跳根
        : "${ENCODE_RULE:=yuling}"      # 单字编码规则
        : "${USE_VOWEL:=1}"             # 韵码取韵母

        ROOTS_TXT="roots-$SCHEMA.txt"

        ;;

    # https://shurufa.app/docs/ling.html
    yuling)     # 灵明，@朱宇浩，四码自定码，25 键，大码聚类，小码为字根声母，韵码为字根韵腹，单根字 ASY，多根字 A1S1A2A3AzSzYz，分大小根，第三根跳根
        : "${ENCODE_RULE:=yuling}"      # 单字编码规则
        : "${USE_VOWEL:=1}"             # 韵码取韵母

        ROOTS_TXT="roots-$SCHEMA.txt"

        : "${USE_MIXED_FREQ:=1.0}"      # 与灵明官方版本一致，默认简繁字频等权重

        : "${OPTIMIZE_KEY_0=j}"         # 映射到 j，https://shurufa.app/docs/ling#%E4%B8%BA%E4%BB%80%E4%B9%88%E9%9B%B6%E5%A3%B0%E6%AF%8D%E7%9A%84%E5%A3%B0%E7%A0%81%E6%98%AF-j
        : "${OPTIMIZE_KEY_q=k}"         # 映射到 k，https://shurufa.app/docs/ling#%E4%B8%BA%E4%BB%80%E4%B9%88%E5%A3%B0%E7%A0%81%E4%B8%8D%E7%94%A8-q-%E9%94%AE
        : "${OPTIMIZE_KEY_z=v}"         # 25 键方案，映射到 v，https://shurufa.app/docs/ling.html#%E4%B8%BA%E4%BB%80%E4%B9%88%E4%B8%8D%E7%94%A8-z-%E9%94%AE
        export OPTIMIZE_KEY_0 OPTIMIZE_KEY_q OPTIMIZE_KEY_z

        ;;

    *)
        echo "ERROR: unknown schema '$SCHEMA', supported schemas: moling xiaoming yaoling yueling yuling" >&2
        exit 1
esac


## 默认配置
: "${ENCODE_RULE:=moling}"                      # 默认使用魔灵单字编码规则
: "${USE_VOWEL:=0}"                             # 默认韵码取字根首笔
: "${USE_MIXED_FREQ:=0.1}"                      # 默认混合 10% 加权的繁体字频

: "${TOP_ROOT_FREQ:=2.5}"                       # 高频字根频率阈值
: "${TOP_ROOT_KEYS:=sdfghjkl}"                  # 高频字根大码约束
: "${HOT_ROOT_FREQ:=1.5}"                       # 中频字根频率阈值
: "${HOT_ROOT_KEYS:=wr sdfghjkl vnm}"           # 中频字根大码约束
: "${ALL_ROOT_KEYS:=qwrtyp sdfghjkl xcvbnm}"    # 低频字根大码约束
: "${B_AREA_KEYS:=aeuio}"                       # B 区小码 or 补码约束
: "${MAX_CODE_LEN:=4}"                          # 最大码长

: "${USE_PINYIN_DU_FOR_TU:=0}"                  # 默认 “土” 使用 tu
: "${USE_PINYIN_VOU_FOR_KOU:=0}"                # 默认 “口” 使用 kou
: "${USE_STROKE_5_FOR_6:=1}"                    # 默认将笔画 6 合并到笔画 5

: "${ENABLE_SPACE_SHORTCODE:=0}"                # 是否启用空格简码，默认关闭
: "${PREFER_SPACE_SHORTCODE:=0}"                # 是否优先使用空格简码，默认优先使用韵码简码
: "${SIMPLE_PROTECT_TOP_N:=8000}"               # 简码不会抢前 N 个高频字的码位

# 优先空格简意味着开启空格简
[ "${PREFER_SPACE_SHORTCODE:-}" = 1 ] && export ENABLE_SPACE_SHORTCODE=1

# Perl 脚本里用到这些环境变量
export SCHEMA ENCODE_RULE USE_VOWEL
export TOP_ROOT_FREQ TOP_ROOT_KEYS HOT_ROOT_FREQ HOT_ROOT_KEYS ALL_ROOT_KEYS B_AREA_KEYS MAX_CODE_LEN
export USE_PINYIN_DU_FOR_TU USE_PINYIN_VOU_FOR_KOU USE_STROKE_5_FOR_6
export ENABLE_SPACE_SHORTCODE PREFER_SPACE_SHORTCODE

## 方案可以自行提供全字集字频文件、算码字集文件、字根小码补码表、字根聚类表
for s in full-freq chars roots roots-cluster; do
    v=$(echo $s | tr a-z A-Z)   # 大写，低版本 Bash 不支持 ${s^^}
    v=${v//-/_}                 # - 换成 _
    [ -f "$s-$SCHEMA.txt" ] && eval "${v}_TXT=$s-$SCHEMA.txt" || eval ": \${${v}_TXT:=$s.txt}"
    export "${v}_TXT"
done

