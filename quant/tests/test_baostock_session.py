"""baostock 会话可重入:嵌套 login_session 不得提前 logout。

依据 REVIEW §3.5:`scripts/backfill_universe.py` 外层开 `login_session()`,
`universe.sync_all_indices` 内层又开一个,内层 finally 会 logout 并置
`_logged_in = False`,外层剩余工作在未登录状态下跑。
"""
from __future__ import annotations

import sys
import threading
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.data import baostock_client as bc


def _ok_login():
    return mock.Mock(error_code="0")


def test_nested_login_session_does_not_logout_early():
    with mock.patch.object(bc, "bs") as bs:
        bs.login.return_value = _ok_login()
        with bc.login_session():
            assert bc._login_depth == 1
            with bc.login_session():
                assert bc._login_depth == 2
            # 内层退出后必须仍处于登录状态,否则外层剩余工作全部无会话
            assert bc._login_depth == 1
            assert bs.logout.call_count == 0, "内层提前 logout"
        assert bc._login_depth == 0
        assert bs.logout.call_count == 1
        assert bs.login.call_count == 1, "嵌套时重复登录"


def test_fetch_inside_session_reuses_login():
    """会话内 fetch 不重复 login/logout。"""
    with mock.patch.object(bc, "bs") as bs:
        bs.login.return_value = _ok_login()
        rs = mock.Mock(error_code="0", fields=["date", "close"])
        rs.next.return_value = False
        bs.query_trade_dates.return_value = rs
        with bc.login_session():
            bc.fetch_trade_dates("2026-01-01", "2026-01-31")
            assert bs.logout.call_count == 0
        assert bs.login.call_count == 1
        assert bs.logout.call_count == 1


def test_fetch_outside_session_owns_its_login():
    """会话外单次 fetch 自行登录并登出。"""
    with mock.patch.object(bc, "bs") as bs:
        bs.login.return_value = _ok_login()
        rs = mock.Mock(error_code="0", fields=["date", "close"])
        rs.next.return_value = False
        bs.query_trade_dates.return_value = rs
        bc.fetch_trade_dates("2026-01-01", "2026-01-31")
        assert bs.login.call_count == 1
        assert bs.logout.call_count == 1
        assert bc._login_depth == 0


def test_login_failure_does_not_leak_refcount():
    """登录失败时引用计数不得残留,否则后续所有 fetch 都以为已登录。"""
    with mock.patch.object(bc, "bs") as bs:
        bs.login.return_value = mock.Mock(error_code="10001",
                                          error_msg="登录失败")
        try:
            with bc.login_session():
                pass
        except RuntimeError:
            pass
        else:
            raise AssertionError("登录失败未抛异常")
        assert bc._login_depth == 0, "登录失败泄漏了引用计数"


def test_refcount_is_lock_protected_under_concurrency():
    """并发 acquire/release 后引用计数必须归零(原实现在锁外读 _logged_in)。"""
    with mock.patch.object(bc, "bs") as bs:
        bs.login.return_value = _ok_login()
        errors: list[BaseException] = []

        def worker():
            try:
                for _ in range(50):
                    with bc.login_session():
                        pass
            except BaseException as e:  # noqa: BLE001
                errors.append(e)

        threads = [threading.Thread(target=worker) for _ in range(8)]
        for t in threads:
            t.start()
        for t in threads:
            t.join()

        assert errors == []
        assert bc._login_depth == 0
