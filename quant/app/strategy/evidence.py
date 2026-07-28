"""evidence_status 状态机:规格研究证据的服务端权威迁移。

状态写在 ``spec.metadata.evidence_status`` 里(规格的一部分,参与 spec_hash),
因此带来一个循环:回测按旧状态规格的哈希落库,状态推进后策略哈希随之改变,
旧回测的哈希不再等于当前 spec_hash。解决办法是**身份哈希**——把
evidence_status 归一后再算哈希,同一规则内容的不同状态共享同一身份;
所有"这条回测/评估是否对应当前规格"的判断都按身份(等价实现:
``candidate_spec_hashes`` 枚举五种状态下的完整哈希做成员判断)。

迁移规则(只能前进,rejected 为终态):

- unverified --(手动标记,须通过 design_complete 硬清单)--> design_complete
- design_complete --(有同身份规格的完成回测)--> backtested
- backtested --(locked_oos 回测且否决条件全过)--> oos_passed
- 任意非终态 --(回测命中否决条件)--> rejected
- rejected --(人工复位,同样须通过硬清单)--> design_complete
- 规格编辑导致身份变化且旧状态高于 design_complete(含 rejected)时,保存即
  回落到 design_complete:旧回测证据与旧否决结论都是针对旧规格的。

硬清单只约束进入 design_complete 的路径;普通 validate/save 仍允许草稿
(例如 locked_oos=false),避免把探索阶段挡在门外。
"""
from __future__ import annotations

from typing import Any

from sqlalchemy.orm import Session

from .runtime import strategy_spec_for
from .spec import (
    CapabilityStatus,
    StrategySpec,
    parse_strategy_spec,
    resolve_capabilities,
    strategy_spec_hash,
)

EVIDENCE_STATUSES = (
    "unverified", "design_complete", "backtested", "oos_passed", "rejected",
)
STATUS_RANK = {
    "unverified": 0, "design_complete": 1, "backtested": 2, "oos_passed": 3,
}
MANUAL_ACTIONS = ("mark_design_complete", "reset_rejected")

# 与 app/backtest/validation.py 的内置基线 / 遗留否决字符串保持同步
KNOWN_BASELINE_IDS = frozenset({"buy_and_hold", "equal_weight"})
KNOWN_LEGACY_REJECTION = frozenset({
    "no_net_oos_increment", "unstable_parameters", "capacity_failure",
})

# 整句假说黑名单(大小写不敏感、去空白后精确匹配)
_HYPOTHESIS_PLACEHOLDERS = frozenset({
    "todo", "tbd", "placeholder", "n/a", "na", "none",
    "测试", "占位", "待补充", "假说", "hypothesis",
})
_HYP_MIN_LEN = 20
_HYP_MAX_LEN = 1000


class DesignCompleteChecklistError(ValueError):
    """硬清单未通过:携带字段级 checks,API 映射为 4xx 结构化 body。"""

    def __init__(self, checks: list[dict[str, Any]]):
        self.checks = checks
        failed = sum(1 for item in checks if not item.get("ok"))
        super().__init__(
            f"design_complete_checklist_failed: {failed} 项未通过",
        )


def _check(
    check_id: str, ok: bool, *, code: str | None, message: str,
) -> dict[str, Any]:
    return {
        "id": check_id,
        "ok": ok,
        "code": None if ok else code,
        "message": message,
    }


