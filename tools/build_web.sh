#!/bin/bash
# Build the browser bundle into web/dist/ (deployable as static files).
set -e
cd "$(dirname "$0")/.."
cargo build -p begin-web --target wasm32-unknown-unknown --release
rm -rf web/dist
mkdir -p web/dist
cp web/index.html web/begin.js web/dist/
cp -r web/vendor web/dist/vendor
cp target/wasm32-unknown-unknown/release/begin_web.wasm web/dist/
echo "web/dist ready:"
du -sh web/dist
