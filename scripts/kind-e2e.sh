#!/usr/bin/env bash
# End-to-end check through the api on the kind cluster (`just kind-e2e`):
# logins with codes from Mailpit, a fresh course, roles, upload → Kaniko build
# → validation → verdict, egress, submissions → admission → grading, dates,
# regrades and their notices, visibility before and after release, activation.
# Needs a cluster from `just kind-up`, `just kind-load-jobs`, `just kind-load-api`
# and `just kind-deploy`. Exits non-zero if any check fails.
set -uo pipefail

cd "$(dirname "$0")/.."
source scripts/kind-lib.sh
NS=pedagog-builds
RUNS=pedagog-grading
EXAMPLE=recipes/examples/linked-list-python
EXAMPLE_ID=linked-list-python
UPLOAD_EXAMPLE=recipes/examples/linked-list-python-upload
UPLOAD_ID=linked-list-python-upload
# A fresh course each run; the personas (and their sessions) are reused.
COURSE=e2e-$(date +%s)
D=kind.local
export HAMMER_RECIPES="$PWD/recipes"

failures=0
pass() { echo "PASS  $1"; }
fail() { echo "FAIL  $1"; failures=$((failures + 1)); }
k() { kubectl --context "$KIND_CTX" "$@"; }
sql() { k -n pedagog-data exec deploy/postgres -- psql -U pedagog -d pedagog -tAc "$1"; }
# hammer's error report wraps long lines: join them (and drop its colors and frame).
flat() { sed 's/\x1b\[[0-9;]*m//g; s/[│×╰─╭]//g' | tr -s ' \n' ' '; }
# An RFC 3339 time HOURS from now.
at() { python3 -I -c 'import datetime as d, sys; print((d.datetime.now(d.timezone.utc) + d.timedelta(hours=float(sys.argv[1]))).isoformat(timespec="seconds"))' "$1"; }

cargo build -q -p hammer
upload() { kind_as prof upload "$1" -c "$COURSE" -q "${@:2}" 2>&1; }
build_id() { grep -oE "build [0-9]+" | head -1 | grep -oE "[0-9]+"; }
# submit STUDENT ASSIGNMENT ARGS...
submit() { kind_as "$1" submit "$2" "${@:3}" -c "$COURSE" 2>&1; }
submission_id() { grep -oE "^submission [0-9]+" | grep -oE "[0-9]+"; }
# The latest run of each of the course's submissions to the upload example by
# users whose email matches LIKE.
runs_of() {
    echo "SELECT g.status, g.score, g.total, g.notify, g.changes, g.reason, g.build_id, u.email
            FROM grading_runs g JOIN submissions s ON s.id = g.submission_id JOIN users u ON u.id = s.user_id
            JOIN assignments a ON a.id = s.assignment_id
           WHERE a.course_id = '$COURSE' AND a.slug = '$UPLOAD_ID' AND u.email LIKE '$1'"
}

kind_forwards || exit 1
work=$(mktemp -d)
trap 'kind_unforward; rm -rf "$work"' EXIT

echo "== 0. personas log in with codes from Mailpit; a fresh course $COURSE"
k -n pedagog-system exec deploy/pedagog-api -- /usr/local/bin/pedagog-api add-admin "e2e-admin@$D" >/dev/null
STUDENTS="good buggy zipped broken late"
ok=1
kind_session "e2e-admin@$D" admin || ok=0
kind_as admin users role "e2e-prof@$D" staff >/dev/null || ok=0
kind_session "e2e-prof@$D" prof || ok=0
kind_as prof courses create "$COURSE" "End-to-end checks" >/dev/null || ok=0
kind_as prof members add "e2e-ta@$D" --role grader -c "$COURSE" >/dev/null || ok=0
emails=""
for s in $STUDENTS burst1 burst2 burst3 burst4 burst5 burst6; do emails="$emails e2e-$s@$D"; done
kind_as prof members add $emails -c "$COURSE" >/dev/null || ok=0
kind_session "e2e-ta@$D" ta || ok=0
for s in $STUDENTS; do kind_session "e2e-$s@$D" "$s" || ok=0; done
if [ $ok = 1 ]; then pass "admin, instructor, grader and 5 students logged in; course created"; else fail "personas"; exit 1; fi

echo "== 1. the example builds and validates"
out=$(upload "$EXAMPLE" --force --wait)
if echo "$out" | grep -q "validation passed: 10.0 / 10.0"; then pass "example validates (10/10)"; else fail "example validates: $out"; fi
# Open, so that a submission is refused for its platform (check 11), not its dates.
kind_as prof dates set "$EXAMPLE_ID" --open always -c "$COURSE" >/dev/null 2>&1

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

