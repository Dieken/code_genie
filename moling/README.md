# 「魔灵两可」输入方案算码说明

## 方案设计

「魔灵两可」结合了 90% 的[灵明](https://shurufa.app/docs/ling.html)和 10% 的[星陈](https://shurufa.app/docs/star.html) 的设计：

1. 参照灵明，使用 25 键方案，声母 z/zh 用 v 代替，声母 q 用 k 代替，零声母用 j 代替，另外声母 y 用 d 代替以降低 Y 键压力, 声母 r 用 g 代替以降低回头码带来的声码相连差手感；
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

1. 在上层目录运行 `cargo build --release` 构建码灵；
2. 在本目录运行 `./optimize.sh` 或 `./optimize.sh --amhb --keysoul`(需最新版 Code Genie)；

`optimize.sh` 调用了 `prepare-inputs.sh`，后者接受几个环境变量来定制行为：

* `USE_VOWEL`: 设置为 1 表示字根的补码使用字根的韵母，默认是使用字根的首笔笔画；
* `OPTIMIZE_KEYS`: 设置为按键序列的字符串：
    * 包含 0 时，使用退火算法决定零声母的按键，默认使用 j；
    * 包含 q 时，使用退火算法决定声母 q 的按键，默认使用 k；
    * 包含 r 时，使用退火算法决定声母 r 的按键，默认不映射；
    * 包含 y 时，使用退火算法决定声母 y 的按键，默认使用 d；
    * 包含 z 时，使用退火算法决定声母 z 的按键，默认使用 v；
    * 包含 1 时，使用退火算法决定笔画「横」的按键，默认使用 i；
    * 包含 2 时，使用退火算法决定笔画「竖」的按键，默认使用 u；
    * 包含 3 时，使用退火算法决定笔画「撇」的按键，默认使用 o；
    * 包含 4 时，使用退火算法决定笔画「点」的按键，默认使用 e；
    * 包含 5 时，使用退火算法决定笔画「折」的按键，默认使用 a；

例如：

```sh
# 优化全部十个键映射，使用字根首笔作为韵码
OPTIMIZE_KEYS=012345qryz ./optimize.sh

# 优化全部五个键映射，使用字根韵母作为韵码
USE_VOWEL=1 OPTIMIZE_KEYS=0qryz ./optimize.sh
```

注意，开启按键映射后，`roots.tsv` 中的字根声码不是最终版，关闭 `USE_VOWEL` 使用字根首笔时，
`roots.tsv` 中的字根韵码不是最终版，最终的字根编码以码灵输出的 `output-TIMESTAMP/output-keymap.txt` 为准。

可以使用 `./batch-test-weights.sh` 来探测合理的权重参数范围：

```sh
./batch-test-weights.sh
./analyze-results-of-batch-test-weights.sh
```

可以使用 `./analyze-duplicates-by-cluster.pl` 来检查字根聚类的影响：

```sh
# 使用 roots-cluster.txt 中指定的聚类
diff --color -U0 <(./analyze-duplicates-by-cluster.pl -m 0 --cluster "") <(./analyze-duplicates-by-cluster.pl -m 0)

# 命令行指定聚类
diff --color -U0 <(./analyze-duplicates-by-cluster.pl -m 0 --cluster "") <(./analyze-duplicates-by-cluster.pl -m 0 --cluster "虍 虎 ; 皿 罒")

# 评估 roots-cluster.txt 中每一行聚类单独可能带来的重码
./analyze-duplicates-by-cluster.sh | tabulate -s '\t' -f plain
```

## 检查结果

1. 使用 https://ceping.shurufa.app 查看 `output-<TIMESTAMP>/output-combined.txt` 码表的指标，注意在「首页」里设置「編碼終止指示符] 为 "aeuio_" (不要引号)；
2. 运行 `./stat-moling-roots.pl --mabiao output-<TIMESTAMP>/output-combined.txt`；
3. 运行 `./generate-root-chart.sh output-<TIMESTAMP>` 生成字根表和字根图，也可以指定到 `output-<TIMESTAMP>/thread-<NN>` 目录；

## 文件说明

* 脚本程序
    * `optimize.sh`               算码流程包装脚本，调用 `./prepare-inputs.sh` 和 `code_genie optimize`
    * `prepare-inputs.sh`         准备码灵输入文件所用的脚本
    * `stat-moling-roots.pl`      统计优化出的魔灵码表和字根表
    * `generate-root-chart.sh`    生成字根表和字根图
    * `batch-test-weights.sh`     批处理优化以探测合理的权重参数范围
    * `analyze-duplicates-by-cluster.pl`
                                  分析字根聚类带来的重码
    * `analyze-duplicates-by-cluster.sh`
                                  评估 roots-cluster.txt 中每一行聚类单独可能带来的重码
    * `analyze-results-of-batch-test-weights.sh`
                                  分析 `batch-test-weights.sh` 的运行结果

* 第三方文件
    * `简体字频表-2.5b.txt`       北语字频, https://faculty.blcu.edu.cn/xinghb/zh_CN/article/167473/content/1437.htm
    * `宇浩字根列表.csv`          宇浩输入法系列的字根元信息，来自其作者朱宇浩
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

* 脚本生成的文件
    * `chaifen.txt`               生成的拆分表
    * `chaifen-all.txt`           生成的全字集拆分表，不参与优化，只用于生成大字集码表
    * `chars.txt`                 生成的常用字表
    * `freq.txt`                  生成的常用字字频文件
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

