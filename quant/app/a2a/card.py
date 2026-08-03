"""A2A Agent Card。

按 a2a-design.md §6 逐字生成 19 个 skill 与 capabilities/security。
Card 通过 create_agent_card_routes 挂在 /.well-known/agent-card.json。
"""
from __future__ import annotations

import os
from typing import Any

from a2a.types.a2a_pb2 import AgentCard
from google.protobuf import json_format


def _card_dict() -> dict[str, Any]:
    """设计文档 §6 JSON 的 Python 等价结构。"""
    return {
        "name": "quant-research",
        "description": (
            "A-share daily research tools: validate StrategySpec, register experiments/trials, "
            "run simulated backtests, factor validation/preview/market-wide evaluation (IC, layered returns), "
            "screen universe, report data quality and capability gaps, record research findings. "
            "No broker connectivity or order execution. Deterministic server (no LLM). "
            "Invocation requires structured data parts (skill+payload); text-only messages are rejected."
        ),
        "version": "1.0.0",
        "supported_interfaces": [
            {
                "url": os.environ.get("QUANT_A2A_URL", "http://127.0.0.1:8100/a2a"),
                "protocol_binding": "JSONRPC",
                "protocol_version": "0.3",
            }
        ],
        "capabilities": {
            "streaming": True,
            "push_notifications": False,
            "extended_agent_card": False,
        },
        "default_input_modes": ["application/json", "text/plain"],
        "default_output_modes": ["application/json", "text/plain"],
        "security_schemes": {
            "bearer": {
                "httpAuthSecurityScheme": {
                    "description": "Shared HS256 JWT issued by Trace server.",
                    "scheme": "bearer",
                    "bearerFormat": "JWT",
                }
            }
        },
        "security_requirements": [
            {"schemes": {"bearer": []}}
        ],
        "skills": [
            {
                "id": "catalog.get",
                "name": "Get research catalog",
                "description": "Research dictionaries plus the strategy_authoring contract: exact operator shapes and complete valid StrategySpec examples. Call before authoring specs.",
                "tags": ["catalog", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "market.data_quality",
                "name": "Data quality snapshot",
                "description": "Coverage and trust metrics for bars, ST, valuation, fundamentals.",
                "tags": ["data", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "strategy.validate",
                "name": "Validate StrategySpec",
                "description": "Strict validation and capability report. Does not persist strategy.",
                "tags": ["strategy", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "strategy.save_draft",
                "name": "Save strategy draft",
                "description": "Persist StrategySpec as disabled unverified draft. Optional parent_strategy_id for lineage. Returns strategy_id for experiments/backtests.",
                "tags": ["strategy", "write-draft"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "experiment.create",
                "name": "Create experiment",
                "description": "Register frozen-spec experiment with hypothesis and permanent_candidate_id. Requires strategy_id for trial runs. No confirmation.",
                "tags": ["experiment", "write-registry"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "experiment.get",
                "name": "Get experiment",
                "description": "Experiment detail, trials, multiplicity hints, pending evidence promotions.",
                "tags": ["experiment", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "experiment.list",
                "name": "List experiments",
                "description": "List caller's experiments (summary). Avoid duplicate candidate registrations.",
                "tags": ["experiment", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "experiment.trial",
                "name": "Run experiment trial",
                "description": "Apply param_patch on frozen spec, run simulated backtest, append immutable trial. Long-running. Requires confirmed=true.",
                "tags": ["experiment", "write-sim"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "experiment.trial_batch",
                "name": "Run experiment trial batch",
                "description": "Sequential param_patch trials (hard cap). Requires confirmed=true. Counts as multiple high-cost units.",
                "tags": ["experiment", "write-sim"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "factor.validate",
                "name": "Validate factor expression",
                "description": "Validate factor expression. Does not by itself prove market-wide efficacy; use factor.evaluate for IC/layered evidence.",
                "tags": ["factor", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "factor.preview",
                "name": "Preview factor series",
                "description": "Compute factor on 1..N codes (small N). Spot-check only; use factor.evaluate for market-wide IC/layered returns.",
                "tags": ["factor", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "factor.evaluate",
                "name": "Evaluate factor efficacy",
                "description": "Market-wide (or pool-scoped) factor evaluation: IC / RankIC / ICIR with Newey-West t-stats, IC decay across forward horizons, optional industry / market-cap neutralization, layered long-short returns and a multiplicity report. Long-running, high-cost. Requires confirmed=true.",
                "tags": ["factor", "write-sim"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "factor.evaluation_list",
                "name": "List factor evaluations",
                "description": "List own past factor evaluations (summary: IC headline, t-stat, neutralization, horizons). Cursor paginated. Use to compare rounds instead of relying on conversation memory.",
                "tags": ["factor", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "factor.evaluation_get",
                "name": "Get factor evaluation",
                "description": "Fetch one own factor evaluation by evaluation_id, including layered returns, IC decay curve and multiplicity report.",
                "tags": ["factor", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "factor.save_draft",
                "name": "Save factor draft",
                "description": "Persist disabled factor definition owned by the caller. Optional parent_factor_key for lineage. Always enabled=false. No confirmation. Follow with factor.backfill before evaluating by factor_key.",
                "tags": ["factor", "write-draft"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "factor.backfill",
                "name": "Backfill factor history",
                "description": "Compute and persist historical daily values for one's own factor draft into quant_factor_daily, so later factor.evaluate by factor_key has data to read. Own non-system drafts only. Long-running, high-cost. Requires confirmed=true.",
                "tags": ["factor", "write-sim"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "factor.correlation",
                "name": "Factor correlation and orthogonality",
                "description": "Test whether a candidate factor adds information over a benchmark factor set: per-rebalance cross-sectional Pearson/Spearman correlation with stability, correlation between IC series, and residual IC with Newey-West t-stats after cross-sectional regression on the benchmarks. Residual IC insignificant means no increment over existing factors. Long-running, high-cost. Requires confirmed=true.",
                "tags": ["factor", "write-sim"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "factor.correlation_get",
                "name": "Get factor correlation result",
                "description": "Fetch one own factor correlation result by correlation_id, including per-benchmark correlation pairs and the residual IC verdict.",
                "tags": ["factor", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "backtest.run",
                "name": "Run backtest",
                "description": "Simulated T+1 backtest for saved strategy_id (path S). Long-running. Requires confirmed=true. Prefer experiment.trial for multi-round refinement.",
                "tags": ["backtest", "write-sim"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "backtest.get",
                "name": "Get backtest run",
                "description": "Fetch summary for an existing backtest run owned by the caller.",
                "tags": ["backtest", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "backtest.list",
                "name": "List backtest runs",
                "description": "List caller's recent backtest runs. Secondary memory; prefer experiment.get for trial lineage.",
                "tags": ["backtest", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "selection.screen",
                "name": "Structured screener",
                "description": "AND/OR condition screen with per-field coverage warnings.",
                "tags": ["selection", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "system.gap_summary",
                "name": "Capability gap summary",
                "description": "Aggregate missing_capability / failure_kind from A2A audit plus persisted research findings, for the caller or admin global view. Read-only.",
                "tags": ["system", "read"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
            {
                "id": "system.report_finding",
                "name": "Report research finding",
                "description": "Persist structured research findings (missing capability, data gap, hypothesis outcome) from the Orchestrator's Conclude step. Aggregated by system.gap_summary.",
                "tags": ["system", "write-finding"],
                "input_modes": ["application/json"],
                "output_modes": ["application/json"],
            },
        ],
    }


def build_agent_card() -> AgentCard:
    """构造 protobuf AgentCard。"""
    card = AgentCard()
    json_format.ParseDict(_card_dict(), card)
    return card


__all__ = ["build_agent_card"]
