#!/usr/bin/env bash
# Dev launcher for rift. Usage:
#   ./scripts/rift.sh server [start|build|run] [-- extra args]
#   ./scripts/rift.sh client [start|build|run] [-- extra args]
#   ./scripts/rift.sh both                # server in bg, client in fg
#   ./scripts/rift.sh build               # cargo build --workspace
#
# Defaults to "start" (= run debug binary, building if needed).
# Examples:
#   ./scripts/rift.sh server
#   ./scripts/rift.sh client
#   ./scripts/rift.sh client -- --connect 127.0.0.1:34000
set -euo pipefail

cd "$(dirname "$0")/.."

SERVER_BIND="${RIFT_SERVER_BIND:-127.0.0.1:34000}"
SERVER_LOG="${RIFT_SERVER_LOG:-info}"
CLIENT_LOG="${RIFT_CLIENT_LOG:-info}"
CLIENT_CONNECT="${RIFT_CONNECT:-127.0.0.1:34000}"

cmd="${1:-help}"; shift || true
sub="${1:-start}"
case "$sub" in start|build|run) shift || true ;; *) sub="start" ;; esac

build_server() { cargo build -p rift-server; }
build_client() { cargo build -p rift-client; }

run_server() {
  RUST_LOG="$SERVER_LOG" ./target/debug/rift-server.exe --bind "$SERVER_BIND" "$@"
}
run_client() {
  RUST_LOG="$CLIENT_LOG" ./target/debug/rift.exe --connect "$CLIENT_CONNECT" "$@"
}

case "$cmd" in
  server)
    [[ "$sub" == build ]] && { build_server; exit 0; }
    [[ "$sub" == start ]] && build_server
    run_server "$@"
    ;;
  client)
    [[ "$sub" == build ]] && { build_client; exit 0; }
    [[ "$sub" == start ]] && build_client
    run_client "$@"
    ;;
  both)
    cargo build --workspace
    RUST_LOG="$SERVER_LOG" ./target/debug/rift-server.exe --bind "$SERVER_BIND" &
    SERVER_PID=$!
    trap 'kill $SERVER_PID 2>/dev/null || true' EXIT
    sleep 0.5
    run_client "$@"
    ;;
  build) cargo build --workspace ;;
  help|*)
    cat <<EOF
rift dev launcher

  ./scripts/rift.sh server [start|build|run] [-- args]
  ./scripts/rift.sh client [start|build|run] [-- args]
  ./scripts/rift.sh both
  ./scripts/rift.sh build

Env: RIFT_SERVER_BIND, RIFT_CONNECT, RIFT_SERVER_LOG, RIFT_CLIENT_LOG
EOF
    ;;
esac
