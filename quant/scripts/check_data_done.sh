#!/usr/bin/env bash
# agent-data 完成判定。非零退出 = 继续干活。
set -uo pipefail
cd "$(dirname "$0")/.."

fail() { echo "❌ $1"; exit 1; }
absent() { if grep -qnE "$1" "$2" 2>/dev/null; then fail "$3"; fi; }
present() { if ! grep -qE "$1" "$2" 2>/dev/null; then fail "$3"; fi; }

echo "== 1. 采集相关测试 =="
uv run pytest tests/test_scheduler.py tests/test_universe.py \
  tests/test_backfill_pool.py -q || fail "采集相关测试未全绿"

echo "== 2. 重锚测试必须覆盖两个场景 =="
cases=$(uv run pytest tests/ -q -k "reanchor or rescale" --collect-only 2>/dev/null || true)
echo "$cases" | grep -qE "mixed|混接|scale" || fail "缺少『新旧尺度混接』重锚测试"
echo "$cases" | grep -qE "no_overlap|missing|stored_none" || fail "缺少『无重叠行』重锚测试"

echo "== 3. backfill_pool 必须走重锚检查 =="
absent "^[[:space:]]*ingest\.backfill\(" scripts/backfill_pool.py \
  "backfill_pool.py 仍直调 ingest.backfill,绕过重锚检查"

echo "== 4. 交易日历必须落地且被 scheduler 使用 =="
present "query_trade_dates" app/data/baostock_client.py "未接 baostock query_trade_dates"
present "trade_calendar|is_trading_day" app/scheduler.py \
  "scheduler 未改用交易日历(仍在用 _is_weekday)"

echo "== 5. import_stock_list 必须 upsert 并维护上市/退市/ST =="
for f in list_date delist_date is_st; do
  present "$f" app/data/ingest.py "import_stock_list 未维护 $f"
done

echo "== 6. login_session 必须可重入 =="
present "_login_depth|refcount|reentran" app/data/baostock_client.py \
  "login_session 未实现可重入(嵌套会提前 logout)"

echo "== 7. universe.py 不属于本 scope,不得改动 =="
if git diff master --name-only 2>/dev/null | grep -q "app/data/universe.py"; then
  fail "越界修改 app/data/universe.py(归 agent-pool)"
fi

echo "== 8. 全量回归(基线 59 passed,不得回退) =="
uv run pytest tests/ -q || fail "全量回归未通过"

echo "✅ all gates passed (agent-data)"
