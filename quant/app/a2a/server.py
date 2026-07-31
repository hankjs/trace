"""A2A JSON-RPC 入口与 QuantAgentExecutor。"""
from __future__ import annotations

import asyncio
import functools
import logging
import threading
import uuid
from datetime import datetime
from typing import Any

from a2a.server.agent_execution import AgentExecutor, RequestContext, SimpleRequestContextBuilder
from a2a.server.events.event_queue import EventQueue
from a2a.server.request_handlers import DefaultRequestHandlerV2
from a2a.server.routes.common import DefaultServerCallContextBuilder
from a2a.server.routes.fastapi_routes import add_a2a_routes_to_fastapi
from a2a.server.routes.jsonrpc_routes import create_jsonrpc_routes
from a2a.types.a2a_pb2 import Task, TaskState, TaskStatus
from google.protobuf import json_format

from a2a.helpers.proto_helpers import (
    get_data_parts,
    get_text_parts,
    new_data_artifact_update_event,
    new_task,
    new_text_status_update_event,
)

from ..config import settings
from ..db import SessionLocal
from ..strategy.spec import StrategyCapabilityError
from .auth import QuantA2AContextBuilder, can_admin, can_client, user_id_from_claims
from .card import build_agent_card
from .skills import SKILL_IDS, SKILLS, A2AContext
from .tasks import (
    HIGH_COST_SKILLS,
    find_idempotent_task,
    quota_tracker,
    rate_limiter,
    record_audit,
    short_task_store,
)

logger = logging.getLogger(__name__)

_HIGH_COST_BATCH_SKILL = "experiment.trial_batch"


def _normalize_numbers(value: Any) -> Any:
    """把 proto Struct 中所有整数值的 float 表示还原为 int。

    struct_pb2.Struct 只支持 double,JSON-RPC 穿越后 window/shift 等 int 字段
    会变成 20.0,导致 Pydantic strict 校验失败。整数值的 float 无损转 int。
    """
    if isinstance(value, float):
        if value.is_integer():
            return int(value)
        return value
    if isinstance(value, dict):
        return {k: _normalize_numbers(v) for k, v in value.items()}
    if isinstance(value, list):
        return [_normalize_numbers(v) for v in value]
    return value


def _today_reset_hint() -> str:
    return "配额每日 00:00(quant 服务器本地时区)重置"


def _audit_refs(artifact: dict[str, Any]) -> tuple[Any, Any, Any]:
    """从成功 artifact 提取审计关联列 (run_id, experiment_id, trial_id)(§12)。"""
    run_id = experiment_id = trial_id = None
    try:
        if "backtest_summary" in artifact:
            run_id = artifact["backtest_summary"].get("run_id")
        elif "trial_result" in artifact:
            tr = artifact["trial_result"]
            ref = tr.get("detail_ref") or {}
            run_id = ref.get("run_id")
            experiment_id = ref.get("experiment_id")
            trial_id = (tr.get("trial") or {}).get("id")
        elif "trial_batch_result" in artifact:
            items = artifact["trial_batch_result"].get("items") or []
            if items:
                ref = items[0].get("detail_ref") or {}
                experiment_id = ref.get("experiment_id")
        elif isinstance(artifact.get("experiment"), dict):
            exp = artifact["experiment"]
            if exp.get("id") is not None:
                experiment_id = exp.get("id")
    except Exception:  # noqa: BLE001 - 审计关联列缺失不阻断主流程
        pass
    return run_id, experiment_id, trial_id


