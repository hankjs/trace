"""表达式级校验、哈希与能力 API 测试。"""
from __future__ import annotations

import hashlib
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.strategy.spec import (
    CapabilityStatus,
    Expression,
    ExpressionValidationResult,
    canonical_expression_json,
    expression_hash,
    validate_expression,
)


def _make_deep_chain(depth: int) -> dict:
    """构造一条 add 左嵌套链,深度为 depth(包含叶子)。"""
    node: dict = {"op": "field", "name": "close"}
    for _ in range(depth - 1):
        node = {"op": "add", "left": node, "right": {"op": "literal", "value": 1}}
    return node


def test_valid_numeric_expression_populates_all_fields():
    value = {
        "op": "ma",
        "input": {"op": "field", "name": "close"},
        "window": 20,
    }
    result = validate_expression(value)
    assert isinstance(result, ExpressionValidationResult)
    assert result.valid is True
    assert result.result_type == "number"
    assert result.min_bars == 20
    assert result.used_fields == ["close"]
    assert result.expression_hash is not None
    assert result.canonical_json == canonical_expression_json(value)
    assert result.capability.status == CapabilityStatus.SUPPORTED
    assert result.capability.issues == []


def test_bool_root_with_number_require_type_is_invalid():
    value = {
        "op": "gt",
        "left": {"op": "field", "name": "close"},
        "right": {"op": "literal", "value": 10},
    }
    result = validate_expression(value, require_type="number")
    assert result.valid is False
    assert result.capability.status == CapabilityStatus.MISSING_ENGINE
    assert any("必须返回 number" in issue.message for issue in result.capability.issues)


def test_expression_ast_depth_limit_enforced():
    value = _make_deep_chain(13)
    result = validate_expression(value)
    assert result.valid is False
    assert result.capability.status == CapabilityStatus.MISSING_ENGINE
    assert any("AST 深度" in issue.message for issue in result.capability.issues)


def test_expression_ast_node_limit_enforced():
    # 构造 257 个节点的 add 链
    value = _make_deep_chain(257)
    result = validate_expression(value)
    assert result.valid is False
    assert result.capability.status == CapabilityStatus.MISSING_ENGINE
    messages = [issue.message for issue in result.capability.issues]
    assert any(
        "AST 节点数" in message or "Recursion error" in message
        for message in messages
    )


def test_unknown_operator_returns_unknown_operator_issue():
    value = {"op": "magic_op", "input": {"op": "field", "name": "close"}}
    result = validate_expression(value)
    assert result.valid is False
    assert result.capability.status == CapabilityStatus.MISSING_ENGINE
    assert any(
        issue.code == "unknown_operator"
        for issue in result.capability.issues
    )


def test_unknown_field_returns_unknown_field_issue():
    value = {"op": "field", "name": "not_a_real_field_xyz"}
    result = validate_expression(value)
    assert result.valid is False
    assert result.capability.status == CapabilityStatus.MISSING_DATA
    assert any(
        issue.code == "unknown_field"
        for issue in result.capability.issues
    )


def test_forbidden_key_in_subtree_is_boundary_denied():
    value = {"op": "field", "name": "close", "eval": 1}
    result = validate_expression(value)
    assert result.valid is False
    assert result.capability.status == CapabilityStatus.BOUNDARY_DENIED
    assert any(
        issue.code == "arbitrary_code_denied"
        for issue in result.capability.issues
    )


def test_denied_string_pattern_in_field_name():
    value = {"op": "field", "name": "eval(something)"}
    result = validate_expression(value)
    assert result.valid is False
    assert result.capability.status == CapabilityStatus.BOUNDARY_DENIED
    assert any(
        issue.code == "arbitrary_code_denied"
        for issue in result.capability.issues
    )


def test_key_order_invariance_of_expression_hash():
    a = {"op": "gt", "left": {"op": "field", "name": "close"}, "right": {"op": "literal", "value": 10}}
    b = {"right": {"value": 10, "op": "literal"}, "op": "gt", "left": {"name": "close", "op": "field"}}
    assert expression_hash(a) == expression_hash(b)
    canonical = canonical_expression_json(a)
    expected = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
    assert expression_hash(a) == expected


def test_validate_expression_accepts_expression_instance():
    expr = Expression.model_validate({"op": "field", "name": "close"})
    result = validate_expression(expr)
    assert result.valid is True
    assert result.result_type == "number"


def test_validate_expression_available_fields_check():
    value = {"op": "field", "name": "close"}
    result = validate_expression(value, available_fields={"open", "high"})
    assert result.valid is False
    assert result.capability.status == CapabilityStatus.MISSING_DATA
    assert any(
        issue.code == "field_not_available"
        for issue in result.capability.issues
    )
