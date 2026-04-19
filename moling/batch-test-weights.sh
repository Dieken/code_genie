#!/usr/bin/env bash

set -euo pipefail
shopt -s failglob

: ${CODE_GENIE:=../target/release/code_genie}
: ${THREADS:=8}
: ${STEPS:=2000000}
: ${PARAMS:="5 10 15 20 25 30 35 40 45 50"}
: ${COUNT_PARAMS:=$PARAMS}
: ${RATE_PARAMS:=$PARAMS}
: ${EQUIV_PARAMS:=$PARAMS}
: ${EQUIV_CV:=1}

which caffeinate >/dev/null && CAFFEINATE="caffeinate -imsu" || CAFFEINATE=

for collision_count in $COUNT_PARAMS; do
  for collision_rate in $RATE_PARAMS; do
    for equivalance in $EQUIV_PARAMS; do
      equiv_cv=$EQUIV_CV

      if [ $(( collision_count + collision_rate + equivalance + $equiv_cv )) -gt 100 ]; then
        echo "Skip collision_count=$collision_count collision_rate=$collision_rate equivalance=$equivalance equiv_cv=$equiv_cv"
        continue
      fi

      date
      distribution=$(( 100 - collision_count - collision_rate - equivalance - $equiv_cv ))
      echo "Test collision_count=$collision_count collision_rate=$collision_rate equivalance=$equivalance equiv_cv=$equiv_cv distribution=$distribution ..."

      suffix="c$collision_count-r$collision_rate-e$equivalance-cv$equiv_cv-d$distribution"
      [ -e done-$suffix ] && continue

      [ ${#collision_count} = 1 ] && collision_count="0$collision_count"
      [ ${#collision_rate} = 1 ] && collision_rate="0$collision_rate"
      [ ${#equivalance} = 1 ] && equivalance="0$equivalance"
      [ ${#equiv_cv} = 1 ] && equiv_cv="0$equiv_cv"
      [ ${#distribution} = 1 ] && distribution="0$distribution"

      sed -e "s/@@THREADS@@/$THREADS/; s/@@TOTAL_STEPS@@/$STEPS/; s/@@COLLISION_COUNT@@/0.$collision_count/; s/@@COLLISION_RATE@@/0.$collision_rate/; s/@@EQUIVALENCE@@/0.$equivalance/; s/@@EQUIV_CV@@/0.$equiv_cv/; s/@@DISTRIBUTION@@/0.$distribution/" \
        config.toml.tmpl > config-$suffix.toml

      set -x
      $CAFFEINATE $CODE_GENIE --config config-$suffix.toml optimize && touch done-$suffix
      set +x

      echo; echo
    done
  done
done | tee test-$(date +%Y%m%d-%H%M%S).log