class QuantAgentExecutor(AgentExecutor):
    """确定性 quant skill 执行器：无 LLM，只路由到 domain handler。"""

    def __init__(self) -> None:
        self._active: dict[str, dict[str, Any]] = {}

    async def execute(self, context: RequestContext, event_queue: EventQueue) -> None:
        task_id = context.task_id or str(uuid.uuid4())
        context_id = context.context_id or task_id
        call_context = context.call_context
        claims = call_context.state.get("claims")
        source = "trace_chat"

        # 先创建 Task 对象，后续所有状态/artifact 都基于它更新
        initial_task = new_task(
            task_id=task_id,
            context_id=context_id,
            state=TaskState.TASK_STATE_SUBMITTED,
        )
        await event_queue.enqueue_event(initial_task)
        await self._emit_status(
            event_queue, task_id, context_id, TaskState.TASK_STATE_WORKING,
        )

        try:
            if not claims:
                raise ValueError("未登录")
            user_id = user_id_from_claims(claims)

            message = context.message
            metadata = (
                json_format.MessageToDict(message.metadata)
                if message is not None and message.HasField("metadata")
                else {}
            )
            source = metadata.get("source", "trace_chat") or "trace_chat"

            data_parts = (
                get_data_parts(message.parts)
                if message is not None
                else []
            )
            text_parts = (
                get_text_parts(message.parts)
                if message is not None
                else []
            )
            if not data_parts:
                if text_parts:
                    raise ValueError(
                        "仅 text 的请求被拒绝。quant 是确定性工具节点，"
                        "调用方必须发送 data part：{\"skill\": \"...\", \"payload\": {...}}。"
                        f"可用 skill id：{', '.join(sorted(SKILL_IDS))}"
                    )
                raise ValueError("请求中未找到 data part")

            data = data_parts[0]
            if not isinstance(data, dict):
                raise ValueError("data part 必须是 JSON 对象")
            skill = data.get("skill")
            payload = data.get("payload") or {}
            if not isinstance(payload, dict):
                raise ValueError("payload 必须是 JSON 对象")

            if skill not in SKILL_IDS:
                raise ValueError(
                    f"未知 skill '{skill}'。可用 skill id："
                    f"{', '.join(sorted(SKILL_IDS))}"
                )

            # 全局授权：除 Card 外全部需登录；factor.save_draft / gap_summary.global 需 admin
            if not can_client(claims):
                raise ValueError("没有量化研究系统访问权限")
            if skill == "factor.save_draft" and not can_admin(claims):
                raise ValueError("factor.save_draft 仅管理员可用")
            if skill == "system.gap_summary":
                scope = payload.get("scope", "me")
                if scope == "global" and not can_admin(claims):
                    raise ValueError("scope=global 仅管理员可用")

            # 读/create 类限速
            if skill not in HIGH_COST_SKILLS:
                if not rate_limiter.check(user_id):
                    raise ValueError(
                        "读/create 类请求过快(60/分钟)，请稍后再试"
                    )

            # 高成本闸门
            if skill in HIGH_COST_SKILLS:
                if payload.get("confirmed") is not True:
                    raise ValueError(
                        f"{skill} 是高成本 skill，需要 confirmed=true 并由用户确认后注入"
                    )
                cost = (
                    len(payload.get("param_patches", []))
                    if skill == _HIGH_COST_BATCH_SKILL
                    else 1
                )
                if not quota_tracker.is_available(user_id, cost):
                    remaining = quota_tracker.remaining(user_id)
                    raise ValueError(
                        f"今日高成本 skill 配额不足，需要 {cost}，"
                        f"剩余 {remaining}。{_today_reset_hint()}"
                    )
                # 幂等
                existing = find_idempotent_task(
                    user_id,
                    payload.get("client_request_id"),
                    skill,
                )
                if existing is not None and existing.status == "done":
                    artifact = existing.result or {}
                    await self._emit_artifact(
                        event_queue, task_id, context_id, artifact,
                    )
                    await self._emit_status(
                        event_queue, task_id, context_id,
                        TaskState.TASK_STATE_COMPLETED,
                    )
                    return

            cancel_event: threading.Event | None = None
            if skill in HIGH_COST_SKILLS:
                cancel_event = threading.Event()
                self._active[task_id] = {
                    "event": cancel_event,
                    "skill": skill,
                }

            try:
                loop = asyncio.get_event_loop()
                artifact = await loop.run_in_executor(
                    None,
                    self._run_skill,
                    skill,
                    payload,
                    user_id,
                    claims,
                    source,
                    task_id,
                    cancel_event,
                )
            finally:
                self._active.pop(task_id, None)

            # 审计（旁路）
            failure_kind, missing_capability = self._audit_info(
                skill, artifact, error=None,
            )
            run_id, experiment_id, trial_id = _audit_refs(artifact)
            await loop.run_in_executor(
                None,
                functools.partial(
                    record_audit,
                    user_id=user_id,
                    a2a_task_id=task_id,
                    skill=skill,
                    source=source,
                    run_id=run_id,
                    experiment_id=experiment_id,
                    trial_id=trial_id,
                    failure_kind=failure_kind,
                    missing_capability=missing_capability,
                ),
            )

            await self._emit_artifact(
                event_queue, task_id, context_id, artifact,
            )
            await self._emit_status(
                event_queue, task_id, context_id,
                TaskState.TASK_STATE_COMPLETED,
            )

        except Exception as exc:  # noqa: BLE001
            logger.exception("A2A skill 执行失败 skill=%s", skill if 'skill' in locals() else None)
            failure_kind = "runtime_error"
            missing_capability = None
            if isinstance(exc, StrategyCapabilityError):
                failure_kind = exc.report.status.value
                issue = exc.report.issues[0] if exc.report.issues else None
                missing_capability = issue.code if issue else None
            elif isinstance(exc, ValueError) and "表达式校验失败" in str(exc):
                failure_kind = "missing_engine"
                missing_capability = "expression_invalid"

            if 'user_id' in locals() and 'skill' in locals():
                try:
                    await asyncio.get_event_loop().run_in_executor(
                        None,
                        functools.partial(
                            record_audit,
                            user_id=user_id,
                            a2a_task_id=task_id,
                            skill=skill,
                            source=source,
                            run_id=None,
                            experiment_id=None,
                            trial_id=None,
                            failure_kind=failure_kind,
                            missing_capability=missing_capability,
                        ),
                    )
                except Exception:  # noqa: BLE001
                    logger.exception("失败审计写入失败")

            await self._emit_status(
                event_queue, task_id, context_id,
                TaskState.TASK_STATE_FAILED,
                message=str(exc),
            )

    async def cancel(self, context: RequestContext, event_queue: EventQueue) -> None:
        task_id = context.task_id or ""
        active = self._active.get(task_id)
        if active is not None:
            active["event"].set()
        # 同时尝试取消关联的 quant_task（如存在）
        # 实际取消由执行线程轮询 event 后调用 cancel_task 处理
        logger.info("A2A cancel 信号已发送 task_id=%s", task_id)

    def _run_skill(
        self,
        skill: str,
        payload: dict[str, Any],
        user_id: str,
        claims: dict,
        source: str,
        a2a_task_id: str,
        cancel_event: threading.Event | None,
    ) -> dict[str, Any]:
        """在独立线程中执行 skill；所有 DB 操作使用新 Session。"""
        db = SessionLocal()
        try:
            ctx = A2AContext(
                user_id=user_id,
                claims=claims,
                source=source,
                a2a_task_id=a2a_task_id,
                db=db,
            )
            handler = SKILLS[skill]
            result = handler(_normalize_numbers(payload), ctx, cancel_event)
            if db.is_active:
                db.commit()
            return result
        except Exception:
            if db.is_active:
                db.rollback()
            raise
        finally:
            db.close()

    def _audit_info(
        self,
        skill: str,
        artifact: dict[str, Any],
        error: Exception | None,
    ) -> tuple[str | None, str | None]:
        """从 artifact 提取 validate 失败等缺口信号。"""
        if error is not None:
            return None, None
        if skill == "strategy.validate":
            vr = artifact.get("validation_result", {})
            capability = vr.get("capability") or {}
            if vr.get("valid") is not True and capability.get("status") != "supported":
                issues = capability.get("issues", [])
                issue = issues[0] if issues else {}
                return capability.get("status"), issue.get("code")
        if skill == "factor.validate":
            fv = artifact.get("factor_validation", {})
            if not fv.get("valid"):
                capability = fv.get("capability") or {}
                issues = capability.get("issues", [])
                issue = issues[0] if issues else {}
                return capability.get("status") or "missing_engine", issue.get("code")
        # 运行期失败(trial outcome=error)同样是一等缺口信号(§12/§18)
        if skill == "experiment.trial" and "trial_result" in artifact:
            trial = artifact["trial_result"].get("trial") or {}
            if trial.get("outcome") == "error":
                return "runtime_error", None
        if skill == "experiment.trial_batch" and "trial_batch_result" in artifact:
            items = artifact["trial_batch_result"].get("items") or []
            if any((i.get("trial") or {}).get("outcome") == "error" for i in items):
                return "runtime_error", None
        return None, None

    async def _emit_status(
        self,
        event_queue: EventQueue,
        task_id: str,
        context_id: str,
        state: TaskState,
        message: str | None = None,
    ) -> None:
        event = new_text_status_update_event(
            task_id=task_id,
            context_id=context_id,
            state=state,
            text=message or "",
        )
        await event_queue.enqueue_event(event)

    async def _emit_artifact(
        self,
        event_queue: EventQueue,
        task_id: str,
        context_id: str,
        artifact: dict[str, Any],
    ) -> None:
        # artifact dict 通常只有一个顶层 key，即 design 约定的 artifact name
        name = "result"
        if artifact and isinstance(artifact, dict):
            name = next(iter(artifact.keys()))
        event = new_data_artifact_update_event(
            task_id=task_id,
            context_id=context_id,
            name=name,
            data=artifact,
        )
        await event_queue.enqueue_event(event)


