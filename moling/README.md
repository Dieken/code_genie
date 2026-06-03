# 「魔灵两可」输入方案算码说明

## 方案设计

「魔灵两可」结合了 90% 的[灵明](https://shurufa.app/docs/ling.html)和 10% 的[星陈](https://shurufa.app/docs/star.html) 的设计：

1. 参照灵明，使用 25 键方案，声母 z/zh 用 v 代替，零声母用 w 代替，另外声母 y 用 k 代替以降低 Y 键压力；
2. 字根聚类参照灵明，因为它已经验证了这些聚在一起不太损耗性能；
3. 声码和韵码基本参照灵明，严格按拼音，非成字字根和非常用字字根省略声母，字根尽量只聚类不归并，除非形状过于相似容易看错，不归并带来的好处是可以保留字根原来的读音，不因字根设计而导致读音变了，得特殊记忆错误的读音，注意为了提高手感，默认配置下韵码使用首笔笔画代替，参见下面 `USE_VOWEL` 环境变量的说明；
4. 单字编码为 A1A2A3AzSzYz，除了双根字编码为 A1A2S2S1Y1 ，模仿了星陈的回头码设计，单字编码限长四码；

魔灵两可的初衷是降低灵明的学习难度（灵明要记忆省略声母的小根），以及舍弃了灵明二码字根字不能组二字词的设计决策（灵明是为了码长短），虽然魔灵两可的简体动态重码率还不错，但在静态重码数、繁体动态重码率、当量、码长上都距灵明甚远，只能算是结合了灵明特性的改进版星陈（但繁体性能依然比星陈差），因此并不推荐使用，对自分割码感兴趣的朋友应去学习宇浩输入法系列的[日月](https://shurufa.app/docs/ming.html)和[灵明](https://shurufa.app/docs/ling.html) 方案，这里公开算码相关文件是希望同道中人一起挖掘宇码方案的不同玩法，复用宇码的基础设施如拆分、字根图、字根练习、拆分查询等。

性能目标：

* 通规字静态重码数：低于 700；
* 简体动态重码率： 2‱ 左右；
* 繁体动态重码率： 10‱ 左右；
* 陈氏键均当量：1.28；


## 算码流程

> 推荐 Windows 用户使用 [MSYS2](https://packages.msys2.org/) 来运行以下工具，在 MSYS2 里使用 `pacman -S git rust perl` 安装 Git、Rust、Perl，参照 [TUNA crates.io 镜像](https://mirrors.tuna.tsinghua.edu.cn/help/crates.io-index/)配置 Cargo。

1. 在上层目录运行 `cargo build --release` 构建码灵；
2. 在本目录运行 `./optimize.sh` 或 `./optimize.sh --amhb --keysoul`(需最新版 Code Genie)；

`optimize.sh` 调用了 `prepare-inputs.sh`，后者接受几个环境变量来定制行为：

* `USE_MIXED_FREQ`：非零时表示组合台版繁体字频的权重，默认为 0.1，设置为空或 0 时表示只使用简体字频；
* `USE_VOWEL`: 设置为 1 表示字根的补码使用字根的韵母，默认是使用字根的首笔笔画；
* `USE_YULING_RULE`： 设置为 1 表示使用宇浩灵明的单字编码规则，并从宇浩灵明字根表初始化 `roots.txt`(如果文件不存在)，后续需手动维护此文件，默认是使用魔灵的单字编码规则；
* `USE_YAOLING_RULE`: 设置为 1 表示使用 @Evildoer 的妖灵规则（大根声码映射且韵码固定），包含了 `USE_YULING_RULE=1` 和 `USE_VOWEL=1`，默认关闭，使用魔灵规则；
* `USE_YUELING_RULE`: 设置为 1 表示使用 @枕月 的月灵规则(韵码仿日月映射)，包含了 `USE_YULING_RULE=1` 和 `USE_VOWEL=1`，默认关闭，使用魔灵规则；
* `OPTIMIZE_KEYS`: 设置为按键序列的字符串：
    * 包含 0 时，使用退火算法决定零声母的按键，默认使用 w；
    * 包含 q 时，使用退火算法决定声母 q 的按键，默认不映射；
    * 包含 r 时，使用退火算法决定声母 r 的按键，默认不映射；
    * 包含 y 时，使用退火算法决定声母 y 的按键，默认使用 k；
    * 包含 z 时，使用退火算法决定声母 z 的按键，默认使用 v；
    * 包含 1 时，使用退火算法决定笔画「横」的按键，默认使用 o；
    * 包含 2 时，使用退火算法决定笔画「竖」的按键，默认使用 u；
    * 包含 3 时，使用退火算法决定笔画「撇」的按键，默认使用 e；
    * 包含 4 时，使用退火算法决定笔画「点」的按键，默认使用 i；
    * 包含 5 时，使用退火算法决定笔画「折」的按键，默认使用 a；
    * 包含 6 时，使用退火算法决定声码在键盘右手侧时笔画「横」的按键，默认使用 e；
    * 包含 7 时，使用退火算法决定声码在键盘右手侧时笔画「竖」的按键，默认使用 e；
    * 包含 8 时，使用退火算法决定声码在键盘左手侧时笔画「撇」的按键，默认使用 i；
    * 包含 9 时，使用退火算法决定声码在键盘右手侧时笔画「点」的按键，默认使用 e；
    * 包含 A 时，使用退火算法决定声码在键盘左手侧时笔画「折」的按键，默认使用 u；

例如：

```sh
# 优化全部十个键映射，使用字根首笔作为韵码
OPTIMIZE_KEYS=012345qryz ./optimize.sh

# 优化全部五个键映射，使用字根韵母作为韵码
USE_VOWEL=1 OPTIMIZE_KEYS=0qryz ./optimize.sh

# 计算妖灵
rm roots.txt # 从灵明字根表初始化
USE_YAOLING_RULE=1 ./optimize.sh

# 计算月灵
## !!! 注意提前调整 roots.txt 的字根拼音
USE_YUELING_RULE=1 ./optimize.sh
```

注意：开启按键映射后，`roots.txt` 中的字根声码不是最终版，关闭 `USE_VOWEL` 使用字根首笔时，
`roots.txt` 中的字根韵码不是最终版，最终的字根编码以码灵输出的 `output-TIMESTAMP/output-keymap.txt` 为准。


## 检查结果

1. 使用 https://ceping.shurufa.app 查看 `output-<TIMESTAMP>/output-combined.txt` 码表的指标，注意在「首页」里设置「編碼終止指示符] 为 "aeuio_" (不要引号)；
2. 运行 `./compare-optimization-results.sh` 批量检查 `output-<TIMESTAMP>/thread-<NN>/output-combined.txt`，注意脚本末尾的过滤条件比较严，完整结果见生成的 `all-results.txt`；
3. 运行 `./stat-moling-roots.pl --mabiao output-<TIMESTAMP>/output-combined.txt`；
4. 运行 `./generate-root-chart.sh output-<TIMESTAMP>` 生成字根表和字根图，也可以指定到 `output-<TIMESTAMP>/thread-<NN>` 目录；


## 优化指北

1. 可以使用 `./batch-test-weights.sh` 来探测合理的权重参数范围：

```sh
./batch-test-weights.sh
./analyze-results-of-batch-test-weights.sh
```

2. 可以使用 `./analyze-duplicates-by-cluster.pl` 来检查字根聚类的影响：

```sh
# 使用 roots-cluster.txt 中指定的聚类
diff --color -U0 <(./analyze-duplicates-by-cluster.pl -m 0 --cluster "") <(./analyze-duplicates-by-cluster.pl -m 0)

# 命令行指定聚类
diff --color -U0 <(./analyze-duplicates-by-cluster.pl -m 0 --cluster "") <(./analyze-duplicates-by-cluster.pl -m 0 --cluster "虍 虎 ; 皿 罒")

# 评估 roots-cluster.txt 中每一行聚类单独可能带来的重码
./analyze-duplicates-by-cluster.sh | tabulate -s '\t' -f plain
```

3. `config.toml` 为魔灵定制，其它方案应注意调整：

    1. `total_steps` 可取 8000000 用于调整参数时的试验，当调大步数时，观察日志，如果在某一进度百分比后过早停滞，说明已经收敛，更多的步数只是浪费；
    2. 观察日志里的优化真正有效时起始温度，保留开头的 20~30% 步数用于探索，以及优化进展比较大的温度区间、优化停滞时结束温度，适度调整 temp_start, temp_end, comfort_temp，可以把 `config.toml` 和日志、代码丢给大语言模型分析，让其给出解释和建议；
    3. 先注释掉 `[scale]` 段，通过自动校正得出合适的值设置上，以保证调整参数时的稳定性；
    4. 先关掉 `[targets.full_code]` 段，观察多次优化的结果再设置上，目标应比优化的最好结果略微低一点，以提供足够的优化动力；
    5. 理解[基于目标偏差优化](https://github.com/Dieken/code_genie/commit/ad79690efe454140886ab69bf6341f2a07561307)的设计原理，`[weights.full_code]` 用作指标重要性的度量，总和应为 1，`[scale]` 作为优化动力强度的度量，优化时主要调整这两处，注意修改 `[scale]` 后，优化得分跟之前的轮次再无可比性，只能比较指标数值本身。`[targets.full_code]` 经过多次摸底后应少改，以方便朝既定目标调整参数对比。


## 字根练习

1. https://shurufa.app/ime/moling#%E7%BB%83%E4%B9%A0
2. https://zigen-trainer.hch12907.dev/
3. https://github.com/mogud/yu_tool
4. https://chs.hertz.ltd/#practice
5. https://unyaa-code.github.io/root-practice/
6. https://github.com/Dieken/typer


## 文件说明

* 脚本程序
    * `optimize.sh`               算码流程包装脚本，调用 `./prepare-inputs.sh` 和 `code_genie optimize`，支持环境变量 `USE_YULING_RULE`
    * `prepare-inputs.sh`         准备码灵输入文件所用的脚本，支持环境变量 `USE_YULING_RULE`
    * `stat-moling-roots.pl`      统计优化出的魔灵码表和字根表
    * `generate-root-chart.sh`    生成字根表和字根图
    * `batch-test-weights.sh`     批处理优化以探测合理的权重参数范围
    * `analyze-duplicates-by-cluster.pl`
                                  分析字根聚类带来的重码，支持环境变量 `USE_YULING_RULE`
    * `analyze-duplicates-by-cluster.sh`
                                  评估 roots-cluster.txt 中每一行聚类单独可能带来的重码，支持环境变量 `USE_YULING_RULE`
    * `analyze-results-of-batch-test-weights.sh`
                                  分析 `batch-test-weights.sh` 的运行结果
    * `convert-yuling-rime-schema-to-moling.sh`
                                  转换灵明 RIME 方案为魔灵 RIME 方案，支持环境变量 `USE_YULING_RULE`
    * `compare-optimization-results.sh`
                                  比较 `output-<TIMESTAMP>/thread-<NN>` 的优化结果，依赖[命令行版本的宇浩测评](https://github.com/Dieken/yuhao-assess/tree/cli)

* 第三方文件
    * `beiyu-char-freq.txt`       北语字频, https://faculty.blcu.edu.cn/xinghb/zh_CN/article/167473/content/1437.htm
    * `charAbsoluteFrequencySC.json`
                                  北语简体字频，来自 https://ceping.shurufa.app/data/charAbsoluteFrequencySC.json
    * `charAbsoluteFrequencyTC.json`
                                  台标繁体字频，来自 https://ceping.shurufa.app/data/charAbsoluteFrequencyTC.json
    * `yuhao-zigens.csv`          宇浩字根列表，宇浩输入法系列的字根元信息，来自其作者朱宇浩
    * `chars.dict.yaml`           万象拼音词典，https://github.com/amzxyz/RIME-LMDG/blob/62f844d0fd6ac0d6ab2cf9bace6ed34b5a3e318c/dicts/chars.dict.yaml
    * `yuhao_charsets.lua`        宇浩 RIME 方案 Lua 脚本, 来自`星陳輸入法_v3.11.0/schema/lua/yuhao/yuhao_charsets.lua`
    * `yustar_chaifen.dict.yaml`  星陳拆分表，来自`星陳輸入法_v3.11.0/schema/yustar_chaifen.dict.yaml`

* 手动维护数据文件
    * `config.toml`               码灵配置文件，改自 ../config.toml.example
    * `roots-cluster.txt`         手动维护的字根聚类，来自灵明字根图
    * `roots-fly.txt`             手动维护的飞键字根，一行一个字根
    * `pair_equivalence.txt`      键对当量表，改自 `../pair_equivalence.txt`
    * `key_distribution.txt`      键位分布目标，改自 `../key_distribution.txt`
    * `config.toml.tmpl`          批处理优化的配置文件模版
    * `moling.jsonc`              脚本 `compare-optimization-results.sh` 的方案配置文件

* 脚本生成的文件
    * `chaifen.txt`               生成的拆分表
    * `chaifen-all.txt`           生成的全字集拆分表，不参与优化，只用于生成大字集码表
    * `chars.txt`                 生成的常用字表
    * `freq.txt`                  生成的常用字字频文件
    * `full-freq.txt`             生成的简体字频或者简繁混合字频文件
    * `input-division.txt`        生成的码灵输入文件
    * `input-fixed.txt`           生成的码灵输入文件
    * `input-roots.txt`           生成的码灵输入文件
    * `roots-fly-candidates.txt`  生成的飞键字根候选表
    * `roots-freq.txt`            生成的字根频率表
    * `roots-pinyin.txt`          生成的字根拼音
    * `roots.txt`                 生成的字根声码和韵码
    * `done-*`                    批处理优化的标记文件
    * `test-*.log`                批处理优化的日志文件
    * `config-c*-r*-e*.toml`      批处理优化的配置文件
    * `batch-test-weights.txt`    批处理优化的结果分析, CSV 版本
    * `batch-test-weights.html`   批处理优化的结果分析, HTML 版本
    * `all-results.txt`           脚本 `compare-optimization-results.sh` 的运行结果

