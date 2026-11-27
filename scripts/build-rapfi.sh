#!/bin/sh
# Build the same freestyle engine for local development and the game image.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out=${1:-"$root/.rapfi"}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
engine=1387d11312190ec66665d361ff07a7c70221b78a
weights=7eeb19b55219a48138756293f92211a1539bab4d
raw=https://raw.githubusercontent.com/dotb1ue/rapfi-networks/$weights
fetch() {
    curl --fail --location --silent --show-error --retry 3 "$1" -o "$tmp/$2"
    printf '%s  %s\n' "$3" "$tmp/$2" | sha256sum -c -
}
fetch "https://codeload.github.com/dotb1ue/rapfi/tar.gz/$engine" rapfi.tar.gz a12ab91d2bd14cd34ba31ac22cada55a54869286cc88f07ddd00bce7fea4a108
fetch "$raw/classical/model260809.bin" model260809.bin 8d4f518c58f9c7abe017de383a87d2d7506c62936ea5188004b7eb1780b99393
fetch "$raw/mix9svq/mix9svqfreestyle_bsmix.bin.lz4" mix9svqfreestyle_bsmix.bin.lz4 6bc0d1b0ff8e1d857f7f412923cd458f38f1a435087c91ae71fec3676c23ef62
fetch "$raw/LICENSE" WEIGHTS-LICENSE 084bc2aebf95358b73255bd38213fdb27bfd19c6819ae7c1d0679400cf01cea1
tar -xzf "$tmp/rapfi.tar.gz" -C "$tmp"
cmake -S "$tmp/rapfi-$engine/Rapfi" -B "$tmp/build" -DCMAKE_BUILD_TYPE=Release \
    -DNO_COMMAND_MODULES=ON -DUSE_AVX2=OFF -DUSE_AVX512=OFF -DUSE_BMI2=OFF -DUSE_VNNI=OFF -DUSE_NEON_DOTPROD=OFF
cmake --build "$tmp/build" --parallel 4
mkdir -p "$out/source"
cp "$tmp/rapfi.tar.gz" "$tmp/rapfi-$engine/Copying.txt" "$tmp/rapfi-$engine/AUTHORS" "$tmp/WEIGHTS-LICENSE" "$0" "$out/source/"
cp "$tmp/model260809.bin" "$tmp/mix9svqfreestyle_bsmix.bin.lz4" "$out/"
cp "$root/scripts/rapfi.toml" "$out/config.toml"
install -m 755 "$tmp/build/pbrain-rapfi" "$out/pbrain-rapfi"
