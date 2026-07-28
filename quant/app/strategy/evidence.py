"""evidence_status 状态机:规格研究证据的服务端权威迁移。

状态写在 ``spec.metadata.evidence_status`` 里(规格的一部分,参与 spec_hash),
因此带来一个循环:回测按旧状态规格的哈希落库,状态推进后策略哈希随之改变,
旧回测的哈希不再等于当前 spec_hash。解决办法是**身份哈希**——把
evidence_status 归一后再算哈希,同一规则内容的不同状态共享同一身份;
所有"这条回测/评估是否对应当前规格"的判断都按身份(等价实现:
``candidate_spec_hashes`` 枚举五种状态下的完整哈希做成员判断)。

迁移规则(只能前进,rejected 为终态):

- unverified --(手动标记,规格校验通过且 hypothesis 非空)--> design_complete
- design_complete --(有同身份规格的完成回测)--> backtested
- backtested --(locked_oos 回测且否决条件全过)--> oos_passed
- 任意非终态 --(回测命中否决条件)--> rejected
- rejected --(人工复位)--> design_complete
- 规格编辑导致身份变化且旧状态高于 design_complete(含 rejected)时,保存即
  回落到 design_complete:旧回测证据与旧否决结论都是针对旧规格的。
"""
from __future__ import annotations

from typing import Any

from sqlalchemy.orm import Session

from .runtime import strategy_spec_for
from .spec import StrategySpec, parse_strategy_spec, strategy_spec_hash

EVIDENCE_STATUSES = (
    "unverified", "design_complete", "backtested", "oos_passed", "rejected",
)
STATUS_RANK = {
    "unverified": 0, "design_complete": 1, "backtested": 2, "oos_passed": 3,
}
MANUAL_ACTIONS = ("mark_design_complete", "reset_rejected")


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


def apply_manual_action(db: Session, strategy: Any, action: str) -> dict[str, str]:
    """手动迁移:标记设计完成 / 否决复位。非法迁移抛 ValueError。"""
    spec = strategy_spec_for(strategy)
    current = spec.metadata.evidence_status
    if action == "mark_design_complete":
        # 规格能解析即代表校验通过;hypothesis 非空由 MetadataSpec 硬约束保证
        if current != "unverified":
            raise ValueError("只有未验证状态可以手动标记为设计完成")
        target = "design_complete"
    elif action == "reset_rejected":
        if current != "rejected":
            raise ValueError("只有已否决状态可以复位")
        target = "design_complete"
    else:
        raise ValueError(f"未知证据状态操作: {action}")
    _write_status(db, strategy, spec, target)
    return {"from": current, "to": target}


def advance_after_backtest(
    db: Session, strategy: Any, result: dict[str, Any],
) -> dict[str, str] | None:
    """回测/评估完成后按身份哈希自动推进状态;未推进返回 None。

    推进目标:命中否决 -> rejected(终态);locked_oos 已评估且否决全过
    -> oos_passed;其余完成的回测 -> backtested。只前进不后退,rejected
    不自动迁移(只能人工复位)。不是按当前规格身份跑的(临时参数、旧规格)
    不推进。
    """
    try:
        spec = strategy_spec_for(strategy)
    except Exception:  # noqa: BLE001 - 规格不可解析时没有可推进的状态
        return None
    current = spec.metadata.evidence_status
    if current == "rejected":
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
    "advance_after_backtest", "apply_manual_action", "candidate_spec_hashes",
    "manual_actions_for", "resolve_status_on_edit", "spec_identity_hash",
    "with_status",
]
