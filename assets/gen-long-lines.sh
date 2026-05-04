#!/bin/bash
# Generate a file with extremely long lines for horizontal scroll testing.
set -euo pipefail
cd "$(dirname "$0")"
OUT="long_lines_test.txt"
> "$OUT"

# Helper: repeat a string N times
repeat_str() {
  local str="$1" count="$2" result=""
  for ((i=0; i<count; i++)); do
    result+="$str"
  done
  echo "$result"
}

# Normal lines (reference baseline)
for i in $(seq 1 20); do
  echo "Normal line $i: This is a regular-length line for comparison." >> "$OUT"
done

# 100-char lines
for i in $(seq 1 20); do
  printf "Medium-%03d: %s\n" "$i" "$(repeat_str 'abcdefghij' 9)" >> "$OUT"
done

# 1K-char lines (minified JSON-like)
for i in $(seq 1 20); do
  line="{\"id\":$i,\"data\":["
  for j in $(seq 1 50); do
    line+="{\"key\":\"field_${j}\",\"value\":\"$(repeat_str 'x' 10)\"},"
  done
  line+="null]}"
  echo "$line" >> "$OUT"
done

# 10K-char lines (CSV-like)
for i in $(seq 1 20); do
  line="row_$i"
  for j in $(seq 1 200); do
    line+=",cell_${i}_${j}_$(repeat_str 'data' 10)"
  done
  echo "$line" >> "$OUT"
done

# 50K-char lines (base64-like blobs)
for i in $(seq 1 5); do
  echo "$(repeat_str 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/' 782)" >> "$OUT"
done

# Long prose lines (no delimiters, just wrapped text repeated)
PROSE="The quick brown fox jumps over the lazy dog. "
for i in $(seq 1 10); do
  echo "$(repeat_str "$PROSE" $((i * 100)))" >> "$OUT"
done

LINES=$(wc -l < "$OUT")
SIZE=$(wc -c < "$OUT")
echo "Generated $OUT ($LINES lines, $SIZE bytes)"
