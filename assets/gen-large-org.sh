#!/bin/bash
# Generate a large org-mode test file for performance benchmarking.
set -euo pipefail
cd "$(dirname "$0")"
OUT="large_test.org"
echo "#+TITLE: Large Org Performance Test" > "$OUT"
echo "#+DATE: $(date +%Y-%m-%d)" >> "$OUT"
echo "" >> "$OUT"

for h1 in $(seq 1 20); do
  echo "* Heading $h1: Topic Area ${h1}" >> "$OUT"
  echo "" >> "$OUT"
  echo "Introduction to topic area ${h1}. This section covers various aspects" >> "$OUT"
  echo "of the subject matter, including theoretical foundations, practical" >> "$OUT"
  echo "applications, and future directions for research and development." >> "$OUT"
  echo "" >> "$OUT"
  for h2 in $(seq 1 5); do
    echo "** Section ${h1}.${h2}: Subtopic Details" >> "$OUT"
    echo "" >> "$OUT"
    echo "Detailed discussion of subtopic ${h2} under heading ${h1}." >> "$OUT"
    echo "This paragraph provides context and explains the relationship" >> "$OUT"
    echo "between this subtopic and the broader theme. Performance" >> "$OUT"
    echo "characteristics of the editor should remain smooth even with" >> "$OUT"
    echo "thousands of lines of structured content like this." >> "$OUT"
    echo "" >> "$OUT"
    # Table
    echo "| Name        | Value   | Status    |" >> "$OUT"
    echo "|-------------+---------+-----------|" >> "$OUT"
    for row in $(seq 1 5); do
      echo "| Item-${h1}.${h2}.${row}  | $((row * h1))      | active    |" >> "$OUT"
    done
    echo "" >> "$OUT"
    # Code block
    echo "#+begin_src python" >> "$OUT"
    echo "def process_${h1}_${h2}(data):" >> "$OUT"
    echo "    \"\"\"Process data for section ${h1}.${h2}.\"\"\"" >> "$OUT"
    echo "    result = []" >> "$OUT"
    echo "    for item in data:" >> "$OUT"
    echo "        if item.valid:" >> "$OUT"
    echo "            result.append(item.transform())" >> "$OUT"
    echo "    return result" >> "$OUT"
    echo "#+end_src" >> "$OUT"
    echo "" >> "$OUT"
    # Link and checkboxes
    echo "See [[*Heading $h1: Topic Area ${h1}][back to parent]] for context." >> "$OUT"
    echo "" >> "$OUT"
    echo "- [X] Review section ${h1}.${h2}" >> "$OUT"
    echo "- [ ] Update documentation" >> "$OUT"
    echo "- [ ] Add benchmarks" >> "$OUT"
    echo "" >> "$OUT"
  done
done

LINES=$(wc -l < "$OUT")
echo "Generated $OUT ($LINES lines)"
