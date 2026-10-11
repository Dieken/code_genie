# 潇明算码

通用流程见 `HOWTO-OPTIMIZATION.md`。本文只写潇明自己的文件、目标和当前起步布局。命令都在 `moling/` 目录下执行。`SCHEMA=xiaoming`。

## 1. 文件

| 用途 | 文件 |
|---|---|
| 配置 | `config-xiaoming.toml` |
| 键位分布 | `key_distribution-xiaoming.txt` |
| 输出目录 | `xiaoming-output/output-<时间戳>/` |
| 宇浩方案 | `../../yuhao-assess-data/schemes/xiaoming.json` |

`key_distribution.txt` 是魔灵和未单独配置键位文件的方案用的。潇明不要改它。

优化循环里只改 `config-xiaoming.toml` 和 `key_distribution-xiaoming.txt`。其它输入不动。

`pair_equivalence.txt` 现为宇浩测评使用的陈氏当量，与速度当量同一标准和刻度。第 14 轮 T4 的内部当量 0.1250 对应北语速度当量 1.2374。语料和简码仍可能让两个平均数差一截，五项指标仍以宇浩数字为准。

潇明固定单键简码是 `不 k`、`在 f`、`是 j`、`我 i`、`的 d`、`了 e`。一级简码带空格，码表里写成 `字_`。

`generate-root-chart.sh` 还会写出 `zigen-xiaoming.csv`、`zigen-trainer-xiaoming.json`、`xiaoming.js` 和 `xiaoming-<日期>.html`。

`evaluate-xiaoming-mabiao-to-tsv.sh` 会把全部线程的测评打成一张非常宽的 TSV，给 DuckDB 查询用。优化循环里不要跑它。

## 2. 全码目标

| 指标 | 目标 |
|---|---|
| 通用规范，全码静态重码字数 | 不多于 120 |
| 知乎简体，频率降序全码动态重码率 | 不多于万分之 0.5 |
| 北语简体，频率降序全码动态重码率 | 不多于万分之 1 |
| 知乎简体，全码速度当量 | 不高于 1.235 |
| 北语简体，全码速度当量 | 不高于 1.240 |

五项里，实际值等于目标也算达标。优先级和按键热力用 `HOWTO-OPTIMIZATION.md` 的默认，潇明没有另行覆盖。

第 14 轮仍按静重目标 110 跑完。从第 15 轮起，静重目标为不多于 120。非常接近是指五项超额都很小，速度当量落在 1.24 附近、静重贴着 120，而不是只看综合得分变低。

增量优化时，全码当量硬上限设在起点内部当量之上一截（`equivalence_max`），用来防止为了改键位把速度当量推离起点太远。

## 3. 最终结果

五项全码指标已经达到。人工筛选后的最佳布局是第 29 轮 T6：`xiaoming-output/output-20261011-061440/thread-06`。选择原因是 B 键热力约 0.8%，而且下排字母键 `Z X C V B N M` 的总热力最小。这不是按最后一轮、也不是按 `summary.txt` 的五项判定自动选出的。同目录 `summary.txt` 的得分第一名也是 T6，顶层 `output-keymap.txt` 是它的副本。

以后再次增量优化，从这份键位起步：

```bash
echo "一句话备注" | SCHEMA=xiaoming ./optimize.sh \
  --seed-keymap xiaoming-output/output-20261011-061440/thread-06/output-keymap.txt
```

| 指标 | 目标 | T6 |
|---|---|---|
| 通用规范，全码静态重码字数 | 不多于 120 | 121 |
| 知乎简体，频率降序全码动态重码率 | 不多于万分之 0.5 | 万分之 0.417 |
| 北语简体，频率降序全码动态重码率 | 不多于万分之 1 | 万分之 0.842 |
| 知乎简体，全码速度当量 | 不高于 1.235 | 1.2323 |
| 北语简体，全码速度当量 | 不高于 1.240 | 1.2329 |
| A 键实际用量 | | 3.70% |
| B 键实际用量 | | 0.83% |
| P 键实际用量 | | 2.15% |
| 下排 `Z X C V B N M` 合计 | | 11.96% |

静重 121，比目标多 1。两项动重和两项速度当量达标。退火分布里 B 是 0.8273%，下排合计 11.96%（`Z` 为 0）。使用次数最高的字根是「一 Hdk」「口 Aki」「丶 Lff」。
