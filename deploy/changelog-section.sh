#!/bin/sh
# 打印 CHANGELOG.md 里某个版本的段落（Release 工作流用它当发布说明）。
#   deploy/changelog-section.sh v0.2.0
set -eu
version=${1#v}
awk -v v="$version" '
    /^## / { if (found) exit; if (index($0, "[" v "]")) { found = 1; next } }
    found { print }
' "$(dirname "$0")/../CHANGELOG.md"
