"""A2A skill 注册表。"""
from __future__ import annotations

from typing import Any

from ._common import A2AContext, SkillHandler
from . import (
    backtest_get,
    backtest_list,
    backtest_run,
    catalog,
    data_quality,
    experiment_create,
    experiment_get,
    experiment_list,
    experiment_trial,
    experiment_trial_batch,
    factor_evaluate,
    factor_evaluation_get,
    factor_evaluation_list,
    factor_preview,
    factor_save_draft,
    factor_validate,
    gap_summary,
    report_finding,
    screen,
    strategy_save_draft,
    strategy_validate,
)

SKILLS: dict[str, SkillHandler] = {
    "catalog.get": catalog.handle,
    "market.data_quality": data_quality.handle,
    "strategy.validate": strategy_validate.handle,
    "strategy.save_draft": strategy_save_draft.handle,
    "experiment.create": experiment_create.handle,
    "experiment.get": experiment_get.handle,
    "experiment.list": experiment_list.handle,
    "experiment.trial": experiment_trial.handle,
    "experiment.trial_batch": experiment_trial_batch.handle,
    "factor.validate": factor_validate.handle,
    "factor.preview": factor_preview.handle,
    "factor.evaluate": factor_evaluate.handle,
    "factor.evaluation_list": factor_evaluation_list.handle,
    "factor.evaluation_get": factor_evaluation_get.handle,
    "factor.save_draft": factor_save_draft.handle,
    "backtest.run": backtest_run.handle,
    "backtest.get": backtest_get.handle,
    "backtest.list": backtest_list.handle,
    "selection.screen": screen.handle,
    "system.gap_summary": gap_summary.handle,
    "system.report_finding": report_finding.handle,
}

SKILL_IDS = frozenset(SKILLS)


__all__ = ["SKILLS", "SKILL_IDS", "A2AContext", "SkillHandler"]
