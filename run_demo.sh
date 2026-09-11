#!/bin/bash

set -euo pipefail

# Everything needed to look at Atria, from one terminal.
#
# Starts the server, a shell, and a few drawing clients, then waits. Ctrl-C stops all of them:
# the point is to be able to try something and stop it without hunting for stray processes.
#
#   ./run_demo.sh              listens for a viewer on 127.0.0.1:5900
#   ATRIA_VNC=:5901 ./run_demo.sh
#   ATRIA_CLIENTS=0 ./run_demo.sh   server and shell only, nothing drawn
#   ELYSIUM_MODIFIER=alt ./run_demo.sh

VNC_ADDRESS="${ATRIA_VNC:-0.0.0.0:5901}"
SOCKET="${ATRIA_SOCKET:-/tmp/atria-demo.sock}"
SHELL_SOCKET="${ATRIA_SHELL_SOCKET:-/tmp/atria-demo-shell.sock}"
CLIENTS="${ATRIA_CLIENTS:-3}"
# Which modifier the shell's chords use. A desktop the viewer runs on usually keeps Super for
# itself, so a remote session generally wants ELYSIUM_MODIFIER=alt.
MODIFIER="${ELYSIUM_MODIFIER:-super}"

cargo build -p atriad --bin atriad --example draw --example elysium0

rm -f "${SOCKET}" "${SHELL_SOCKET}"

# Every process started here is killed on the way out, however this exits. A demo that leaves a
# compositor holding a port is a demo you can only run once.
started=()
cleanup() {
    for pid in "${started[@]:-}"; do
        kill "${pid}" 2>/dev/null || true
    done
    rm -f "${SOCKET}" "${SHELL_SOCKET}"
}
trap cleanup EXIT INT TERM

./target/debug/atriad "${SOCKET}" --vnc "${VNC_ADDRESS}" --shell "${SHELL_SOCKET}" &
started+=($!)

# Wait for the socket rather than sleeping a guessed amount: a slow machine is not a failure.
for _ in $(seq 1 100); do
    [ -S "${SOCKET}" ] && break
    sleep 0.05
done

./target/debug/examples/elysium0 "${SHELL_SOCKET}" "${MODIFIER}" &
started+=($!)
sleep 0.3

# Distinct colours and sizes, so which window is which is obvious on screen.
palette=("320 220 2f9ed8 ledger" "320 220 e08a2b map" "260 180 6667ab notes")
for index in $(seq 0 $((CLIENTS - 1))); do
    [ "${index}" -ge "${#palette[@]}" ] && break
    # shellcheck disable=SC2086
    ./target/debug/examples/draw "${SOCKET}" ${palette[${index}]} &
    started+=($!)
    sleep 0.3
done

echo
echo "atria: point a VNC viewer at ${VNC_ADDRESS}"
echo "atria: clicking a window raises and focuses it"
echo "atria: ${MODIFIER}+tab cycles windows, ${MODIFIER}+q closes the focused one"
echo "atria: ctrl-c stops everything"
echo

wait
