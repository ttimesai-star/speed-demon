#!/usr/bin/env bash
# Замер времени/памяти трека. Только Linux x86-64 (ПК WSL2 / GitHub Actions / любой Linux с Docker).
#   bench.sh quick   <track> <track_dir> <data_dir> [runs=5]   docker run с флагами доски; время вкл. старт контейнера (~0.3–0.6 с)
#   bench.sh native  <track> <binary>    <data_dir> [runs=5]   бинарь напрямую: argv1=data, argv2=out; время + maxRSS
#   bench.sh harness <track> <track_dir> <data_dir> [runs=5]   официальный harness/run_submission.py (root, cgroupfs, sd-timer) — точный M
# T считается как у доски: 5 прогонов, отбросить min и max, среднее трёх.
# ВАЖНО про память: maxRSS (native) включает страницы /data, отображённые через mmap, а memory.peak доски — нет
# (/data прогрет vmtouch хостом и на наш cgroup не списывается). Для M верить только harness или доске.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
SD="${SD_REPO:-$here/../specs_2026-10-29/sd2026_repo}"
mode=${1:?mode}; track=${2:?track}; target=${3:?dir_or_binary}; data=${4:?data_dir}; runs=${5:-5}
declare -A OUT=([a]=summary.txt [b]=results.txt [c]=aggregates.txt [d]=distances.txt)
declare -A TREF=([a]=32.047276 [b]=56.352376 [c]=14.444059 [d]=72.692655)   # harness/calibration-seed1.json
declare -A MREF=([a]=227983360 [b]=2199961600 [c]=367435776 [d]=892915712)  # bytes, там же
declare -A SREF=([a]=56 [b]=91 [c]=108 [d]=43)                                # harness/reference_axes.json
out_dir=$(mktemp -d); chmod 777 "$out_dir"
times=(); rss=0

mid3() { printf '%s\n' "$@" | sort -g | awk -v n=$# 'NR>1 && NR<n {s+=$1; c++} END {printf "%.4f", s/c}'; }

case $mode in
  quick)
    tag="sdbench/track-$track"
    docker build --network=none -q -t "$tag" "$target" >/dev/null
    for i in $(seq "$runs"); do
      rm -f "$out_dir"/*
      s=$(date +%s.%N)
      docker run --rm --network=none --read-only --tmpfs /tmp:rw,size=1g,exec \
        --memory=8g --memory-swap=8g --pids-limit=512 --cap-drop=ALL --security-opt=no-new-privileges \
        --user 65534:65534 --ulimit nofile=8192:8192 --cpuset-cpus="${CPUSET:-0-$(( $(nproc)-1 ))}" \
        -e TRACK="$track" -v "$(realpath "$data")":/data:ro -v "$out_dir":/out:rw "$tag"
      e=$(date +%s.%N); times+=("$(awk -v a="$s" -v b="$e" 'BEGIN{print b-a}')")
    done ;;
  native)
    for i in $(seq "$runs"); do
      rm -f "$out_dir"/*
      /usr/bin/time -f '%e %M' -o "$out_dir/.time" "$target" "$data" "$out_dir"
      read -r t m < "$out_dir/.time"; rm -f "$out_dir/.time"; times+=("$t"); (( m > rss )) && rss=$m
    done ;;
  harness)
    exp=$(awk -F'\t' -v t="$track" '$1==t && $2=="sample" {print $5}' "$here/expected_hashes.tsv")
    exec sudo python3 "$SD/harness/run_submission.py" --context "$target" --track "$track" --data "$data" \
      --output "${OUT[$track]}" --expected-sha256 "${EXPECTED:-$exp}" --timeout-s "$(awk -v t="${TREF[$track]}" 'BEGIN{print t*10}')" \
      --timer "${SD_TIMER:-$SD/harness/timer/sd-timer}" --cpuset "${CPUSET:-0-$(( $(nproc)-1 ))}" --runs "$runs" \
      --work "${WORK:-$HOME/sd-work}" --results "${RESULTS:-result_$track.json}" ;;
  *) echo "unknown mode $mode"; exit 2 ;;
esac

T=$(mid3 "${times[@]}")
echo "runs: ${times[*]}"
echo "T (mean of middle 3): $T s   ratio vs ref ${TREF[$track]} s: $(awk -v r="${TREF[$track]}" -v t="$T" 'BEGIN{printf "%.1fx", r/t}')"
[ "$mode" = native ] && echo "maxRSS: $((rss/1024)) MB (завышено, если mmap /data; M доски = memory.peak без /data)"
f="$out_dir/${OUT[$track]}"
if [ -f "$f" ]; then
  scale=${SCALE:-full}; "$here/check_hash.sh" "$track" "$scale" "$f" || true
fi
echo "K-калькулятор: python3 $here/kcalc.py $track <T_s> <M_MB> <S_lines> <deps>"
