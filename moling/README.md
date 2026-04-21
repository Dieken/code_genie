# 「魔灵两可」输入方案算码说明

## 方案设计

「魔灵两可」结合了 90% 的[灵明](https://shurufa.app/docs/ling.html)和 10% 的[星陈](https://shurufa.app/docs/star.html) 的设计：

1. 参照灵明，使用 25 键方案，声母 z/zh 用 v 代替，声母 q 用 k 代替，零声母用 j 代替，另外声母 y 用 d 代替以降低 Y 键压力；
2. 字根聚类参照灵明，因为它已经验证了这些聚在一起不太损耗性能；
3. 声码和韵码基本参照灵明，严格按拼音，但去掉了省略声码的设计，而且字根尽量只聚类不归并，除非形状过于相似容易看错，不归并带来的好处是可以保留字根原来的读音，不因字根设计而导致读音变了，得特殊记忆错误的读音；
4. 单字编码为 A1 A2A3AzSzYz，除了双根字编码为 A1A2S2S1Y1 ，模仿了星陈的回头码设计，单字编码限长四码；

魔灵两可的初衷是降低灵明的学习难度（灵明要记忆省略声母的小根），以及舍弃了灵明二码字根字不能组二字词的设计决策（灵明是为了码长短），虽然魔灵两可的简体动态重码率还不错，但在静态重码数、繁体动态重码率、当量、码长上都距灵明甚远，只能算是结合了灵明特性的改进版星陈（但繁体性能依然比星陈差），因此并不推荐使用，对自分割码感兴趣的朋友应去学习宇浩输入法系列的[日月](https://shurufa.app/docs/ming.html)和[灵明](https://shurufa.app/docs/ling.html) 方案，这里公开算码相关文件是希望同道中人一起挖掘宇码方案的不同玩法，复用宇码的基础设施如拆分、字根图、字根练习、拆分查询等。


## 算码流程

1. 运行 `./prepare-inputs.sh` 生成所需文件；
2. 在上层目录运行 `cargo build --release`；
3. 在本目录运行 `../target/release/code_genie optimize` 或 `../target/release/code_genie optimize --amhb --keysoul`(需最新版 Code Genie)，macOS 下可以命令前加 `caffeinate -imsu` 防止系统休眠；

可以使用 `./batch-test-weights.sh` 来探测合理的权重参数范围：

```sh
./batch-test-weights.sh
./analyze-results-of-batch-test-weights.sh | tabulate -f plain
```

## 检查结果

1. 使用 https://ceping.shurufa.app 查看 `output-<TIMESTAMP>/output-combined.txt` 码表的指标，注意在「首页」里设置「編碼終止指示符] 为 "aeuio_" (不要引号)；
2. 运行 `./stat-moling-roots.pl --mabiao output-<TIMESTAMP>/output-combined.txt`；
3. 运行 `./generate-root-chart.sh output-<TIMESTAMP>` 生成字根表和字根图，也可以指定到 `output-<TIMESTAMP>/thread-<NN>` 目录；

## 文件说明

* 脚本程序
    * `prepare-inputs.sh`         准备码灵输入文件所用的脚本
    * `stat-moling-roots.pl`      统计优化出的魔灵码表和字根表
    * `generate-root-chart.sh`    生成字根表和字根图
    * `batch-test-weights.sh`     批处理优化以探测合理的权重参数范围
    * `analyze-results-of-batch-test-weights.sh`
                                  分析 `batch-test-weights.sh` 的运行结果

* 第三方文件
    * 简体字频表-2.5b.txt         北语字频, https://faculty.blcu.edu.cn/xinghb/zh_CN/article/167473/content/1437.htm
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
    * `chars.txt`                 生成的常用字表
    * `freq.txt`                  生成的常用字字频文件
    * `input-division.txt`        生成的码灵输入文件
    * `input-fixed.txt`           生成的码灵输入文件
    * `input-roots.txt`           生成的码灵输入文件
    * `roots-freq.txt`            生成的字根频率表
    * `roots-pinyin.txt`          生成的字根拼音
    * `roots.txt`                 生成的字根声码和韵码
    * `done-*`                    批处理优化的标记文件
    * `test-*.log`                批处理优化的日志文件
    * `config-c*-r*-e*.toml`      批处理优化的配置文件
    * `batch-test-weights.txt`    批处理优化的结果分析, CSV 版本
    * `batch-test-weights.html`   批处理优化的结果分析, HTML 版本

