# 潇明算码

命令都在 `moling/` 目录下执行。`SCHEMA=xiaoming` 让 `optimize.sh` 读取 `config-xiaoming.toml`，让 `generate-root-chart.sh` 按潇明取码规则出码。

## 1. 优化

```bash
echo "some comment" | SCHEMA=xiaoming ./optimize.sh
```

脚本会：

1. 从标准输入读一句话备注，追加到 `xiaoming-output/output-<时间戳>/COMMENT.txt`。
2. 把本次用到的配置和输入复制到 `xiaoming-output/output-<时间戳>/source/`。`source/` 已存在则跳过（`resume` 复用同一目录时如此）。
3. 默认先跑 `./prepare-inputs.sh`，再 `cargo build --release`，然后执行 `code_genie -c config-xiaoming.toml optimize -d xiaoming-output/output-<时间戳>`。
4. 日志写到 `xiaoming-output/output-<时间戳>/optimize-<时间戳>.log`。结束时在终端打出日志路径、日志末尾，以及 `summary.txt`。

未指定 `-d` 时，输出目录是 `$SCHEMA-output/output-<时间戳>`。潇明即 `xiaoming-output/output-<时间戳>/`。

常用环境变量：`NO_PREPARE=1` 跳过准备输入，`NO_BUILD=1` 跳过编译，`DRYRUN=1` 只打印命令。续跑：`SCHEMA=xiaoming ./optimize.sh resume -d xiaoming-output/output-<时间戳>`。

### 从已有字根布局起步

不指定种子时，每个线程从随机键位开始。增量优化可以改从一份已经测评过的 `output-keymap.txt` 起步，搜索贴着这个布局走。

```bash
echo "一句话备注" | SCHEMA=xiaoming ./optimize.sh \
  --seed-keymap xiaoming-output/output-<时间戳>/thread-<序号>/output-keymap.txt
```

`--seed-dir xiaoming-output/output-<时间戳>` 则按该目录下 `thread-NN/output-keymap.txt` 的序号升序收集，线程 `i` 使用第 `i % K` 份。`--seed-keymap` 与 `--seed-dir` 互斥。文件或目录不存在时，优化在校准之前就退出。

### 指标硬上限

`config-xiaoming.toml` 的 `[targets.full_code]` 和 `[targets.simple_code]` 里，名字以 `_max` 结尾的项是硬约束。值为 `0` 时不启用。启用后，退火每接受一步之前先检查。全码的重码数、重码率、当量、当量变异系数、分布偏差若超过对应上限，这一步直接拒绝并回滚，不进入得分比较。简码同理。其中 `freq_max` 是覆盖率下限，覆盖率低于该值时拒绝。

增量优化用它把搜索圈在起点布局附近。上限按起点的退火内部指标来设，留出很小的余地，使起点本身不被拒绝。设得比起点更严，搜索第一步就被退回，布局不会动。这些上限约束的是退火内部指标。宇浩测评用的是另一套语料，五项是否达标仍以宇浩数字为准。这次把全码当量硬上限设在起点内部当量之上一截（`equivalence_max`），用来防止为了改键位把速度当量推离起点太远。

## 2. 输出目录

`xiaoming-output/output-<时间戳>/` 的结构：

| 路径 | 用途 |
|---|---|
| `COMMENT.txt` | 启动时写入的备注 |
| `source/` | 这次优化的输入快照，事后对照参数以这里为准 |
| `optimize-<时间戳>.log` | 退火全过程日志 |
| `summary.txt` | 各线程按综合得分排序。带 🏆 的是第一名 |
| `thread-<序号>/` | 该线程的一份完整结果 |
| 顶层的 `output-*.txt` | 第一名线程的同名文件再复制一份 |

`source/` 里和这次相关的主要文件：

| 文件 | 格式 |
|---|---|
| `config-xiaoming.toml` | 退火、权重、目标、简码规则 |
| `input-division.txt` | `汉字 <tab> 字根1 字根2 ... <tab> 频率`，约一万多常用字，退火只用这份 |
| `input-fixed.txt` | `字根名 <tab> 键位`。单键为固定字根，多键为允许键位 |
| `input-roots.txt` | 一行一组动态字根，空格分隔 |
| `key_distribution-xiaoming.txt` | `键位 <tab> 目标% <tab> 用少了的惩罚 <tab> 用多了的惩罚`。某一侧惩罚为 0 表示该侧不罚。潇明用这份。`moling/` 里多方案并存，`key_distribution.txt` 是魔灵，以及没有 `key_distribution-$SCHEMA.txt` 的方案用的默认文件 |
| `pair_equivalence.txt` | `键对 <tab> 当量`。现为宇浩测评使用的陈氏当量，与速度当量同一标准和刻度 |
| `chaifen-all.txt` | 约十万字的全字集拆分，不参与退火，只给后面生成完整码表用 |

`thread-<序号>/` 与顶层第一名副本里的结果文件：