def build_a2a_handler() -> DefaultRequestHandlerV2:
    """构造 SDK RequestHandler（含 executor / task store / context builder）。"""
    agent_card = build_agent_card()
    executor = QuantAgentExecutor()
    request_context_builder = SimpleRequestContextBuilder(
        should_populate_referred_tasks=False,
        task_store=short_task_store,
    )
    return DefaultRequestHandlerV2(
        agent_executor=executor,
        task_store=short_task_store,
        agent_card=agent_card,
        request_context_builder=request_context_builder,
    )


def mount_a2a_routes(app) -> DefaultRequestHandlerV2:
    """在 FastAPI app 上挂载 Card（公开）与 /a2a JSON-RPC（鉴权）。"""
    from a2a.server.routes.agent_card_routes import create_agent_card_routes

    handler = build_a2a_handler()
    agent_card_routes = create_agent_card_routes(
        build_agent_card(),
        card_url="/.well-known/agent-card.json",
    )
    jsonrpc_routes = create_jsonrpc_routes(
        request_handler=handler,
        rpc_url="/a2a",
        context_builder=QuantA2AContextBuilder(),
        enable_v0_3_compat=True,
    )
    add_a2a_routes_to_fastapi(
        app,
        agent_card_routes=agent_card_routes,
        jsonrpc_routes=jsonrpc_routes,
    )
    return handler


__all__ = ["QuantAgentExecutor", "build_a2a_handler", "mount_a2a_routes"]
