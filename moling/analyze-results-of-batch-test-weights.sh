#!/usr/bin/env bash

set -euo pipefail
shopt -s failglob

which tabulate >/dev/null && TABULATE="tabulate -f plain" || TABULATE=cat

if [ $# -eq 0 ]; then
  set -- test-*.log
fi

echo "Writing batch-test-weights.txt ..."
perl -CSDA -Mautodie -Mutf8 -lnE 'use List::Util qw/min max/;
  if (/^全码权重/) {
    @a = $_ =~ /\d+\.\d+/g;
  } elsif (/^输出目录.*(output\S+)/) {
    $d = $1;
    $f = "$1/summary.txt";
    next unless -e $f;

    $n = 0;
    open my $fh, $f;
    while (<$fh>) {
      next unless /^T\d/;

      ++$n;
      @b = split;

      if ($n == 1) {
        @a2 = @b[2..6];
        @a3 = @a2;
        @a4 = @a2;
      } else {
        for (0..4) {
          $a3[$_] = min($a3[$_], $b[$_ + 2]);
          $a4[$_] = max($a4[$_], $b[$_ + 2]);
        }
     }
    }

    print "@a | $d | @a2 | @a3 | @a4";
  }' "$@" | $TABULATE > batch-test-weights.txt


echo "Writing batch-test-weights.html ..."
perl -CSDA -Mutf8 -lanE '
  use JSON::PP;
  use List::Util qw/min max/;

  BEGIN {
    @a = ();

    sub dimension($i, $label) {
      my $values = $a[$i];
      my @range = ( min(@$values), max(@$values) );
      return {
        range => \@range,
        label => $label,
        values => $values,
      };
    }
  }

  for ($i = 0; $i < @F; ++$i) { $a[$i][$. - 1] = $F[$i] }

  END {
    %trace = (
      type =>  "parcoords",
      line => {
        color => "blue",
      },
      dimensions => [
        dimension(0, "重码数权重"),
        dimension(1, "重码率权重"),
        dimension(2, "当量权重"),
        dimension(4, "分布偏差权重"),

        dimension(14, "重码数(min)"),
        dimension(8, "重码数"),
        dimension(20, "重码数(max)"),

        dimension(15, "重码率%(min)"),
        dimension(9, "重码率%"),
        dimension(21, "重码率%(max)"),

        dimension(16, "当量(min)"),
        dimension(10, "当量"),
        dimension(22, "当量(max)"),

        dimension(17, "当量变异(min)"),
        dimension(11, "当量变异"),
        dimension(23, "当量变异(max)"),

        dimension(18, "分布偏差(min)"),
        dimension(12, "分布偏差"),
        dimension(24, "分布偏差(max)"),
      ],
    );

    $trace = JSON::PP->new->pretty->encode([ \%trace ]);

    print <<END
<!DOCTYPE html>
<html lang="zh" dir="ltr">
<head>
  <meta charset="UTF-8">
  <meta http-equiv="Content-Type" content="text/html; charset=UTF-8">
  <meta http-equiv="X-UA-Compatible" content="IE=edge,chrome=1">
  <meta name="viewport" content="width=device-width, initial-scale=1.0"/>
  <title>魔灵算码权重空间探索</title>
  <script src="https://cdn.plot.ly/plotly-3.4.0.min.js" charset="utf-8"></script>
</head>
<body>
  <h1>魔灵算码权重空间探索</h1>
  <div id="chart"></div>
  <p>
  使用说明：
  <ol>
  <li>此平行坐标图的用意在于通过多维指标范围的过滤，得出最佳的权重参数范围，也就是前四列的最佳取值范围，以此指导优化时的权重参数调整；</li>
  <li>在数轴上拖动以设置过滤范围；</li>
  <li>点击过滤条以取消过滤；</li>
  <li>每个指标都画了三条数轴，min, highest score, max，观察每次探索的指标波动情况，以评估是否步数太少而失真；</li>
  <li>过滤指标总是用 min 轴，因为正式优化时，steps 会设置比较大，优化出的指标都会更小；</li>
  <li>字根或单字编码方案变化时，以及更新「码灵」程序时，最好重新探索权重空间；</li>
  </ol>
  </p>
  <script>
  Plotly.newPlot("chart", $trace);
  </script>
</body>
</html>
END
  }
' batch-test-weights.txt > batch-test-weights.html
