#!/usr/bin/env bash
# agent-pool 完成判定。非零退出 = 继续干活。
set -uo pipefail
cd "$(dirname "$0")/.."

fail() { echo "❌ $1"; exit 1; }
absent() { if grep -qnE "$1" $2 2>/dev/null; then fail "$3"; fi; }
present() { if ! grep -qE "$1" $2 2>/dev/null; then fail "$3"; fi; }

collected=$(uv run pytest tests/ --collect-only -q 2>/dev/null || true)

echo "== 1. P0: 回测提前建仓必须修掉,且有数值断言测试 =="
uv run pytest tests/test_backtest_engine.py -q || fail "回测测试未全绿"
echo "$collected" | grep -qiE "lookahead|prior_bar|entry_before_start|提前建仓" \
  || fail "缺少提前建仓的数值断言测试"
absent "iloc\[0\] == 1\.0" app/backtest/engine.py \
  "engine.py 仍用窗口首日仓位判断(P0 未修)"

echo "== 2. 指标口径: 242 / 真峰谷回撤 / Sharpe 无风险利率 =="
present "\b242\b" app/backtest/engine.py "年化基数仍非 242"
absent "cummax\(\)\.clip\(lower=" app/backtest/engine.py \
  "max_drawdown 仍从初始资金起算,非真峰谷"
present "risk_free|rf\b" app/backtest/engine.py "Sharpe 未引入无风险利率"

echo "== 3. 排行榜必须用 batch_id =="
# 允许保留 run_at 作为「batch_id 落地前历史行」的兼容降级分支,
# 但主查询必须按 batch_id 过滤。
present "StrategyEval\.batch_id ==" app/backtest/evaluate.py \
  "leaderboard 主查询未按 batch_id 过滤"
present "batch_id" app/backtest/evaluate.py "未引入 batch_id"

echo "== 4. 两处绕过 universe.py 的重复池解析必须收口 =="
if grep -qnE "select\(IndexMember" app/selection/screener.py app/data/fundamentals.py 2>/dev/null; then
  fail "仍有绕过 universe.py 的 IndexMember 直查"
fi

echo "== 5. GET screener 必须批量化(全A 默认后为阻塞项) =="
screen_body=$(awk '/^def screen\(/,/^def [a-z_]+\(/' app/selection/screener.py 2>/dev/null || true)
if echo "$screen_body" | grep -qE "for .* in rows" && echo "$screen_body" | grep -q "load_bars_df"; then
  fail "GET screener 仍逐只 load_bars_df(N+1 未消除)"
fi

echo "== 6. screener total 必须在截断前统计 =="
if grep -nE "items = items\[:limit\]" -A2 app/selection/screener.py 2>/dev/null | grep -q "len(items)"; then
  fail "total 仍在截断后统计"
fi

echo "== 7. count(DailyBar.id) 必须改掉 =="
absent "func\.count\(DailyBar\.id\)" app/selection/screener.py \
  "仍引用 DailyBar.id,会阻塞 migrate 换主键"

echo "== 8. 超卖必须在写入层被拒绝 =="
echo "$collected" | grep -qiE "oversell|over_sell|negative_position|超卖" \
  || fail "缺少超卖拒绝的测试"

echo "== 9. 默认口径必须是全A =="
present "kind\s*==\s*.all.|'all'" app/data/universe.py "universe.py 未实现 kind='all' 解析"

echo "== 10. Gate 2: metrics-before-after.md 必须产出 =="
test -s logs/metrics-before-after.md || fail "缺 logs/metrics-before-after.md(Gate 2)"

echo "== 11. 全量回归(基线 59 passed,不得回退) =="
uv run pytest tests/ -q || fail "全量回归未通过"

echo "✅ all gates passed (agent-pool)"