echo "== 8. the upload example builds and validates, and opens"
out=$(upload "$UPLOAD_EXAMPLE" --force --wait)
dates=$(kind_as prof dates set "$UPLOAD_ID" --open always -c "$COURSE" 2>&1)
if echo "$out" | grep -q "validation passed: 10.0 / 10.0"; then pass "upload example validates (10/10)"; else fail "upload example validates: $out $dates"; fi
SOLUTION="$UPLOAD_EXAMPLE/solution"
cp -r "$SOLUTION" "$work/buggy-sub"
sed -i.bak 's/raise IndexError("pop from empty list")/return None/' "$work/buggy-sub/linked_list.py"
rm "$work/buggy-sub/linked_list.py.bak"
mkdir -p "$work/broken" && printf 'class LinkedList(:\n' > "$work/broken/linked_list.py"
printf 'x = 1\n' > "$work/other.py"

echo "== 9. submissions are graded, and students see their results"
out=$(submit good "$UPLOAD_ID" "$SOLUTION" --wait)
good_first=$(echo "$out" | submission_id)
if echo "$out" | grep -q "(counts), on time: 10/10 on the tests shown"; then pass "the solution grades 10/10"; else fail "the solution grades 10/10: $out"; fi
out=$(submit buggy "$UPLOAD_ID" "$work/buggy-sub" --wait)
if echo "$out" | grep -q "9/10 on the tests shown" && echo "$out" | grep -qE "pop_front +fail"; then
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
out=$(submit zipped "$UPLOAD_ID" "$work/hw1.zip" --wait)
if echo "$out" | grep -q "dropped the wrapping folder hw1/" && echo "$out" | grep -q "ignored by the assignment's rules: notes.txt" \
    && echo "$out" | grep -q "dropped 2 OS file" && echo "$out" | grep -q "10/10 on the tests shown"; then
    pass "zip: wrapping folder, notes.txt and OS clutter dropped; graded 10/10"
else
    fail "zip submission: $out"
fi

echo "== 11. refused and failed submissions"
out=$(submit buggy "$UPLOAD_ID" "$work/other.py" | flat)
if echo "$out" | grep -q "422" && echo "$out" | grep -q "missing required files: linked_list.py"; then
    pass "a submission without linked_list.py is refused"
else
    fail "missing required file: $out"
fi
out=$(submit good "$EXAMPLE_ID" "$SOLUTION" | flat)
if echo "$out" | grep -q "409" && echo "$out" | grep -q "arrives with sessions"; then
    pass "a workspace assignment refuses uploads"
else
    fail "workspace assignment: $out"
fi
out=$(submit broken "$UPLOAD_ID" "$work/broken" --wait); status=$?
out=$(echo "$out" | flat)
if [ $status -ne 0 ] && echo "$out" | grep -q "couldn't be graded: the tests couldn't run with your code" \
    && echo "$out" | grep -q "contact course staff"; then
    pass "a syntax error is a plain sentence for the student, not 0/0"
else
    fail "syntax error (exit $status): $out"
fi

echo "== 12. a burst waits in the queue and never exceeds the quota"
for n in 1 2 3 4 5 6; do submit prof "$UPLOAD_ID" "$SOLUTION" --for "e2e-burst$n@$D" >/dev/null; done
most=0
for _ in $(seq 1 60); do
    # By phase, as the quota counts them (a just-finished pod still shows as running).
    running=$(k -n "$RUNS" get pods -l pedagog.io/role=grade --no-headers \
        --field-selector=status.phase!=Succeeded,status.phase!=Failed 2>/dev/null | wc -l | tr -d ' ')
    [ "$running" -gt "$most" ] && most=$running
    left=$(sql "SELECT count(*) FROM ($(runs_of 'e2e-burst%')) r WHERE status IN ('queued', 'grading')")
    [ "$left" = 0 ] && break
    sleep 1
done
graded=$(sql "SELECT count(*) FROM ($(runs_of 'e2e-burst%')) r WHERE status = 'graded'")
if [ "$left" = 0 ] && [ "$graded" = 6 ] && [ "$most" -le 3 ]; then
    pass "6 submissions drained, at most $most grading at once (quota: 3)"
else
    fail "burst: $graded/6 graded, $left left, $most at once"
fi

echo "== 13. roles"
# denied PERSONA ARGS...: hammer refuses (and not because the api is down).
denied() {
    local out
    out=$(kind_as "$@" 2>&1) && return 1
    ! echo "$out" | grep -q "cannot reach"
}
ok=1
denied good upload "$UPLOAD_EXAMPLE" -c "$COURSE" -q || { ok=0; echo "  a student uploaded"; }
denied good results list "$UPLOAD_ID" -c "$COURSE" || { ok=0; echo "  a student listed results"; }
denied good courses create "$COURSE-x" "Nope" || { ok=0; echo "  a student created a course"; }
denied ta results release "$UPLOAD_ID" -c "$COURSE" || { ok=0; echo "  a grader released results"; }
denied ta members add "e2e-x@$D" -c "$COURSE" || { ok=0; echo "  a grader added a member"; }
kind_as ta results list "$UPLOAD_ID" -c "$COURSE" 2>&1 | grep -q "e2e-good@$D" || { ok=0; echo "  a grader can't list results"; }
if [ $ok = 1 ]; then pass "students can't manage; graders see results but can't release or add members"; else fail "roles"; fi

