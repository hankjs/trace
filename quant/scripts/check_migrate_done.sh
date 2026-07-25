#!/usr/bin/env bash
# agent-migrate 完成判定。非零退出 = 继续干活。
set -uo pipefail
cd "$(dirname "$0")/.."

fail() { echo "❌ $1"; exit 1; }
absent() { if grep -qnE "$1" "$2" 2>/dev/null; then fail "$3"; fi; }
present() { if ! grep -qE "$1" "$2" 2>/dev/null; then fail "$3"; fi; }

echo "== 1. Alembic 必须落地 =="
test -f alembic.ini || fail "缺 alembic.ini"
test -d alembic/versions || fail "缺 alembic/versions/"
present "alembic" pyproject.toml "alembic 未进依赖"

echo "== 2. 全新库 upgrade head 必须与 models 一致 =="
uv run python scripts/verify_migration_parity.py || fail "全新库迁移失败或与 models 不一致"

echo "== 3. P0: user_id 必须是 VARCHAR(36),且 auth.py 不再 int() =="
absent "int\(claims\.get\(.sub.\)\)|int\(.*\[.sub.\]\)" app/auth.py \
  "auth.py 仍把 sub 转 int(P0 未修)"
present "String\(36\)" app/models.py "models.py 中 user_id 仍非 VARCHAR(36)"
absent "user_id.*BigInteger" app/models.py "仍有 user_id 是 BigInteger"

echo "== 4. 认领脚本必须收 UUID 字符串 =="
absent "user-id.*type=int|type=int.*user.id" scripts/claim_legacy_user_data.py \
  "认领脚本仍只接受整数 user_id"

echo "== 5. 价格/金额列必须是 DECIMAL =="
present "DECIMAL|Numeric" app/models.py "价格列未改 DECIMAL"

echo "== 6. 池表 / 日历表 / 上市退市列必须存在 =="
for t in quant_pool quant_pool_member quant_trade_calendar; do
  present "$t" app/models.py "缺表 $t"
done
for c in list_date delist_date is_st min_list_days batch_id costs; do
  present "$c" app/models.py "缺列 $c"
done

echo "== 7. 调度器跨进程互斥 =="
if ! grep -qE "advisory_lock|GET_LOCK|SCHEDULER_ENABLED|scheduler_enabled" \
     app/scheduler.py app/config.py app/main.py 2>/dev/null; then
  fail "未实现调度器跨进程互斥"
fi

echo "== 8. 启动流程不得再 create_all + 手写 ALTER =="
absent "create_all|upgrade_research_schema" app/main.py \
  "main.py 仍在启动时建表/改表,应交给 Alembic"

echo "== 9. 禁止硬编码生产连接串 =="
if grep -rnE "mysql://|mysql\+pymysql://" alembic/ scripts/verify_migration_parity.py 2>/dev/null \
   | grep -qv "sqlite"; then
  fail "迁移脚本内出现硬编码生产连接串"
fi

echo "== 10. 全量回归(基线 59 passed,不得回退) =="
uv run pytest tests/ -q || fail "全量回归未通过"

echo "✅ all gates passed (agent-migrate)"
