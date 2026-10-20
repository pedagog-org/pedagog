#!/usr/bin/env bash
# End-to-end check of the assignment pipeline on the kind cluster
# (`just kind-e2e`): upload → Kaniko build → validation → verdict, then
# submissions → admission → grading → regrades.
# Needs a cluster from `just kind-up`, `just kind-load-jobs` and `just kind-deploy`.
# Exits non-zero if any check fails.
set -uo pipefail

cd "$(dirname "$0")/.."
CTX=kind-pedagog
NS=pedagog-builds
RUNS=pedagog-grading
EXAMPLE=recipes/examples/linked-list-python
EXAMPLE_ID=linked-list-python
UPLOAD_EXAMPLE=recipes/examples/linked-list-python-upload
UPLOAD_ID=linked-list-python-upload
PORT=18081
URL=http://localhost:$PORT
# Uploads go straight to jobs (no login), into a course of their own.
COURSE=e2e
export HAMMER_RECIPES="$PWD/recipes"

failures=0
pass() { echo "PASS  $1"; }
fail() { echo "FAIL  $1"; failures=$((failures + 1)); }
k() { kubectl --context "$CTX" "$@"; }
sql() { k -n pedagog-data exec deploy/postgres -- psql -U pedagog -d pedagog -tAc "$1"; }

cargo build -q -p hammer
HAMMER=target/debug/hammer
upload() { "$HAMMER" upload "$1" -c "$COURSE" --jobs-url "$URL" -q "${@:2}" 2>&1; }
build_id() { grep -oE "build [0-9]+" | head -1 | grep -oE "[0-9]+"; }
# Fixed student ids, so each run's regrade covers the same few submissions.
submit() { "$HAMMER" submit "$1" "${@:3}" -c "$COURSE" --student "e2e-$2" --jobs-url "$URL" 2>&1; }
# hammer's error report wraps long lines: join them (and drop its colors and frame).
flat() { sed 's/\x1b\[[0-9;]*m//g; s/[│×]//g' | tr -s ' \n' ' '; }

# Forward to jobs and wait until it answers (a forward opened during a rollout
# can attach to a terminating pod and die with it).
forward() {
    k -n "$NS" rollout status deploy/pedagog-jobs --timeout=180s >/dev/null
    for _ in $(seq 1 30); do
        k -n "$NS" port-forward svc/pedagog-jobs "$PORT:8080" >/dev/null 2>&1 &
        PF=$!
        disown $PF
        for _ in $(seq 1 10); do
            curl -s -o /dev/null "$URL/" && return 0
            sleep 0.5
        done
        kill $PF 2>/dev/null
    done
    echo "cannot reach jobs through a port-forward" >&2
    exit 1
}
forward
sql "INSERT INTO courses (id, name) VALUES ('$COURSE', 'End-to-end checks') ON CONFLICT DO NOTHING" >/dev/null
work=$(mktemp -d)
trap 'kill $PF 2>/dev/null; rm -rf "$work"' EXIT

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
current=$(sql "SELECT b.validation_status FROM assignments a JOIN assignment_builds b ON b.id = a.current_build_id WHERE a.course_id = '$COURSE' AND a.slug = '$EXAMPLE_ID'")
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
forward

# A test-run pod: jobs' await-egress initContainer (as on validation and
# grading pods), then a shell running $3.
probe_pod() {
    local script
    script=$(printf '%s\n' "$3" | sed 's/^/        /')
    k -n "$RUNS" delete pod "$1" --ignore-not-found >/dev/null
    k -n "$RUNS" apply -f - >/dev/null <<YAML
apiVersion: v1
kind: Pod
metadata:
  name: $1
  labels: { pedagog.io/role: validate, pedagog.io/network: $2 }
spec:
  restartPolicy: Never
  initContainers:
    - name: await-egress
      image: localhost/pedagog/jobs-init:dev
      imagePullPolicy: IfNotPresent
      args: [await-egress, "kube-dns.kube-system.svc:9153", "30"]
  containers:
    - name: probe
      image: busybox:1.36
      command: [sh, -c]
      args:
        - |
$script
YAML
    k -n "$RUNS" wait --for=jsonpath='{.status.phase}'=Succeeded "pod/$1" --timeout=90s >/dev/null
    k -n "$RUNS" logs "$1" -c probe
    k -n "$RUNS" delete pod "$1" --wait=false >/dev/null
}