echo "== 14. late, closed, and on behalf"
kind_as prof dates set "$UPLOAD_ID" --due "$(at -1)" --late-due "$(at 1)" -c "$COURSE" >/dev/null 2>&1
out=$(submit late "$UPLOAD_ID" "$SOLUTION")
if echo "$out" | grep -q "late by"; then pass "after the due date, a submission is late"; else fail "late: $out"; fi
kind_as prof dates set "$UPLOAD_ID" --due "$(at -2)" --late-due "$(at -1)" -c "$COURSE" >/dev/null 2>&1
out=$(submit late "$UPLOAD_ID" "$SOLUTION" | flat)
if echo "$out" | grep -q "deadline has passed"; then pass "after late_due, a student's submission is refused"; else fail "closed: $out"; fi
out=$(submit prof "$UPLOAD_ID" "$SOLUTION" --for "e2e-late@$D" --at last --wait)
mine=$(kind_as late results show "$UPLOAD_ID" -c "$COURSE" 2>&1)
if echo "$out" | grep -q "submitted for e2e-late@$D by e2e-prof@$D" \
    && echo "$mine" | grep -q "submitted by e2e-prof@$D on your behalf"; then
    pass "staff submit on a student's behalf when closed, and the student sees who"
else
    fail "on behalf: $out / $mine"
fi
kind_as prof dates clear "$UPLOAD_ID" --due --late-due -c "$COURSE" >/dev/null 2>&1

echo "== 15. a regrade reports what changed"
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
updated=$(kind_mail "e2e-good@$D" "results updated")
out=$(upload "$work/v2" --force --regrade --notify changed --wait)
v2=$(echo "$out" | build_id)
for _ in $(seq 1 90); do
    left=$(sql "SELECT count(*) FROM grading_runs WHERE build_id = ${v2:-0} AND status IN ('queued', 'grading')")
    [ "$left" = 0 ] && break
    sleep 2
done
regrade=$(sql "SELECT score || '/' || total || ' ' || notify || ' ' || (changes->'added')::text
               FROM ($(runs_of "e2e-good@$D")) r WHERE reason = 'regrade' AND build_id = ${v2:-0}")
if [ "$regrade" = '11/11 true ["pop-shrinks"]' ]; then
    pass "regraded 11/11 with pop-shrinks added, student to be notified"
else
    fail "regrade of e2e-good against build ${v2:-?}: '$regrade' ($out)"
fi

echo "== 16. the student is emailed about the regrade"
mail=$(kind_mail_after "e2e-good@$D" "results updated" "$updated" 90)
if [ -n "$mail" ]; then
    pass "regrade notice arrived: ${mail#*$'\t'}"
else
    fail "no regrade notice for e2e-good in 90 s"
fi

echo "== 17. visibility before and after release"
kind_as prof visibility set "$UPLOAD_ID" --test many --score after_publish -c "$COURSE" >/dev/null 2>&1
before=$(kind_as good results show "$UPLOAD_ID" -c "$COURSE" 2>&1)
released=$(kind_mail "e2e-good@$D" "results released")
out=$(kind_as prof results release "$UPLOAD_ID" -c "$COURSE" 2>&1)
after=$(kind_as good results show "$UPLOAD_ID" -c "$COURSE" 2>&1)
if echo "$before" | grep -q "9/9 on the tests shown" && echo "$before" | grep -q "hidden until" \
    && echo "$after" | grep -q "11/11 on the tests shown"; then
    pass "a test shown on release is hidden before it (9/9), shown after (11/11)"
else
    fail "visibility: before: $before / after: $after"
fi
mail=$(kind_mail_after "e2e-good@$D" "results released" "$released" 90)
if echo "$out" | grep -q "emailing" && [ -n "$mail" ]; then pass "release emails students ($out)"; else fail "release email: $out / '$mail'"; fi

echo "== 18. staff activate an earlier submission until a newer one arrives"
submit good "$UPLOAD_ID" "$work/buggy-sub" --wait >/dev/null
kind_as prof submissions activate "${good_first:-0}" -c "$COURSE" >/dev/null 2>&1
counting=$(kind_as prof results list "$UPLOAD_ID" --search e2e-good -c "$COURSE" 2>&1)
newer=$(submit good "$UPLOAD_ID" "$SOLUTION" --wait | submission_id)
latest=$(kind_as prof results list "$UPLOAD_ID" --search e2e-good -c "$COURSE" 2>&1)
if echo "$counting" | grep -q "#${good_first:-x} " && echo "$latest" | grep -q "#${newer:-x} "; then
    pass "submission $good_first counts once activated; $newer counts once submitted"
else
    fail "activation: $counting / $latest"
fi

echo
if [ $failures -eq 0 ]; then echo "all checks passed"; else echo "$failures check(s) failed"; fi
exit $failures