| 文件 | 格式 |
|---|---|
| `output-keymap.txt` | 头部是该线程的重码、当量等摘要，数据行是 `字根名 <tab> 键位 <tab> 使用次数` |
| `output-encode.txt` | `汉字 <tab> 全码 <tab> 频率`。只含优化用的一万多常用字 |
| `output-simple-codes.txt` | 按简码级别列出 `汉字 <tab> 简码 <tab> 频率` |
| `output-combined.txt` | `汉字 <tab> 编码`。先简码，再全部常用字的全码 |
| `output-distribution.txt` | `键位 <tab> 实际% <tab> 目标% <tab> 偏差 <tab> 偏差²` |
| `output-equiv-dist.txt` | 退火内部当量的分布，以及最高、最低当量的高频字 |
| `summary.txt` | 只在顶层。列：线程、综合得分、重码数、重码率%、当量、CV、分布偏差、简码覆盖%、简码当量、简码分布、简码重码、简码重码率%。得分只在同一份配置下可比 |

`summary.txt` 里的「当量」是陈氏键对当量的字频加权平均，刻度已与宇浩速度当量对齐（第 14 轮 T4 的内部 0.1250 对应北语 1.2374）。语料和简码仍可能让两个平均数差一截，五项指标的判定仍以宇浩测评为准。

## 3. 生成十万字码表

优化结果只有常用字。完整码表：

```bash
SCHEMA=xiaoming NO_HTML=1 ./generate-root-chart.sh xiaoming-output/output-<时间戳>/thread-<序号>
```

顶层第一名副本也可以作为参数。脚本用该目录的 `output-keymap.txt`，加上 `source/`（或上一级 `source/`）里的拆分表，写出：

| 文件 | 格式 |
|---|---|
| `roots.tsv` | `字根 <tab> 编码` |
| `chaifen.tsv` | `汉字 <tab> 拆分` |
| `mabiao.tsv` | `汉字 <tab> 编码`。含固定简码、各级简码和全字集全码，约十万字 |
| `zigen-xiaoming.csv`、`zigen-trainer-xiaoming.json` | 字根表与练习器数据 |
| `xiaoming.js`、`xiaoming-<日期>.html` | 字根图页面 |

潇明固定单键简码是 `不 k`、`在 f`、`是 j`、`我 i`、`的 d`、`了 e`。一级简码带空格，码表里写成 `字_`。

## 4. 测评

测评程序默认在 `../../yuhao-assess`，可用环境变量 `YUHAO_ASSESS` 指定。该目录不存在时，从 https://github.com/forfudan/yuhao-assess 获取。潇明方案配置默认在 `../../yuhao-assess-data/schemes/xiaoming.json`，可用 `YUHAO_ASSESS_DATA` 指定。

每个线程单独测评：

```bash
pnpm --dir "${YUHAO_ASSESS:-../../yuhao-assess}" exec tsx src/cli/index.ts \
  xiaoming-output/output-<时间戳>/thread-<序号>/mabiao.tsv \
  --scheme "${YUHAO_ASSESS_DATA:-../../yuhao-assess-data}/schemes/xiaoming.json" \
  --format table
```

`./evaluate-xiaoming-mabiao-to-tsv.sh` 会把全部线程的测评打成一张非常宽的 TSV，给 DuckDB 查询用。优化循环里不要跑它。

`--output result.txt` 可把扁平键值对写入文件。和全码目标对应的键：

| 目标 | 结果键 |
|---|---|
| 静态重码字数，通用规范 | `測評結果.靜態重碼分析.通用規範.全碼重碼字數` |
| 频率降序动态重码率，知乎简体 | `測評結果.動態選重分析.知乎簡體動態選重率.全碼` |
| 频率降序动态重码率，北语简体 | `測評結果.動態選重分析.北語簡體動態選重率.全碼` |
| 速度当量，全码，知乎简体 | `測評結果.速度當量分析.知乎簡體字頻全碼速度當量` |
| 速度当量，全码，北语简体 | `測評結果.速度當量分析.北語簡體字頻全碼速度當量` |

`動態選重率` 是按字频从高到低重排后的动态重码率。`動態選重率原序` 保持码表原序，不是这里的目标。动态重码率的小数要乘以 10000 才是「万分之」。

## 5. 全码目标

| 指标 | 目标 |
|---|---|
| 通用规范，全码静态重码字数 | 不多于 120 |
| 知乎简体，频率降序全码动态重码率 | 不多于万分之 0.5 |
| 北语简体，频率降序全码动态重码率 | 不多于万分之 1 |
| 知乎简体，全码速度当量 | 不高于 1.235 |
| 北语简体，全码速度当量 | 不高于 1.240 |

五项里，实际值等于目标也算达标。优先级不变。

## 6. 最终结果

