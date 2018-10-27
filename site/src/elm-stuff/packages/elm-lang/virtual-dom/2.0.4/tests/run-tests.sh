#!/bin/sh

set -e

cd "$(dirname "$0")"

mkdir -p build/virtual-dom/Native
cp ../src/VirtualDom.elm build/virtual-dom/
$(npm bin)/browserify ../src/wrapper.js -o build/VirtualDom.browser.js

set +e
diff -u ../src/Native/VirtualDom.js build/VirtualDom.browser.js
if [ $? != 0 ]; then
	echo "ERROR:"
	echo "src/Native/VirtualDom.js has local modifications or is out of date. Please run rebuild.sh"
	exit 1
fi
set -e

$(npm bin)/browserify --no-browser-field ../src/wrapper.js -o build/virtual-dom/Native/VirtualDom.js

elm-make --yes --output build/test.js TestMain.elm
echo "Elm.worker(Elm.Main);" >> build/test.js
node build/test.js