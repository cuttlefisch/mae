#!/usr/bin/env bash
# Badge diagnostic script — run with:
#   MAE_BADGE_TOKEN=ghp_xxx MAE_BADGE_GIST=6f6375e4dc527a9953e6898124329f4c bash scripts/test-badges.sh

set -euo pipefail

TOKEN="${MAE_BADGE_TOKEN:?Set MAE_BADGE_TOKEN env var}"
GIST="${MAE_BADGE_GIST:?Set MAE_BADGE_GIST env var}"

echo "=== Step 1: Check token scopes ==="
SCOPES=$(curl -sI -H "Authorization: token $TOKEN" https://api.github.com/user | grep -i "x-oauth-scopes:" || echo "(no scopes header — might be fine-grained token)")
echo "$SCOPES"
if echo "$SCOPES" | grep -q "(no scopes header"; then
    echo "⚠  No x-oauth-scopes header. This might be a fine-grained PAT (which can't access gists). Use a classic PAT with 'gist' scope."
fi

echo ""
echo "=== Step 2: Check gist exists ==="
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    -H "Authorization: token $TOKEN" \
    "https://api.github.com/gists/$GIST")
echo "GET /gists/$GIST → HTTP $HTTP_CODE"

if [ "$HTTP_CODE" = "404" ]; then
    echo "❌ Gist not found. Either:"
    echo "   1. The gist ID is wrong (should be the 32-char hex ID, not the full URL)"
    echo "   2. The gist was deleted"
    echo "   3. The gist is owned by a different account than the token"
    echo ""
    echo "   To create a new gist:"
    echo "   curl -X POST -H 'Authorization: token \$MAE_BADGE_TOKEN' \\"
    echo "     https://api.github.com/gists \\"
    echo "     -d '{\"public\":true,\"files\":{\"mae-tests.json\":{\"content\":\"{}\"}}}'"
    exit 1
elif [ "$HTTP_CODE" = "401" ]; then
    echo "❌ Unauthorized. Token is invalid or expired."
    exit 1
elif [ "$HTTP_CODE" != "200" ]; then
    echo "❌ Unexpected status. Full response:"
    curl -s -H "Authorization: token $TOKEN" "https://api.github.com/gists/$GIST" | head -20
    exit 1
fi

echo "✅ Gist exists and is accessible"

echo ""
echo "=== Step 3: Test PATCH (write badge data) ==="

# Count tests
echo "Counting tests..."
OUTPUT=$(cargo test --workspace --exclude mae-gui 2>&1)
TOTAL=$(echo "$OUTPUT" | grep -oP '\d+ passed' | awk '{s+=$1} END {print s}')
FORMATTED=$(printf "%'d" "$TOTAL")
echo "Tests: $FORMATTED passing"

# Count LOC
echo "Counting lines of code..."
if command -v tokei &>/dev/null; then
    JSON=$(tokei crates/ modules/ scheme/ -t Rust,Scheme,TOML -o json)
    CODE=$(echo "$JSON" | python3 -c "import sys,json; print(json.load(sys.stdin)['Total']['code'])")
    LOC="~$((CODE / 1000))k"
else
    LOC="n/a (install tokei)"
fi
echo "LOC: $LOC"

# Update test badge
echo ""
echo "Updating test badge..."
RESPONSE=$(curl -s -w "\n%{http_code}" \
    -X PATCH \
    -H "Authorization: token $TOKEN" \
    -H "Content-Type: application/json" \
    "https://api.github.com/gists/$GIST" \
    -d "{\"files\":{\"mae-tests.json\":{\"content\":\"{\\\"schemaVersion\\\":1,\\\"label\\\":\\\"tests\\\",\\\"message\\\":\\\"$FORMATTED passing\\\",\\\"color\\\":\\\"brightgreen\\\"}\"}}}")

BODY=$(echo "$RESPONSE" | head -n -1)
CODE_HTTP=$(echo "$RESPONSE" | tail -1)
echo "PATCH mae-tests.json → HTTP $CODE_HTTP"

if [ "$CODE_HTTP" = "200" ]; then
    echo "✅ Test badge updated!"
else
    echo "❌ PATCH failed:"
    echo "$BODY" | head -10
fi

# Update LOC badge
if [ "$LOC" != "n/a (install tokei)" ]; then
    echo ""
    echo "Updating LOC badge..."
    RESPONSE=$(curl -s -w "\n%{http_code}" \
        -X PATCH \
        -H "Authorization: token $TOKEN" \
        -H "Content-Type: application/json" \
        "https://api.github.com/gists/$GIST" \
        -d "{\"files\":{\"mae-loc.json\":{\"content\":\"{\\\"schemaVersion\\\":1,\\\"label\\\":\\\"lines of code\\\",\\\"message\\\":\\\"$LOC\\\",\\\"color\\\":\\\"informational\\\"}\"}}}")

    CODE_HTTP=$(echo "$RESPONSE" | tail -1)
    echo "PATCH mae-loc.json → HTTP $CODE_HTTP"
    if [ "$CODE_HTTP" = "200" ]; then
        echo "✅ LOC badge updated!"
    fi
fi

echo ""
echo "=== Done ==="
echo "Badge URLs:"
echo "  Tests: https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/cuttlefisch/$GIST/raw/mae-tests.json"
echo "  LOC:   https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/cuttlefisch/$GIST/raw/mae-loc.json"
