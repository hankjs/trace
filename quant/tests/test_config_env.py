"""运行环境 env / QUANT_ENV 规范化。"""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.config import ENV_DEV, ENV_PROD, normalize_env


@pytest.mark.parametrize(
    "raw,expected",
    [
        (None, ENV_DEV),
        ("", ENV_DEV),
        ("dev", ENV_DEV),
        ("development", ENV_DEV),
        ("local", ENV_DEV),
        ("DEV", ENV_DEV),
        ("prod", ENV_PROD),
        ("production", ENV_PROD),
        ("PROD", ENV_PROD),
        ("  production  ", ENV_PROD),
        ("staging", ENV_DEV),  # 未知值安全回落到 dev
        ("whatever", ENV_DEV),
    ],
)
def test_normalize_env(raw, expected):
    assert normalize_env(raw) == expected