echo "== 6. test-run pods can reach Garage but not the internet (mode: deny)"
probe=$(probe_pod e2e-netprobe deny '
    wget -q -T 5 -O /dev/null http://garage-service.pedagog-data.svc:3900/ 2>&1 | grep -q "403" && echo garage-ok
    wget -q -T 5 -O /dev/null http://1.1.1.1/ >/dev/null 2>&1 && echo internet-open || echo internet-blocked')
if echo "$probe" | grep -q garage-ok && echo "$probe" | grep -q internet-blocked; then
    pass "egress: Garage reachable, internet blocked"
else
    fail "egress: $probe"
fi

echo "== 7. under mode: allow, the internet is reachable but node IPs are not"
node_ip=$(k get nodes -o jsonpath='{.items[0].status.addresses[?(@.type=="InternalIP")].address}')
probe=$(probe_pod e2e-netprobe-allow allow "
    wget -q -T 5 -O /dev/null http://1.1.1.1/ >/dev/null 2>&1 && echo internet-open || echo internet-blocked
    nc -z -w 3 $node_ip 10250 && echo node-open || echo node-blocked")
if echo "$probe" | grep -q internet-open && echo "$probe" | grep -q node-blocked; then
    pass "egress: internet reachable, node $node_ip (kubelet) blocked"
else
    fail "egress (allow): $probe"
fi

echo "== 8. the upload example builds and validates"
out=$(upload "$UPLOAD_EXAMPLE" --force --wait)
if echo "$out" | grep -q "validation passed: 10.0 / 10.0"; then pass "upload example validates (10/10)"; else fail "upload example validates: $out"; fi
SOLUTION="$UPLOAD_EXAMPLE/solution"
cp -r "$SOLUTION" "$work/buggy-sub"
sed -i.bak 's/raise IndexError("pop from empty list")/return None/' "$work/buggy-sub/linked_list.py"
rm "$work/buggy-sub/linked_list.py.bak"
mkdir -p "$work/broken" && printf 'class LinkedList(:\n' > "$work/broken/linked_list.py"
printf 'x = 1\n' > "$work/other.py"

echo "== 9. submissions are graded"
out=$(submit "$UPLOAD_ID" good "$SOLUTION" --wait)
if echo "$out" | grep -q "graded: 10 / 10"; then pass "the solution grades 10/10"; else fail "the solution grades 10/10: $out"; fi
out=$(submit "$UPLOAD_ID" buggy "$work/buggy-sub" --wait)
if echo "$out" | grep -q "graded: 9 / 10" && echo "$out" | grep -q "pop_front (fail)"; then
    pass "a buggy submission grades 9/10 and names the failing test"
else
    fail "a buggy submission grades 9/10: $out"
fi

echo "== 10. a Finder zip is unpacked, unwrapped and cleaned"
python3 - "$SOLUTION/linked_list.py" "$work/hw1.zip" <<'PY'
import sys, zipfile
with zipfile.ZipFile(sys.argv[2], "w") as z:
    z.write(sys.argv[1], "hw1/linked_list.py")
    z.writestr("hw1/notes.txt", "my notes")
    z.writestr("hw1/.DS_Store", "junk")
    z.writestr("__MACOSX/hw1/._linked_list.py", "junk")
PY
out=$(submit "$UPLOAD_ID" zipped "$work/hw1.zip" --wait)
if echo "$out" | grep -q "dropped the wrapping folder hw1/" && echo "$out" | grep -q "ignored by the assignment's rules: notes.txt" \
    && echo "$out" | grep -q "dropped 2 OS file" && echo "$out" | grep -q "graded: 10 / 10"; then
    pass "zip: wrapping folder, notes.txt and OS clutter dropped; graded 10/10"
else
    fail "zip submission: $out"
fi

echo "== 11. refused and failed submissions"
out=$(submit "$UPLOAD_ID" missing "$work/other.py" | flat)
if echo "$out" | grep -q "422" && echo "$out" | grep -q "missing required files: linked_list.py"; then
    pass "a submission without linked_list.py is refused"
else
    fail "missing required file: $out"
fi
out=$(submit "$EXAMPLE_ID" missing "$SOLUTION" | flat)
if echo "$out" | grep -q "409" && echo "$out" | grep -q "arrives with sessions"; then
    pass "a workspace assignment refuses uploads"
else
    fail "workspace assignment: $out"
fi
out=$(submit "$UPLOAD_ID" broken "$work/broken" --wait); status=$?
out=$(echo "$out" | flat)
if [ $status -ne 0 ] && echo "$out" | grep -q "tests couldn't all run"; then
    pass "a syntax error is a grading error, not 0/0"
else
    fail "syntax error (exit $status): $out"
fi

echo "== 12. a burst waits in the queue and never exceeds the quota"
for n in 1 2 3 4 5 6; do submit "$UPLOAD_ID" "burst$n" "$SOLUTION" >/dev/null; done
most=0
for _ in $(seq 1 60); do
    # By phase, as the quota counts them (a just-finished pod still shows as running).
    running=$(k -n "$RUNS" get pods -l pedagog.io/role=grade --no-headers \
        --field-selector=status.phase!=Succeeded,status.phase!=Failed 2>/dev/null | wc -l | tr -d ' ')
    [ "$running" -gt "$most" ] && most=$running
    left=$(sql "SELECT count(*) FROM grading_runs g JOIN submissions s ON s.id = g.submission_id
                WHERE s.student_id LIKE 'e2e-burst%' AND g.status IN ('queued', 'grading')")
    [ "$left" = 0 ] && break
    sleep 1
done
graded=$(sql "SELECT count(*) FROM (SELECT DISTINCT ON (s.student_id) g.status FROM grading_runs g
              JOIN submissions s ON s.id = g.submission_id WHERE s.student_id LIKE 'e2e-burst%'
              ORDER BY s.student_id, g.id DESC) latest WHERE status = 'graded'")
if [ "$left" = 0 ] && [ "$graded" = 6 ] && [ "$most" -le 3 ]; then
    pass "6 submissions drained, at most $most grading at once (quota: 3)"
else
    fail "burst: $graded/6 graded, $left left, $most at once"
fi

echo "== 13. a regrade reports what changed"
cp -r "$UPLOAD_EXAMPLE" "$work/v2"
cat >> "$work/v2/tests/test_linked_list.py" <<'PY'


@rubric_item("pop_front shrinks the list", points=1, id="pop-shrinks")
@number(6)
def test_pop_shrinks(score: Score):
    items = LinkedList()
    items.append(1)
    items.append(2)
    items.pop_front()
    assert len(items) == 1
    score.award()
PY
out=$(upload "$work/v2" --force --regrade --notify changed --wait)
v2=$(echo "$out" | build_id)
for _ in $(seq 1 90); do
    left=$(sql "SELECT count(*) FROM grading_runs WHERE build_id = ${v2:-0} AND status IN ('queued', 'grading')")
    [ "$left" = 0 ] && break
    sleep 2
done
regrade=$(sql "SELECT g.score || '/' || g.total || ' ' || g.notify || ' ' || (g.changes->'added')::text
               FROM grading_runs g JOIN submissions s ON s.id = g.submission_id
               WHERE s.student_id = 'e2e-good' AND g.reason = 'regrade' AND g.build_id = ${v2:-0}")
if [ "$regrade" = '11/11 true ["pop-shrinks"]' ]; then
    pass "regraded 11/11 with pop-shrinks added, student to be notified"
else
    fail "regrade of e2e-good against build ${v2:-?}: '$regrade' ($out)"
fi

echo
if [ $failures -eq 0 ]; then echo "all checks passed"; else echo "$failures check(s) failed"; fi
exit $failures
