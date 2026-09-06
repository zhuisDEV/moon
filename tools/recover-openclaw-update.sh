#!/bin/sh

# Bridge an older Moon updater across OpenClaw's explicit gateway-stop consent.
# Moon still verifies the release, asks for approval, and owns the transaction.
umask 077

moon_recovery_home=${MOON_HOME:-"$HOME/.moon"}
moon_recovery_binary="$moon_recovery_home/bin/moon"
if [ ! -f "$moon_recovery_binary" ] || [ ! -x "$moon_recovery_binary" ]; then
  printf '%s\n' 'The canonical Moon executable is missing or not executable.' >&2
  exit 1
fi

moon_recovery_openclaw=$(command -v openclaw) || exit 1
case "$moon_recovery_openclaw" in
  /*) ;;
  *)
    printf '%s\n' 'OpenClaw must resolve to an absolute executable path.' >&2
    exit 1
    ;;
esac
if [ ! -f "$moon_recovery_openclaw" ] || [ ! -x "$moon_recovery_openclaw" ]; then
  printf '%s\n' 'The resolved OpenClaw executable is not usable.' >&2
  exit 1
fi

moon_recovery_dir=$(mktemp -d "${TMPDIR:-/tmp}/moon-openclaw-update.XXXXXX") || exit 1
moon_recovery_cleanup() {
  moon_recovery_status=$?
  trap - 0
  rm -f "$moon_recovery_dir/openclaw"
  rmdir "$moon_recovery_dir"
  exit "$moon_recovery_status"
}
trap moon_recovery_cleanup 0
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

cat > "$moon_recovery_dir/openclaw" <<'SHIM'
#!/bin/sh
if [ "$#" -eq 3 ] && [ "$1" = gateway ] && [ "$2" = stop ] && [ "$3" = --json ]; then
  exec "$MOON_RECOVERY_OPENCLAW" gateway stop --force --json
fi
exec "$MOON_RECOVERY_OPENCLAW" "$@"
SHIM
if [ "$?" -ne 0 ] || ! chmod 700 "$moon_recovery_dir/openclaw"; then
  exit 1
fi

MOON_RECOVERY_OPENCLAW="$moon_recovery_openclaw" \
  PATH="$moon_recovery_dir:$PATH" \
  "$moon_recovery_binary" --home "$moon_recovery_home" update "$@"
