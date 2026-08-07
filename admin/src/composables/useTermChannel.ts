/**
 * 终端 IO 通道：RTC 优先（DataChannel 直连），失败/心跳判死 → 静默回落中转。
 * 协议对齐 handy/docs/p2p-terminal.md。
 */
import { ref, type Ref } from 'vue'
import { api } from './api'

export type TermChannelMode = 'rtc' | 'relay'

export interface TermChannel {
  mode: Ref<TermChannelMode>
  write(data: Uint8Array): void
  resize(cols: number, rows: number): void
  close(): void
}

export interface TermChannelHandlers {
  onData(data: Uint8Array): void
  onClosed?(reason: string): void
  onError?(message: string): void
}

export interface TermChannelOptions {
  rtcFactory?: (config: RTCConfiguration) => RTCPeerConnection
  getSize?: () => { cols: number; rows: number }
  rtcTimeoutMs?: number
}

const RTC_TIMEOUT_MS = 6000
const PING_INTERVAL_MS = 10000
const PONG_WINDOW_MS = 15000
const MAX_MISSED_PONGS = 3

function b64encode(data: Uint8Array): string {
  let s = ''
  for (let i = 0; i < data.length; i++) s += String.fromCharCode(data[i])
  return btoa(s)
}

function b64decode(text: string): Uint8Array {
  const bin = atob(text)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

interface TermLink {
  write(data: Uint8Array): void
  resize(cols: number, rows: number): void
  stop(): void
}

/** 中转：3s 拉 raw 整屏 + terminal_input 写 */
function createRelayLink(
  clientId: string,
  termId: string,
  handlers: TermChannelHandlers,
): TermLink & { start(): void } {
  let stopped = false
  let lastSnapshot = ''
  let timer: ReturnType<typeof setInterval> | null = null

  async function pull(): Promise<void> {
    if (stopped) return
    try {
      const res = await api.terminalOutputRaw(clientId, termId)
      if (stopped) return
      if (res.output !== lastSnapshot) {
        lastSnapshot = res.output
        // 整屏回放：前缀清屏复位，避免叠字
        handlers.onData(
          new TextEncoder().encode('\x1b[H\x1b[2J\x1b[3J' + res.output),
        )
      }
    } catch (e) {
      if (!stopped) {
        handlers.onError?.(e instanceof Error ? e.message : '中转拉取失败')
      }
    }
  }

  return {
    start() {
      void pull()
      timer = setInterval(() => void pull(), 3000)
    },
    write(data) {
      // 文本按 UTF-8 发；\r 由调用方保证
      const text = new TextDecoder().decode(data)
      void api.terminalInput(clientId, termId, text).catch(() => {})
    },
    resize(_cols, _rows) {
      // 中转暂不主动 resize 远端
    },
    stop() {
      stopped = true
      if (timer) clearInterval(timer)
      timer = null
    },
  }
}

interface DcMessage {
  type: string
  seq?: number
  ansi_b64?: string
  data_b64?: string
  reason?: string
}

function waitIceGathered(pc: RTCPeerConnection): Promise<void> {
  if (pc.iceGatheringState === 'complete') return Promise.resolve()
  return new Promise((resolve) => {
    pc.addEventListener('icegatheringstatechange', () => {
      if (pc.iceGatheringState === 'complete') resolve()
    })
  })
}

function waitChannelOpen(dc: RTCDataChannel): Promise<void> {
  if (dc.readyState === 'open') return Promise.resolve()
  return new Promise((resolve, reject) => {
    dc.onopen = () => resolve()
    dc.onclose = () => reject(new Error('datachannel closed'))
  })
}

async function connectRtcLink(
  factory: (config: RTCConfiguration) => RTCPeerConnection,
  clientId: string,
  termId: string,
  getSize: () => { cols: number; rows: number },
  handlers: TermChannelHandlers,
  onDead: (reason: string) => void,
  ctl: { cancelled: boolean },
): Promise<TermLink> {
  const ice = await api.rtcIce()
  const pc = factory({ iceServers: ice.iceServers })
  const dc = pc.createDataChannel('term')

  let stopped = false
  let lastSeq = -1
  let missedPongs = 0
  let pingTimer: ReturnType<typeof setTimeout> | null = null
  let pongTimer: ReturnType<typeof setTimeout> | null = null

  function clearTimers(): void {
    if (pingTimer !== null) clearTimeout(pingTimer)
    if (pongTimer !== null) clearTimeout(pongTimer)
    pingTimer = pongTimer = null
  }

  function send(msg: Record<string, unknown>): void {
    if (!stopped && dc.readyState === 'open') dc.send(JSON.stringify(msg))
  }

  function die(reason: string): void {
    if (stopped) return
    stopped = true
    clearTimers()
    pc.close()
    onDead(reason)
  }

  function schedulePing(): void {
    pingTimer = setTimeout(() => {
      send({ type: 'ping' })
      pongTimer = setTimeout(() => {
        missedPongs += 1
        if (missedPongs >= MAX_MISSED_PONGS) die('P2P 连接中断')
        else schedulePing()
      }, PONG_WINDOW_MS)
    }, PING_INTERVAL_MS)
  }

  dc.onmessage = (ev) => {
    if (stopped || ctl.cancelled) return
    let msg: DcMessage
    try {
      msg = JSON.parse(String(ev.data)) as DcMessage
    } catch {
      return
    }
    switch (msg.type) {
      case 'snapshot':
        if (typeof msg.seq === 'number') lastSeq = msg.seq
        if (msg.ansi_b64) handlers.onData(b64decode(msg.ansi_b64))
        break
      case 'output':
        if (typeof msg.seq === 'number') {
          if (lastSeq >= 0 && msg.seq > lastSeq + 1) send({ type: 'resync' })
          lastSeq = Math.max(lastSeq, msg.seq)
        }
        if (msg.data_b64) handlers.onData(b64decode(msg.data_b64))
        break
      case 'pong':
        missedPongs = 0
        if (pongTimer !== null) {
          clearTimeout(pongTimer)
          pongTimer = null
        }
        schedulePing()
        break
      case 'closed':
        stopped = true
        clearTimers()
        pc.close()
        handlers.onClosed?.(msg.reason ?? 'closed')
        break
    }
  }

  function throwIfCancelled(): void {
    if (ctl.cancelled) {
      pc.close()
      throw new Error('rtc cancelled')
    }
  }

  const offer = await pc.createOffer()
  await pc.setLocalDescription(offer)
  throwIfCancelled()
  await waitIceGathered(pc)
  throwIfCancelled()
  const answer = await api.rtcOffer(clientId, pc.localDescription?.sdp ?? '')
  await pc.setRemoteDescription(answer)
  throwIfCancelled()
  await waitChannelOpen(dc)
  throwIfCancelled()

  dc.onclose = () => die('P2P 通道断开')

  const { cols, rows } = getSize()
  send({ type: 'open', term_id: termId, cols, rows })
  schedulePing()

  return {
    write: (data) => send({ type: 'input', data_b64: b64encode(data) }),
    resize: (cols, rows) => send({ type: 'resize', cols, rows }),
    stop: () => {
      stopped = true
      clearTimers()
      pc.close()
    },
  }
}

export function useTermChannel(
  clientId: string,
  termId: string,
  handlers: TermChannelHandlers,
  options: TermChannelOptions = {},
): TermChannel {
  const mode: Ref<TermChannelMode> = ref('relay')
  let closed = false
  let link: TermLink | null = null
  const pendingWrites: Uint8Array[] = []
  const initialSize = options.getSize?.() ?? { cols: 80, rows: 24 }
  let lastCols = initialSize.cols
  let lastRows = initialSize.rows

  function attach(l: TermLink): void {
    link = l
    for (const data of pendingWrites) l.write(data)
    pendingWrites.length = 0
  }

  function startRelay(): void {
    if (closed) return
    mode.value = 'relay'
    const relay = createRelayLink(clientId, termId, handlers)
    relay.start()
    attach(relay)
  }

  async function boot(): Promise<void> {
    const factory =
      options.rtcFactory ??
      (typeof RTCPeerConnection !== 'undefined'
        ? (config: RTCConfiguration) => new RTCPeerConnection(config)
        : undefined)
    if (!factory) {
      startRelay()
      return
    }
    const ctl = { cancelled: false }
    try {
      const rtc = await withTimeout(
        connectRtcLink(
          factory,
          clientId,
          termId,
          () => ({ cols: lastCols, rows: lastRows }),
          handlers,
          (reason) => {
            handlers.onClosed?.(reason)
            startRelay()
          },
          ctl,
        ),
        options.rtcTimeoutMs ?? RTC_TIMEOUT_MS,
      )
      if (closed) {
        rtc.stop()
        return
      }
      mode.value = 'rtc'
      attach(rtc)
    } catch {
      ctl.cancelled = true
      startRelay()
    }
  }
  void boot()

  return {
    mode,
    write(data) {
      if (link) link.write(data)
      else pendingWrites.push(data)
    },
    resize(cols, rows) {
      lastCols = cols
      lastRows = rows
      link?.resize(cols, rows)
    },
    close() {
      closed = true
      link?.stop()
      link = null
      pendingWrites.length = 0
    },
  }
}

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('rtc timeout')), ms)
    p.then(
      (v) => {
        clearTimeout(timer)
        resolve(v)
      },
      (err) => {
        clearTimeout(timer)
        reject(err instanceof Error ? err : new Error(String(err)))
      },
    )
  })
}
