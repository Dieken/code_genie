# 「魔灵两可」输入方案算码说明

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