def design_complete_checks(
    spec: StrategySpec | dict[str, Any],
    *,
    available_fields: set[str] | frozenset[str] | None = None,
) -> list[dict[str, Any]]:
    """机器可判定的验证设计硬清单(全部 ok 才能进入 design_complete)。

    不修改规格;普通 validate 不调用本函数。
    """
    parsed = spec if isinstance(spec, StrategySpec) else parse_strategy_spec(spec)
    hyp = (parsed.metadata.hypothesis or "").strip()
    hyp_lower = hyp.casefold()
    checks: list[dict[str, Any]] = []

    hyp_len_ok = _HYP_MIN_LEN <= len(hyp) <= _HYP_MAX_LEN
    checks.append(_check(
        "HYP_LEN", hyp_len_ok,
        code="hypothesis_too_short",
        message=(
            f"假说去空白后长度 {len(hyp)},须在 {_HYP_MIN_LEN}–{_HYP_MAX_LEN} 字"
            if not hyp_len_ok else f"假说长度 {len(hyp)} 字,符合要求"
        ),
    ))

    hyp_placeholder = hyp_lower in _HYPOTHESIS_PLACEHOLDERS
    checks.append(_check(
        "HYP_PLACEHOLDER", not hyp_placeholder,
        code="hypothesis_placeholder",
        message=(
            "假说不能是占位词(todo/测试/TBD/占位等)"
            if hyp_placeholder else "假说不是已知占位词"
        ),
    ))

    baselines = list(parsed.validation.baseline_ids)
    unknown = [b for b in baselines if b not in KNOWN_BASELINE_IDS]
    checks.append(_check(
        "BASELINE_KNOWN", not unknown and len(baselines) >= 1,
        code="baseline_unknown",
        message=(
            f"未知基线: {', '.join(unknown)}"
            if unknown else
            ("至少需要 1 个已知基线" if not baselines else "基线均在已知集合内")
        ),
    ))
    checks.append(_check(
        "BASELINE_MIN", len(baselines) >= 1,
        code="baseline_missing",
        message="至少需要 1 个基线" if not baselines else f"已声明 {len(baselines)} 个基线",
    ))

    criteria = [str(c).strip() for c in parsed.validation.rejection_criteria]
    rules = list(parsed.validation.rejection_rules or [])
    empty_criteria = any(not c for c in criteria)
    non_empty = [c for c in criteria if c]
    has_legacy = any(c in KNOWN_LEGACY_REJECTION for c in non_empty)
    # 去空白后每项非空,且至少有一条已知遗留条件或结构化规则
    reject_nonempty = (
        len(criteria) >= 1
        and not empty_criteria
        and (has_legacy or len(rules) >= 1)
    )
    checks.append(_check(
        "REJECT_NONEMPTY", reject_nonempty,
        code="rejection_missing",
        message=(
            "否决条件去空白后存在空项,或缺少已知否决/结构化规则"
            if not reject_nonempty else "否决条件非空且可用"
        ),
    ))

    unknown_criteria = [c for c in non_empty if c not in KNOWN_LEGACY_REJECTION]
    # 禁止随意字符串:每个字符串 criteria 必须属于已知遗留集合
    # 若只写 structured rules,仍须至少有一条合法字符串(Spec 层 min_length=1),
    # 该字符串也必须是已知的(或允许用合法 legacy 占位 + rules)
    reject_known = not unknown_criteria and (has_legacy or len(rules) >= 1)
    if unknown_criteria:
        reject_known = False
    checks.append(_check(
        "REJECT_KNOWN", reject_known,
        code="rejection_unknown",
        message=(
            f"未知否决条件: {', '.join(unknown_criteria)}"
            if unknown_criteria else
            (
                "否决条件均在已知集合"
                if reject_known else "缺少已知否决条件或结构化规则"
            )
        ),
    ))

    locked = bool(parsed.validation.locked_oos)
    checks.append(_check(
        "LOCKED_OOS", locked,
        code="oos_not_locked",
        message="已锁定样本外" if locked else "须将 validation.locked_oos 设为 true",
    ))

    if parsed.kind == "single":
        native_ok = parsed.native_exit is not None
        checks.append(_check(
            "NATIVE_EXIT", native_ok,
            code="native_exit_missing",
            message=(
                "单标的策略必须包含 native_exit"
                if not native_ok else "已声明原生离场"
            ),
        ))
    else:
        # portfolio: positioning 完整性由 StrategySpec 层保证;此处再确认有 score
        pos = parsed.positioning
        pos_ok = (
            getattr(pos, "type", None) is not None
            or (isinstance(pos, dict) and pos.get("type"))
            or hasattr(pos, "selection")
        )
        checks.append(_check(
            "NATIVE_EXIT", pos_ok,
            code="native_exit_missing",
            message=(
                "组合策略 positioning 不完整"
                if not pos_ok else "组合 positioning 完整"
            ),
        ))

    report = resolve_capabilities(
        parsed.model_dump(mode="json"),
        available_fields=available_fields,
    )
    cap_ok = report.status == CapabilityStatus.SUPPORTED
    checks.append(_check(
        "CAPABILITY", cap_ok,
        code="capability_not_supported",
        message=(
            f"能力状态为 {report.status.value},须为 supported"
            if not cap_ok else "能力解析为 supported"
        ),
    ))

    return checks


def assert_design_complete_ready(
    spec: StrategySpec | dict[str, Any],
    *,
    available_fields: set[str] | frozenset[str] | None = None,
) -> list[dict[str, Any]]:
    """清单全绿返回 checks;否则抛 DesignCompleteChecklistError。"""
    checks = design_complete_checks(spec, available_fields=available_fields)
    if any(not item["ok"] for item in checks):
        raise DesignCompleteChecklistError(checks)
    return checks


def with_status(spec: StrategySpec, status: str) -> StrategySpec:
    """返回替换 metadata.evidence_status 后的规格副本。"""
    return spec.model_copy(update={
        "metadata": spec.metadata.model_copy(update={"evidence_status": status}),
    })


def spec_identity_hash(spec: StrategySpec | dict[str, Any]) -> str:
    """身份哈希:evidence_status 归一为 unverified 后的规范化哈希。"""
    parsed = spec if isinstance(spec, StrategySpec) else parse_strategy_spec(spec)
    return strategy_spec_hash(with_status(parsed, "unverified"))


def candidate_spec_hashes(spec: StrategySpec | dict[str, Any]) -> set[str]:
    """同一规则内容在五种证据状态下的全部完整哈希(身份匹配用)。"""
    parsed = spec if isinstance(spec, StrategySpec) else parse_strategy_spec(spec)
    return {
        strategy_spec_hash(with_status(parsed, status))
        for status in EVIDENCE_STATUSES
    }


