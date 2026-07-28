#!/usr/bin/env bash
# 3-node d-lmdb-server + HAProxy smoke test.
#
# The primary story: a client only ever talks to the LB (localhost:8080).
# It never needs to know which node is leader, or that a leader even
# exists — HAProxy's health-check-based routing (compose/haproxy.cfg)
# handles that, including automatically re-routing after a leader crash.
#
# Usage: compose/smoke-test.sh   (run from repo root)
#
# DLMDB_SKIP_BUILD=1: reuse whatever image is already loaded under the
# d-lmdb-server:compose tag instead of building — for CI, which builds one
# specific platform first (see .github/workflows/docker-release.yml) and
# wants this script to test exactly that image, not rebuild for the host's
# native platform.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

declare -A PORT=( [node1]=18081 [node2]=18082 [node3]=18083 )
LB_URL="http://localhost:8080"
COMPOSE="docker compose"

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; cleanup_on_fail; exit 1; }

cleanup_on_fail() {
    echo "--- container logs on failure ---"
    $COMPOSE logs --tail=50
}

command -v jq >/dev/null || { echo "jq is required (brew install jq)"; exit 1; }

echo "== bringing up 3-node cluster + HAProxy =="
if [ "${DLMDB_SKIP_BUILD:-}" = "1" ]; then
    $COMPOSE up -d
else
    $COMPOSE up -d --build
fi

echo "== waiting for all 3 nodes to report healthy =="
for name in node1 node2 node3; do
    for _ in $(seq 1 60); do
        status=$($COMPOSE ps -q "$name" | xargs docker inspect --format '{{.State.Health.Status}}' 2>/dev/null || echo "starting")
        [ "$status" = "healthy" ] && break
        sleep 1
    done
    [ "$status" = "healthy" ] || fail "$name never became healthy (last status: $status)"
done
pass "all 3 nodes healthy"

find_leader_directly() {
    for name in node1 node2 node3; do
        code=$(curl -s -o /dev/null -w '%{http_code}' "http://localhost:${PORT[$name]}/primary")
        [ "$code" = "200" ] && { echo "$name"; return; }
    done
    echo ""
}

echo "== confirming exactly one node is leader (direct check, precondition) =="
leader=$(find_leader_directly)
[ -n "$leader" ] || fail "no node reports /primary=200"
primary_count=0
for name in node1 node2 node3; do
    code=$(curl -s -o /dev/null -w '%{http_code}' "http://localhost:${PORT[$name]}/primary")
    [ "$code" = "200" ] && primary_count=$((primary_count + 1))
done
[ "$primary_count" -eq 1 ] || fail "expected exactly 1 leader, got $primary_count"
pass "exactly one leader: $leader"

echo "== waiting for HAProxy to pick up the leader in write_backend =="
for _ in $(seq 1 30); do
    code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$LB_URL/kv/lb-warmup" -d 'x' || echo "000")
    [ "$code" = "204" ] && break
    sleep 1
done
[ "$code" = "204" ] || fail "LB never started routing writes successfully (last code: $code)"
pass "LB is routing writes"

echo "== PUT through the LB only — client never touches a node directly =="
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$LB_URL/kv/smoke-key" -d 'smoke-value')
[ "$code" = "204" ] || fail "PUT via LB returned $code, expected 204"
pass "PUT via LB succeeded"

echo "== GET through the LB (linearizable — read_backend round-robins across all"
echo "   nodes, and a default eventual read can legitimately race a follower that"
echo "   hasn't applied this write yet; linearizable avoids that race) =="
value=$(curl -s "$LB_URL/kv/smoke-key?level=linearizable")
[ "$value" = "smoke-value" ] || fail "GET via LB returned '$value', expected 'smoke-value'"
pass "GET via LB returned correct value"

echo "== (supplementary, non-blocking) direct PUT to a follower still returns leader_hint =="
follower=""
for name in node1 node2 node3; do
    [ "$name" != "$leader" ] && { follower="$name"; break; }
done
body_file=$(mktemp)
code=$(curl -s -o "$body_file" -w '%{http_code}' -X PUT "http://localhost:${PORT[$follower]}/kv/smoke-key" -d 'wrong-node')
body=$(cat "$body_file")
rm -f "$body_file"
if [ "$code" = "503" ] && [ -n "$(echo "$body" | jq -r '.leader_hint.address // empty')" ]; then
    pass "follower still rejects direct writes with a leader_hint present (known limitation: address may not be externally reachable, see leader-hint-http-address-mismatch.md — not the primary routing path, LB is)"
else
    echo "NOTE: direct-to-follower rejection didn't look as expected (code=$code, body=$body) — not failing the suite, this path is superseded by the LB"
fi

echo "== killing the leader ($leader) to force re-election =="
leader_container=$($COMPOSE ps -q "$leader")
docker kill "$leader_container" >/dev/null
old_leader="$leader"

echo "== confirming a new leader is elected (direct check) =="
new_leader=""
for _ in $(seq 1 60); do
    for name in node1 node2 node3; do
        [ "$name" = "$old_leader" ] && continue
        code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 1 "http://localhost:${PORT[$name]}/primary" 2>/dev/null || echo "000")
        [ "$code" = "200" ] && { new_leader="$name"; break 2; }
    done
    sleep 1
done
[ -n "$new_leader" ] || fail "no new leader elected within 60s of killing $old_leader"
pass "new leader elected after crash: $new_leader"

echo "== the real point: write through the LB again, with ZERO client-side changes =="
success=""
for _ in $(seq 1 30); do
    code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT "$LB_URL/kv/post-failover-key" -d 'still-alive' || echo "000")
    [ "$code" = "204" ] && { success=1; break; }
    sleep 1
done
[ -n "$success" ] || fail "LB never resumed routing writes after leader crash (last code: $code)"
value=$(curl -s "$LB_URL/kv/post-failover-key?level=linearizable")
[ "$value" = "still-alive" ] || fail "post-failover GET via LB returned '$value'"
pass "LB automatically re-routed to the new leader — client never had to know anything changed"

echo "== tearing down =="
$COMPOSE down -v

echo
echo "ALL SMOKE TESTS PASSED"
