#!/usr/bin/env bash
# End-to-end check of the assignment pipeline on the kind cluster
# (`just kind-e2e`): upload → Kaniko build → validation → verdict.
# Needs a cluster from `just kind-up`, `just kind-load-jobs` and `just kind-deploy`.
# Exits non-zero if any check fails.
set -uo pipefail

cd "$(dirname "$0")/.."
CTX=kind-pedagog
NS=pedagog-builds
EXAMPLE=recipes/examples/linked-list-python
PORT=18081
URL=http://localhost:$PORT
export HAMMER_RECIPES="$PWD/recipes"

failures=0
pass() { echo "PASS  $1"; }
fail() { echo "FAIL  $1"; failures=$((failures + 1)); }
k() { kubectl --context "$CTX" "$@"; }
sql() { k -n pedagog-data exec deploy/postgres -- psql -U pedagog -d pedagog -tAc "$1"; }

cargo build -q -p hammer
HAMMER=target/debug/hammer
upload() { "$HAMMER" upload "$1" --jobs-url "$URL" -q "${@:2}" 2>&1; }
build_id() { grep -oE "build [0-9]+" | head -1 | grep -oE "[0-9]+"; }

k -n "$NS" port-forward svc/pedagog-jobs "$PORT:8080" >/dev/null 2>&1 &
PF=$!
disown $PF
work=$(mktemp -d)
trap 'kill $PF 2>/dev/null; rm -rf "$work"' EXIT
sleep 2

echo "== 1. the example builds and validates"
out=$(upload "$EXAMPLE" --force --wait)
if echo "$out" | grep -q "validation passed: 10.0 / 10.0"; then pass "example validates (10/10)"; else fail "example validates: $out"; fi

echo "== 2. identical content dispatches nothing"
out=$(upload "$EXAMPLE")
if echo "$out" | grep -q "already built and validated"; then pass "re-upload is recognized"; else fail "re-upload is recognized: $out"; fi

echo "== 3. a solution that disagrees with the tests fails validation"
cp -r "$EXAMPLE" "$work/buggy"
sed -i.bak 's/raise IndexError("pop from empty list")/return None/' "$work/buggy/solution/linked_list.py"
out=$(upload "$work/buggy" --wait); status=$?
if [ $status -ne 0 ] && echo "$out" | grep -q "pop-front: expected the solution to pass, but it failed"; then
    pass "buggy solution fails with the mismatch"
else
    fail "buggy solution fails with the mismatch (exit $status): $out"
fi
current=$(sql "SELECT b.validation_status FROM assignments a JOIN assignment_builds b ON b.id = a.current_build_id WHERE a.id = 'linked-list-python'")
if [ "$current" = passed ]; then pass "a failed build doesn't replace the current one"; else fail "current build is '$current'"; fi

echo "== 4. a newer upload supersedes an attempt in flight"
first=$(upload "$EXAMPLE" --force | build_id)
second=$(upload "$EXAMPLE" --force | build_id)
superseded=$(sql "SELECT build_status FROM assignment_builds WHERE id = ${first:-0}")
if [ "$superseded" = superseded ]; then pass "build $first superseded by $second"; else fail "build ${first:-?} is '$superseded'"; fi

echo "== 5. an attempt resumes after jobs restarts"
for _ in $(seq 1 150); do
    stage=$(sql "SELECT validation_status FROM assignment_builds WHERE id = ${second:-0}")
    [ "$stage" = running ] && break
    sleep 2
done
k -n "$NS" rollout restart deploy/pedagog-jobs >/dev/null
k -n "$NS" rollout status deploy/pedagog-jobs --timeout=180s >/dev/null
for _ in $(seq 1 150); do
    final=$(sql "SELECT validation_status FROM assignment_builds WHERE id = ${second:-0} AND finished_at IS NOT NULL")
    [ -n "$final" ] && break
    sleep 2
done
if [ "$final" = passed ]; then pass "build $second resumed and passed (restarted during '$stage')"; else fail "build ${second:-?} after restart: '$final'"; fi
# The restart dropped the port-forward.
kill $PF 2>/dev/null
k -n "$NS" port-forward svc/pedagog-jobs "$PORT:8080" >/dev/null 2>&1 &
PF=$!
disown $PF

echo "== 6. validation pods can reach Garage but not the internet (mode: deny)"
k -n "$NS" delete pod e2e-netprobe --ignore-not-found >/dev/null
k -n "$NS" run e2e-netprobe --restart=Never --image=busybox:1.36 \
    --labels=pedagog.io/role=validate,pedagog.io/network=deny --command -- sh -c '
    sleep 8  # network policies apply a few seconds after a pod starts
    wget -q -T 5 -O /dev/null http://garage-service.pedagog-data.svc:3900/ 2>&1 | grep -q "403" && echo garage-ok
    wget -q -T 5 -O /dev/null http://1.1.1.1/ >/dev/null 2>&1 && echo internet-open || echo internet-blocked' >/dev/null
k -n "$NS" wait --for=jsonpath='{.status.phase}'=Succeeded pod/e2e-netprobe --timeout=90s >/dev/null
probe=$(k -n "$NS" logs e2e-netprobe)
k -n "$NS" delete pod e2e-netprobe --wait=false >/dev/null
if echo "$probe" | grep -q garage-ok && echo "$probe" | grep -q internet-blocked; then
    pass "egress: Garage reachable, internet blocked"
else
    fail "egress: $probe"
fi

echo "== 7. under mode: allow, the internet is reachable but node IPs are not"
node_ip=$(k get nodes -o jsonpath='{.items[0].status.addresses[?(@.type=="InternalIP")].address}')
k -n "$NS" delete pod e2e-netprobe-allow --ignore-not-found >/dev/null
k -n "$NS" run e2e-netprobe-allow --restart=Never --image=busybox:1.36 \
    --labels=pedagog.io/role=validate,pedagog.io/network=allow --command -- sh -c "
    sleep 8  # network policies apply a few seconds after a pod starts
    wget -q -T 5 -O /dev/null http://1.1.1.1/ >/dev/null 2>&1 && echo internet-open || echo internet-blocked
    nc -z -w 3 $node_ip 10250 && echo node-open || echo node-blocked" >/dev/null
k -n "$NS" wait --for=jsonpath='{.status.phase}'=Succeeded pod/e2e-netprobe-allow --timeout=90s >/dev/null
probe=$(k -n "$NS" logs e2e-netprobe-allow)
k -n "$NS" delete pod e2e-netprobe-allow --wait=false >/dev/null
if echo "$probe" | grep -q internet-open && echo "$probe" | grep -q node-blocked; then
    pass "egress: internet reachable, node $node_ip (kubelet) blocked"
else
    fail "egress (allow): $probe"
fi

echo
if [ $failures -eq 0 ]; then echo "all checks passed"; else echo "$failures check(s) failed"; fi
exit $failures