def manual_actions_for(status: str | None) -> list[str]:
    """当前状态允许的手动操作(自动推进的状态不允许手改)。"""
    if status == "unverified":
        return ["mark_design_complete"]
    if status == "rejected":
        return ["reset_rejected"]
    return []


def _write_status(db: Session, strategy: Any, spec: StrategySpec,
                  status: str) -> None:
    updated = with_status(spec, status)
    strategy.spec = updated.model_dump(mode="json")
    strategy.spec_hash = strategy_spec_hash(updated)
    db.flush()


def resolve_status_on_edit(old_raw: Any, new_spec: StrategySpec) -> str:
    """规格编辑保存后的状态(编辑接口调用,客户端传入的状态值一律忽略)。

    身份未变(只改了非状态字段之外无实质变化)时保持旧状态;身份变化且旧状态
    高于 design_complete(含 rejected)时回落到 design_complete——旧回测与旧
    否决结论都针对旧规格,不能带进新规格。旧规格不可解析时保守回落 unverified。
    """
    try:
        old_spec = parse_strategy_spec(old_raw)
        old_status = old_spec.metadata.evidence_status
    except Exception:  # noqa: BLE001 - 存量坏数据不应挡住保存
        return "unverified"
    if spec_identity_hash(old_spec) == spec_identity_hash(new_spec):
        return old_status
    advanced = old_status == "rejected" or (
        STATUS_RANK[old_status] > STATUS_RANK["design_complete"]
    )
    return "design_complete" if advanced else old_status


def apply_manual_action(
    db: Session,
    strategy: Any,
    action: str,
    *,
    available_fields: set[str] | frozenset[str] | None = None,
) -> dict[str, Any]:
    """手动迁移:标记设计完成 / 否决复位。

    进入 design_complete 时跑硬清单;失败抛 DesignCompleteChecklistError。
    其它非法迁移抛 ValueError。
    """
    spec = strategy_spec_for(strategy)
    current = spec.metadata.evidence_status
    if action == "mark_design_complete":
        if current != "unverified":
            raise ValueError("只有未验证状态可以手动标记为设计完成")
        target = "design_complete"
    elif action == "reset_rejected":
        if current != "rejected":
            raise ValueError("只有已否决状态可以复位")
        target = "design_complete"
    else:
        raise ValueError(f"未知证据状态操作: {action}")

    checks = assert_design_complete_ready(
        spec, available_fields=available_fields,
    )
    _write_status(db, strategy, spec, target)
    return {"from": current, "to": target, "checks": checks}


def advance_after_backtest(
    db: Session, strategy: Any, result: dict[str, Any],
) -> dict[str, str] | None:
    """落库回测完成后按身份哈希自动推进状态;未推进返回 None。

    闸门(开发阶段收紧,避免幽灵升级与未设计即宣称样本外通过):
    - 必须有 ``run_id``(已持久化的 BacktestRun);save=False 的评估不推进
    - 起点必须是 ``design_complete`` 及之后;``unverified`` 只记 run、不推进
    - 命中否决 -> rejected(终态);locked_oos 已评估且否决全过 -> oos_passed;
      其余完成的回测 -> backtested
    - 只前进不后退;rejected 不自动迁移(只能人工复位)
    - 不是按当前规格身份跑的(临时参数、旧规格)不推进

    ``oos_passed`` 仅表示「通过规格声明的否决条件」,不是科学证实可交易。
    """
    try:
        spec = strategy_spec_for(strategy)
    except Exception:  # noqa: BLE001 - 规格不可解析时没有可推进的状态
        return None
    current = spec.metadata.evidence_status
    if current == "rejected":
        return None
    # 未完成验证设计的规格:允许跑回测,但不自动升级证据状态
    if current == "unverified":
        return None
    # 必须有落库 run,禁止周度评估等 save=False 路径推进状态
    if not result.get("run_id"):
        return None
    run_hash = result.get("strategy_spec_hash")
    if not run_hash or run_hash not in candidate_spec_hashes(spec):
        return None
    validation = result.get("validation") or {}
    rejection = validation.get("rejection") or {}
    oos = validation.get("oos") or {}
    if rejection.get("verdict") == "rejected":
        target = "rejected"
    elif rejection.get("verdict") == "passed" and oos.get("available"):
        target = "oos_passed"
    else:
        target = "backtested"
    if target != "rejected" and STATUS_RANK[target] <= STATUS_RANK[current]:
        return None
    _write_status(db, strategy, spec, target)
    return {"from": current, "to": target}


__all__ = [
    "EVIDENCE_STATUSES", "MANUAL_ACTIONS", "STATUS_RANK",
    "DesignCompleteChecklistError",
    "advance_after_backtest", "apply_manual_action",
    "assert_design_complete_ready", "candidate_spec_hashes",
    "design_complete_checks", "manual_actions_for",
    "resolve_status_on_edit", "spec_identity_hash", "with_status",
]
