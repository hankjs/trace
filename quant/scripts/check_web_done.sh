#!/usr/bin/env bash
# agent-web 完成判定。非零退出 = 继续干活。
set -uo pipefail
cd "$(dirname "$0")/.."

fail() { echo "❌ $1"; exit 1; }
absent() { if grep -qnE "$1" "$2" 2>/dev/null; then fail "$3"; fi; }
present() { if ! grep -qE "$1" "$2" 2>/dev/null; then fail "$3"; fi; }

echo "== 1. 构建与类型检查必须通过 =="
( cd web && pnpm build ) || fail "pnpm build 失败(含 vue-tsc 类型错误)"

echo "== 2. 静默降级必须删除(会丢 OR 逻辑和基本面条件) =="
if grep -nE "40[45]" web/src/api.ts 2>/dev/null | grep -qiE "fallback|legacy|回退"; then
  fail "api.ts 仍有 screener 静默降级回退"
fi

echo "== 3. universe 字面量必须换成 pool_id =="
absent "'hs300'|'zz500'|'hs300_zz500'" web/src/api.ts "api.ts 仍有硬编码 universe 字面量"
present "pool_id|poolId" web/src/api.ts "api.ts 未改用 pool_id"

echo "== 4. 池组管理页必须存在 =="
ls web/src/views/ | grep -qiE "pool" || fail "缺池组管理页"

echo "== 5. 统一池选择器组件必须存在 =="
ls web/src/components/ | grep -qiE "pool" || fail "缺统一池选择器组件"

echo "== 6. localStorage key 必须 bump =="
present "_v2|_V2" web/src/views/Screener.vue "Screener.vue 的 STORAGE_KEY 未 bump 到 _v2"

echo "== 7. 静态池幸存者偏差必须在 UI 标注(两处) =="
hits=$(grep -rloE "幸存者偏差|survivorship" web/src/ 2>/dev/null | wc -l | tr -d ' ')
test "${hits:-0}" -ge 2 || fail "幸存者偏差标注不足两处(当前 ${hits:-0} 处)"

echo "== 8. dual field name 死代码必须清理 =="
absent "chg_pct" web/src/api.ts "仍有 pct_chg/chg_pct 双字段兼容死代码"

echo "✅ all gates passed (agent-web)"