人工筛选后的最佳布局是第 29 轮 T6：`xiaoming-output/output-20261011-061440/thread-06`。选择原因是 B 键热力约 0.8%，而且下排字母键 `Z X C V B N M` 的总热力最小。这不是按最后一轮、也不是按 `summary.txt` 的五项判定自动选出的。同目录 `summary.txt` 的得分第一名也是 T6，顶层 `output-keymap.txt` 是它的副本。

以后再次增量优化，从这份键位起步：

```bash
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

## 7. 自动优化循环

第 5 节的五项全码指标已经达到。下面是当时的流程，以后做增量调整时沿用。非常接近是指五项超额都很小，速度当量落在 1.24 附近、静重贴着 120，而不是只看综合得分变低。实际值等于目标也算达标。第 14 轮仍按静重目标 110 跑完。从第 15 轮起，静重目标为不多于 120，优先级不变。

只改两个文件：`config-xiaoming.toml`、`key_distribution-xiaoming.txt`。其它输入不动。不要改 `key_distribution.txt`，那是魔灵和未单独配置键位文件的方案用的。

优先级：速度当量，动态重码率，静态重码数，按键分布。先守住宇浩测评的静重、动重和速度当量，再优化按键分布。判定用宇浩测评对每个 `thread-*` 的 `mabiao.tsv` 的结果，不看 `summary.txt` 的第一名。键对表已是陈氏当量，退火里的当量与速度当量同一刻度，但语料不同，仍以宇浩数字为准。`summary.txt` 仍要全表看，用来判断 8 条线程是否散开、步数是否不够。

每次调整按帕累托前沿推进：往更高优先级的目标靠近，同时尽量不把已经达标或更好的低优先级指标退回去。一次只改一类东西（键位，或权重和目标，或步数），这样才分得清是哪一次改动把前沿往前推了。

按键热力希望集中在两个倒 T 上：左手 `E` 在上、`S D F` 为横；右手 `I` 在上、`J K L` 为横。`Q P T Y U` 尽量低。第三排 `X C V B N M` 次低，`C` 更低，`B` 最低。`Z` 不参与大码。键位是第四优先。速度当量还远的时候，不要为了倒 T 去压当量。

起步 `threads = 8`、`total_steps = 8000000`。步数少时，退火不充分会把整轮结果带偏，所以 800 万步主要用来探测参数方向，不拿它当最终结论。若分不清是参数不合适，还是步数太少，就适当加步数，不超过 `40000000`。加步会明显变慢：800 万步大约 8 分钟，2000 万步大约 20 分钟。方向还没确认时先用短程探测，确认盆地在动、或各线程散开、或已经接近目标，再加步。不要在一个已经证明不动的盆地里把步数加到上限。

再次增量优化时，从第 6 节的 T6 起步，不要从最后一轮或 `summary.txt` 第一名另起。命令用 `--seed-keymap xiaoming-output/output-20261011-061440/thread-06/output-keymap.txt`。并用 `equivalence_max` 等硬上限把内部当量圈在这份起点附近，避免为了改键位把速度当量推离起点太远。硬上限按起点的内部指标留出很小余地，起点本身要能通过。

每跑完 4 轮，回顾这 4 轮的参数改动和全部线程的测评：五项目标推进了多少，调整方向对不对。方向错了就收回，方向对了再沿着帕累托前沿走下一步。复盘直接显示出来。

每一轮的假设、测评表、字根码表摘要和下一步直接显示出来。退火进行中若超过几分钟，补一句当前轮次、目录和进度。

一轮的命令顺序：

```bash
echo "round-N: 一句话假设" | SCHEMA=xiaoming ./optimize.sh \
  --seed-keymap xiaoming-output/output-20261011-061440/thread-06/output-keymap.txt
SCHEMA=xiaoming NO_HTML=1 ./generate-root-chart.sh xiaoming-output/output-<时间戳>/thread-<序号>
pnpm --dir "${YUHAO_ASSESS:-../../yuhao-assess}" exec tsx src/cli/index.ts \
  xiaoming-output/output-<时间戳>/thread-<序号>/mabiao.tsv \
  --scheme "${YUHAO_ASSESS_DATA:-../../yuhao-assess-data}/schemes/xiaoming.json" \
  --format table
```

每个线程都生成码表并测评。测评之后还要看两份文件。优化日志看末段：得分是否还在降，用来决定要不要加步。`output-keymap.txt` 看进入前沿的线程（速度最好、超额最小、得分第一；不是同一条就都看）。数据行是 `字根名 <tab> 编码 <tab> 使用次数`。记下使用次数最高的字根落在哪些编码上，并对照按键热力：小码区是 `D F J K E I`，大码区是除 `Z` 以外的其余字母。这两份都不代替宇浩测评的五项指标。然后对照第 5 节的五项目标，按优先级决定下一轮只动配置或键位文件里的哪几项。

第一轮配置写好后先等人确认再跑。第一轮结果也先给人看。再次确认之后才进入连续的自动循环。
