#!/usr/bin/env bash
# Run the real TouchCapture class on Android without installing or launching the app.
# Usage: ANDROID_HOME=/path/to/sdk scripts/check-pending-config.sh DEVICE_SERIAL
set -euo pipefail

serial=${1:?Pass an adb device serial}
sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}
: "${sdk:?Set ANDROID_HOME to the Android SDK}"
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
scratch=$(mktemp -d)
remote=/data/local/tmp/uscreen-pending-check-$$
cleanup() {
    adb -s "$serial" shell rm -rf "$remote" >/dev/null 2>&1 || true
    rm -rf -- "$scratch"
}
trap cleanup EXIT

cd "$root/android"
./gradlew --no-daemon :app:assembleDebug :app:compileDebugUnitTestJavaWithJavac
classes=app/build/intermediates/javac/debugUnitTest/classes
mapfile -d '' checks < <(find "$classes/com/uscreen" -name 'PendingConfigCheck*.class' -print0)
(( ${#checks[@]} )) || { echo 'Regression check classes were not compiled' >&2; exit 1; }
for candidate in "$sdk"/build-tools/*/d8; do
    [[ -x "$candidate" ]] && d8=$candidate
done
: "${d8:?No d8 found in the Android SDK build-tools}"
"$d8" --min-api 27 --lib "$sdk/platforms/android-34/android.jar" \
    --output "$scratch/checks.jar" "${checks[@]}"
adb -s "$serial" shell mkdir -p "$remote"
adb -s "$serial" push app/build/outputs/apk/debug/app-debug.apk "$remote/app.apk" >/dev/null
adb -s "$serial" push "$scratch/checks.jar" "$remote/checks.jar" >/dev/null
adb -s "$serial" shell "CLASSPATH=$remote/app.apk:$remote/checks.jar app_process / com.uscreen.PendingConfigCheck"
