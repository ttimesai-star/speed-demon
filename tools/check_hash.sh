#!/usr/bin/env bash
# Сверка SHA-256 вывода трека с эталоном из спеки.
#   check_hash.sh <track a|b|c|d> <scale full|sample|tiny|worked> <path/to/output_file>
# Код возврата: 0 = совпало, 1 = не совпало, 2 = нет эталона/ошибка аргументов.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
[ $# -eq 3 ] || { echo "usage: $0 <track> <scale> <output_file>"; exit 2; }
track=$1; scale=$2; out=$3
[ -f "$out" ] || { echo "FAIL  нет файла $out"; exit 2; }
want=$(awk -F'\t' -v t="$track" -v s="$scale" '$1==t && $2==s {print $5}' "$here/expected_hashes.tsv")
[ -n "$want" ] || { echo "FAIL  нет эталона для $track/$scale в expected_hashes.tsv"; exit 2; }
got=$(sha256sum "$out" | cut -d' ' -f1)
# Формат: LF без CR, без BOM, перевод строки в конце (общие правила CONVENTIONS.md)
if head -c3 "$out" | grep -q $'\xEF\xBB\xBF'; then echo "warn  в начале файла BOM"; fi
if grep -q $'\r' "$out"; then echo "warn  в файле есть CR"; fi
if [ -s "$out" ] && [ "$(tail -c1 "$out" | od -An -c | tr -d ' ')" != '\n' ]; then echo "warn  нет LF в конце"; fi
if [ "$got" = "$want" ]; then echo "PASS  $track/$scale $got"; exit 0; fi
echo "FAIL  $track/$scale"
echo "      got  $got"
echo "      want $want"
exit 1
