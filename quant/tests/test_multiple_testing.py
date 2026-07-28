"""多重检验提示单元测试。"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.strategy.multiple_testing import multiplicity_report


def test_multiplicity_report_counts_and_disclaimer():
    rows = [
        {"params": {"$.x": 1}, "metrics": {"annual_return_median": 0.1}},
        {"params": {"$.x": 2}, "metrics": {"annual_return_median": 0.3}},
        {"params": {"$.x": 3}, "metrics": {"annual_return_median": 0.2}},
    ]
    report = multiplicity_report(rows)
    assert report["n_trials"] == 3
    assert report["n_evaluable"] == 3
    assert report["best_metric"] == 0.3
    assert report["best_params"] == {"$.x": 2}
    assert report["bonferroni_alpha"] == pytest_approx_alpha()
    assert "探索" in report["disclaimer"] or "未校正" in report["disclaimer"]


def pytest_approx_alpha():
    return round(0.05 / 3, 6)
