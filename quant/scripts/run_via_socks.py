"""经本机 SOCKS5 代理运行指定脚本:baostock 被封 IP 时的运维兜底。

## 何时需要

baostock 官方:每日 API ≤ 5 万次、禁止并发连接;超限按出口 IP 进黑名单
(常见 10001011)。本脚本只在**已被封**时换干净出口继续串行任务,不是
用来提高吞吐的 —— 换 IP 后仍须遵守日配额与禁止并发,详见
DATA-ARCHITECTURE.md 第 5 节。

baostock SDK 走原生 TCP,不读 http_proxy/all_proxy 环境变量,所以在
进程内把 socket.socket 换成选择性代理实现:

- 局域网地址、回环地址、以及启动时解析出的数据库主机 —— 直连,
  MySQL 流量不过代理(云数据库经代理既慢又多一个故障点);
- 其余连接(baostock)走 SOCKS5,出口 IP 与直连不同,绕开封禁。

用法:
    uv run --with pysocks python scripts/run_via_socks.py scripts/backfill_is_st.py [参数...]
    uv run --with pysocks python scripts/run_via_socks.py -c "python 代码"

代理地址取环境变量 QUANT_SOCKS_PROXY(默认 127.0.0.1:7890,Clash 混合端口)。
pysocks 用 uv run --with 临时引入,不进项目依赖。
"""
from __future__ import annotations

import ipaddress
import os
import runpy
import socket
import sys

import socks

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

_DIRECT_NETWORKS = [
    ipaddress.ip_network(n)
    for n in ("127.0.0.0/8", "10.0.0.0/8", "172.16.0.0/12",
              "192.168.0.0/16", "169.254.0.0/16", "::1/128")
]


def _db_ips() -> set[str]:
    """解析配置里的 DB 主机 IP,其流量保持直连。"""
    try:
        from urllib.parse import urlparse

        from app.config import settings
        url = urlparse(settings.database_url)
        if not url.hostname:
            return set()
        infos = socket.getaddrinfo(url.hostname, url.port or 3306)
        return {info[4][0] for info in infos}
    except Exception:  # noqa: BLE001 - 解析失败则退化为只按局域网直连
        return set()


def install() -> None:
    proxy = os.environ.get("QUANT_SOCKS_PROXY", "127.0.0.1:7890")
    host, _, port = proxy.partition(":")
    socks.set_default_proxy(socks.SOCKS5, host, int(port or "7890"))
    direct_ips = _db_ips()
    if REPO_ROOT not in sys.path:
        sys.path.insert(0, REPO_ROOT)

    class _SelectiveSocket(socks.socksocket):
        def connect(self, address):
            dest = address[0]
            try:
                ip = ipaddress.ip_address(dest)
                if dest in direct_ips or any(ip in n for n in _DIRECT_NETWORKS):
                    self.set_proxy(None)  # 直连
            except ValueError:
                pass  # 域名:交给 SOCKS5 远端解析
            super().connect(address)

    socket.socket = _SelectiveSocket


def main() -> None:
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    install()
    target = sys.argv[1]
    if target == "-c":
        code = sys.argv[2]
        sys.argv = ["-c", *sys.argv[3:]]
        exec(compile(code, "<command>", "exec"), {"__name__": "__main__"})  # noqa: S102
    else:
        sys.argv = [target, *sys.argv[2:]]
        runpy.run_path(target, run_name="__main__")


if __name__ == "__main__":
    main()
