#!/usr/bin/env bash
# Runs the emperor-mail Docker image over stdio against a GreenMail container.
#
#   scripts/docker-greenmail.sh [image]      (default image: emperor-mail:test)
#
# GreenMail and emperor-mail share one network namespace (--network container:...), so the
# mail server is on localhost and the plain IMAP/SMTP ports are allowed (plain is loopback only;
# GreenMail's TLS ports use a self-signed certificate that emperor-mail rightly refuses). The
# user name and password are made up for this run. Checks: doctor passes, MAIL_WRITABLE and
# MAIL_ALLOW_SEND are read from the environment and switch draft and send on and off (4, 5 or 6
# tools listed), and an MCP session (initialize, tools/list, list_folders) writes only JSON-RPC to
# stdout and the tool call succeeds.
set -euo pipefail

IMAGE="${1:-emperor-mail:test}"
GREENMAIL="greenmail/standalone:2.1.5"
ID="$(date +%s)$$"
NET="emperor-mail-net-$ID"
GM="emperor-mail-gm-$ID"
USER_ADDR="u$ID@example.test"
PASS="p$ID"
WORK="$(mktemp -d)"

cleanup() {
  docker rm -f "$GM" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

docker network create "$NET" >/dev/null
docker run -d --name "$GM" --network "$NET" \
  -e "GREENMAIL_OPTS=-Dgreenmail.setup.test.all -Dgreenmail.hostname=0.0.0.0 -Dgreenmail.users=u$ID:$PASS@example.test -Dgreenmail.users.login=email" \
  "$GREENMAIL" >/dev/null

MAIL_ENV=(
  -e "MAIL_USER=$USER_ADDR" -e "MAIL_PASSWORD=$PASS"
  -e IMAP_HOST=127.0.0.1 -e IMAP_PORT=3143 -e IMAP_SECURITY=plain
  -e SMTP_HOST=127.0.0.1 -e SMTP_PORT=3025 -e SMTP_SECURITY=plain
)
run() { docker run --rm -i --network "container:$GM" "${MAIL_ENV[@]}" "$@"; }
doctor() { run --entrypoint emperor-mail "$@" "$IMAGE" doctor --stdio; }

echo "== waiting for GreenMail (doctor until it passes)"
for i in $(seq 1 60); do
  if doctor >"$WORK/doctor.txt" 2>&1; then break; fi
  if [ "$i" = 60 ]; then cat "$WORK/doctor.txt"; docker logs "$GM" | tail -20; exit 1; fi
  sleep 2
done
cat "$WORK/doctor.txt"
grep -q "mode .*read-only" "$WORK/doctor.txt"
grep -q "4 of at most 8 tools" "$WORK/doctor.txt"
if grep -q "$PASS" "$WORK/doctor.txt"; then echo "doctor printed the password"; exit 1; fi

echo "== write switches from the environment"
# expect_doctor <grep pattern> <tool count> [docker -e args...]
expect_doctor() {
  local pattern=$1 tools=$2; shift 2
  doctor "$@" >"$WORK/d.txt" 2>&1 || { echo "doctor failed: $*"; cat "$WORK/d.txt"; exit 1; }
  grep -q "$pattern" "$WORK/d.txt" || { echo "no '$pattern' with $*"; cat "$WORK/d.txt"; exit 1; }
  grep -q "$tools of at most 8 tools" "$WORK/d.txt" \
    || { echo "not $tools tools with $*"; cat "$WORK/d.txt"; exit 1; }
}
expect_doctor "mode .*read-only" 4 -e MAIL_WRITABLE=false -e MAIL_ALLOW_SEND=false -e MAIL_SEND_ALLOW=
expect_doctor "mode .*read-only" 4 -e MAIL_WRITABLE= -e MAIL_ALLOW_SEND=0
expect_doctor "mode .*drafts:" 5 -e MAIL_WRITABLE=true
expect_doctor "mode .*drafts+send with 2 recipient pattern(s)" 6 \
  -e MAIL_ALLOW_SEND=true -e MAIL_SEND_ALLOW=@example.test,boss@example.org
expect_doctor "every recipient is refused" 6 -e MAIL_ALLOW_SEND=yes
# A typo is refused at start-up, never read as on.
if doctor -e MAIL_WRITABLE=maybe >"$WORK/d.txt" 2>&1; then
  echo "MAIL_WRITABLE=maybe was accepted"; cat "$WORK/d.txt"; exit 1
fi

# tools/list over stdio, without a mail call: list_tools [docker -e args...] -> names, sorted
list_tools() {
  # stdin stays open a moment so the server answers before it sees end of input
  { printf '%s\n' \
      '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"docker-greenmail","version":"0"}}}' \
      '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
      '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
    sleep 3; } \
    | run "$@" "$IMAGE" 2>/dev/null \
    | jq -r 'select(.id == 2) | [.result.tools[].name] | sort | join(",")'
}
expect_tools() {
  local want=$1; shift
  local got
  got="$(list_tools "$@")"
  [ "$got" = "$want" ] || { echo "tools with $*: got '$got', want '$want'"; exit 1; }
  echo "  $* -> $got"
}
READ="list_folders,read_attachment,read_message,search"
expect_tools "$READ"
expect_tools "$READ" -e MAIL_WRITABLE=off -e MAIL_ALLOW_SEND=false
expect_tools "draft,$READ" -e MAIL_WRITABLE=1
expect_tools "draft,$READ,send" -e MAIL_ALLOW_SEND=true -e MAIL_SEND_ALLOW=@example.test

echo "== MCP over stdio"
mkfifo "$WORK/in"
run "$IMAGE" <"$WORK/in" >"$WORK/out.jsonl" 2>"$WORK/err.txt" &
SERVER=$!
exec 3>"$WORK/in"
send() { printf '%s\n' "$1" >&3; }
wait_for_id() {
  for _ in $(seq 1 120); do
    if jq -e --argjson id "$1" 'select(.id == $id)' "$WORK/out.jsonl" >/dev/null 2>&1; then return 0; fi
    sleep 0.5
  done
  echo "no response with id $1"; cat "$WORK/out.jsonl" "$WORK/err.txt"; exit 1
}
send '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"docker-greenmail","version":"0"}}}'
wait_for_id 1
send '{"jsonrpc":"2.0","method":"notifications/initialized"}'
send '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
wait_for_id 2
send '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_folders","arguments":{}}}'
wait_for_id 3
exec 3>&-
wait "$SERVER" || true

echo "-- stdout"
cat "$WORK/out.jsonl"
echo "-- stderr"
cat "$WORK/err.txt"

# stdout carries nothing but JSON-RPC messages.
while IFS= read -r line; do
  printf '%s' "$line" | jq -e '.jsonrpc == "2.0"' >/dev/null \
    || { echo "not JSON-RPC on stdout: $line"; exit 1; }
done <"$WORK/out.jsonl"
jq -e 'select(.id == 1) | .result.protocolVersion == "2025-06-18"' "$WORK/out.jsonl" >/dev/null
jq -e 'select(.id == 2) | (.result.tools | length) == 4' "$WORK/out.jsonl" >/dev/null
jq -e 'select(.id == 3) | .result.isError == false' "$WORK/out.jsonl" >/dev/null \
  || { echo "list_folders failed"; exit 1; }
jq -e 'select(.id == 3) | .result.content | map(.text // "") | join("\n") | test("INBOX")' \
  "$WORK/out.jsonl" >/dev/null || { echo "INBOX not listed"; exit 1; }
if grep -q "$PASS" "$WORK/out.jsonl" "$WORK/err.txt"; then echo "password in the output"; exit 1; fi
echo "== ok: $IMAGE against $GREENMAIL"
