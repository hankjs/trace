"""证据推进质量闸门与待办:试验不自动改 evidence_status。"""
from __future__ import annotations

import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.experiment.promotion import (
    MIN_TRADE_COUNT,
    evaluate_promotion_quality,
)
from app.strategy.evidence import with_status
from app.strategy.presets import get_preset_spec
from app.strategy.spec import strategy_spec_hash


def _strategy(status: str = "design_complete"):
    spec = with_status(get_preset_spec("breakout"), status)
    return SimpleNamespace(
        id=1,
        spec=spec.model_dump(mode="json"),
        spec_hash=strategy_spec_hash(spec),
    )


def _result(*, trades=10, run_id=99, verdict=None, oos=False, dq_ok=None):
    spec = with_status(get_preset_spec("breakout"), "design_complete")
    validation = {}
    if verdict is not None:
        validation["rejection"] = {"verdict": verdict}
    if oos:
        validation["oos"] = {"available": True}
    payload = {
        "run_id": run_id,
        "strategy_spec_hash": strategy_spec_hash(spec),
        "metrics": {
            "trade_count": trades,
            "round_trips": max(1, trades // 2),
            "sharpe": 0.5,
            "annual_return": 0.1,
        },
        "validation": validation,
    }
    if dq_ok is not None:
        payload["data_quality"] = {"ok": dq_ok}
    return payload


def test_quality_blocks_unverified_strategy():
    ev = evaluate_promotion_quality(
        strategy=_strategy("unverified"),
        trial_outcome="ok",
        result=_result(),
        param_patch={},
    )
    assert not ev["eligible"]
    assert any(c["id"] == "EVIDENCE_BASE" and not c["ok"] for c in ev["checks"])


def test_quality_blocks_no_trades_and_thin_sample():
    s = _strategy()
    no_trades = evaluate_promotion_quality(
        strategy=s,
        trial_outcome="no_trades",
        result=_result(trades=0),
        param_patch={},
    )
    assert not no_trades["eligible"]

    thin = evaluate_promotion_quality(
        strategy=s,
        trial_outcome="ok",
        result=_result(trades=MIN_TRADE_COUNT - 1),
        param_patch={},
    )
    # may fail SAMPLE_SIZE if round_trips also low
    thin_result = _result(trades=0)
    thin_result["metrics"]["trade_count"] = 1
    thin_result["metrics"]["round_trips"] = 0
    thin2 = evaluate_promotion_quality(
        strategy=s, trial_outcome="ok", result=thin_result, param_patch={},
    )
    assert not thin2["eligible"]
    assert any(c["id"] == "SAMPLE_SIZE" and not c["ok"] for c in thin2["checks"])


def test_quality_blocks_param_patch_variants():
    ev = evaluate_promotion_quality(
        strategy=_strategy(),
        trial_outcome="ok",
        result=_result(),
        param_patch={"$.native_exit.condition.right.window": 10},
    )
    assert not ev["eligible"]
    assert any(c["id"] == "IDENTITY" and not c["ok"] for c in ev["checks"])


def test_quality_passes_baseline_ok_to_backtested():
    ev = evaluate_promotion_quality(
        strategy=_strategy("design_complete"),
        trial_outcome="ok",
        result=_result(trades=8),
        param_patch={},
    )
    assert ev["eligible"]
    assert ev["suggested_target"] == "backtested"


def test_quality_suggests_oos_when_passed_and_available():
    ev = evaluate_promotion_quality(
        strategy=_strategy("backtested"),
        trial_outcome="ok",
        result=_result(trades=8, verdict="passed", oos=True),
        param_patch={},
    )
    assert ev["eligible"]
    assert ev["suggested_target"] == "oos_passed"


def test_quality_allows_reject_path_even_with_few_trades():
    ev = evaluate_promotion_quality(
        strategy=_strategy("design_complete"),
        trial_outcome="rejected",
        result=_result(trades=0, verdict="rejected"),
        param_patch={},
    )
    assert ev["eligible"]
    assert ev["suggested_target"] == "rejected"


def test_quality_blocks_bad_data_quality():
    ev = evaluate_promotion_quality(
        strategy=_strategy(),
        trial_outcome="ok",
        result=_result(trades=10, dq_ok=False),
        param_patch={},
    )
    assert not ev["eligible"]
    assert any(c["id"] == "DATA_QUALITY" and not c["ok"] for c in ev["checks"])
