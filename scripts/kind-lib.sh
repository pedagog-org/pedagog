# Helpers for the api on the kind cluster (source it): port-forwards, persona
# logins with codes read from Mailpit, and hammer as a persona. Personas keep
# their sessions under target/kind/<name>.

KIND_CTX=${KIND_CTX:-kind-pedagog}
API=http://localhost:18082
MAILPIT=http://localhost:18025/api/v1
PERSONAS=${PERSONAS:-$PWD/target/kind}
HAMMER=${HAMMER:-$PWD/target/debug/hammer}
KIND_FORWARDS=

# kind_forward NAMESPACE SERVICE LOCAL_PORT PORT: forward unless something
# answers already. A forward opened during a rollout can attach to a
# terminating pod and die with it, so wait for the rollout first.
kind_forward() {
    local ns=$1 svc=$2 port=$3 target=$4 pf
    curl -s -o /dev/null "http://localhost:$port/" && return 0
    pkill -f "port-forward svc/$svc $port:" 2>/dev/null
    kubectl --context "$KIND_CTX" -n "$ns" rollout status "deploy/$svc" --timeout=180s >/dev/null
    for _ in $(seq 1 30); do
        kubectl --context "$KIND_CTX" -n "$ns" port-forward "svc/$svc" "$port:$target" >/dev/null 2>&1 &
        pf=$!
        disown $pf
        for _ in $(seq 1 10); do
            if curl -s -o /dev/null "http://localhost:$port/"; then
                KIND_FORWARDS="$KIND_FORWARDS $pf"
                return 0
            fi
            sleep 0.5
        done
        kill $pf 2>/dev/null
    done
    echo "cannot reach $svc through a port-forward" >&2
    return 1
}

# The api (:18082) and Mailpit (:18025); kind_unforward stops what this started.
kind_forwards() {
    kind_forward pedagog-system pedagog-api 18082 8080 && kind_forward pedagog-data mailpit 18025 8025
}
kind_unforward() {
    [ -n "$KIND_FORWARDS" ] && kill $KIND_FORWARDS 2>/dev/null
    KIND_FORWARDS=
}

# The newest message to EMAIL whose subject contains WORDS, as "ID<tab>Subject".
kind_mail() {
    curl -s -G "$MAILPIT/search" --data-urlencode "query=to:$1 subject:\"$2\"" --data-urlencode limit=1 |
        python3 -I -c 'import json, sys
m = json.load(sys.stdin)["messages"]
print(m[0]["ID"] + "\t" + m[0]["Subject"] if m else "")'
}

# kind_mail_after EMAIL WORDS BEFORE [SECONDS]: wait for a message newer than
# BEFORE (a kind_mail line, or empty) and print it.
kind_mail_after() {
    local line
    for _ in $(seq 1 "${4:-30}"); do
        line=$(kind_mail "$1" "$2")
        if [ -n "$line" ] && [ "${line%%$'\t'*}" != "${3%%$'\t'*}" ]; then
            echo "$line"
            return 0
        fi
        sleep 1
    done
    return 1
}

# kind_login EMAIL [NAME]: log in with the code mailed to EMAIL (they must be
# registered); NAME defaults to the email's local part.
kind_login() {
    local email=$1 name=${2:-${1%%@*}} before
    mkdir -p "$PERSONAS/$name"
    before=$(kind_mail "$email" "login code")
    if ! { kind_mail_after "$email" "login code" "$before" | sed 's/.* //'; } |
        XDG_CONFIG_HOME="$PERSONAS/$name" "$HAMMER" login --api "$API" --email "$email" >/dev/null; then
        echo "$name: login as $email failed" >&2
        return 1
    fi
}

# kind_session EMAIL [NAME]: log in only if NAME has no working session.
kind_session() {
    local name=${2:-${1%%@*}}
    XDG_CONFIG_HOME="$PERSONAS/$name" "$HAMMER" whoami 2>/dev/null | grep -q "^$1 " && return 0
    kind_login "$@"
}

# kind_as NAME ARGS...: hammer as a persona.
kind_as() {
    local name=$1
    shift
    XDG_CONFIG_HOME="$PERSONAS/$name" "$HAMMER" "$@"
}
