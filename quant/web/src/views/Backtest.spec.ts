import { flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { BacktestResult, Strategy, StrategyCapability } from '../api'
import { createBreakoutStrategySpec } from '../strategySpecForm'

const spec = (() => {
  const value = createBreakoutStrategySpec()
  value.universe.pool_id = 1
  return value
})()

function strategy(capability: StrategyCapability = { status: 'supported', issues: [] }): Strategy {
  return {
    id: 7,
    name: '20 日放量突破',
    template: 'strategy_spec',
    template_name: '数据库策略',
    kind: 'single',
    kind_name: '单只股票',
    params: {},
    effective_params: {},
    params_valid: true,
    enabled: true,
    is_system: false,
    editable: true,
    spec,
    spec_hash: 'current-spec-hash',
    capability,
  }
}

function backtestResult(): BacktestResult {
  return {
    run_id: 21,
    strategy_id: 7,
    strategy_name: '20 日放量突破',
    strategy_spec_snapshot: spec,
    strategy_spec_hash: 'current-spec-hash',
    compiler_version: 'strategy-compiler-v1',
    component_versions: { rolling_max: '1', rolling_min: '1' },
    data_fingerprint: 'data-fingerprint-123',
    universe_fingerprint: 'universe-fingerprint-123',
    cost_fingerprint: 'cost-fingerprint-123',
    execution_fingerprint: 'execution-fingerprint-123',
    codes: ['sh.600519'],
    stocks: [{ code: 'sh.600519', name: '贵州茅台' }],
    start: '2024-01-01',
    end: '2024-12-31',
    costs: { commission: 0.00025, stamp_tax: 0.0005, slippage: 0.0001 },
    metrics: {
      total_return: 0.12,
      annual_return: 0.11,
      max_drawdown: -0.08,
      win_rate: 0.55,
      trade_count: 4,
      round_trips: 2,
    },
    equity: [],
    trade_details: [],
  }
}

async function mountPage(item: Strategy) {
  const { api } = await import('../api')
  vi.spyOn(api, 'strategies').mockResolvedValue({
    items: [item],
    limits: { max_total: 50, max_enabled: 10 },
  })
  vi.spyOn(api, 'watchlist').mockResolvedValue({ count: 0, items: [] })
  vi.spyOn(api, 'stockList').mockResolvedValue({
    count: 1,
    items: [{ code: 'sh.600519', name: '贵州茅台', industry: '白酒', is_watch: false }],
  })
  vi.spyOn(api, 'pools').mockResolvedValue({
    count: 1,
    items: [{ id: 2, kind: 'all', name: '全部A股', min_list_days: 60, is_system: true }],
  })
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', component: { template: '<div />' } },
      { path: '/strategies/manage', name: 'strategies-manage', component: { template: '<div />' } },
    ],
  })
  await router.push('/')
  await router.isReady()
  const Component = (await import('./Backtest.vue')).default
  const wrapper = mount(Component, {
    global: {
      plugins: [router],
      stubs: { EChart: true },
    },
  })
  await flushPromises()
  // 通过 QuSelect 选择策略:打开下拉,点击对应选项
  const strategyTrigger = wrapper.findAll('button[aria-haspopup="listbox"]')
    .find((button) => button.text().includes(item.name))
  expect(strategyTrigger).toBeDefined()
  await strategyTrigger!.trigger('click')
  const strategyOption = wrapper.findAll('button[role="option"]')
    .find((button) => button.text().includes(item.name))
  expect(strategyOption).toBeDefined()
  await strategyOption!.trigger('click')
  await flushPromises()
  return { wrapper, api }
}

/** QuDatePicker 选日期:打开面板,向前翻月直到目标日期出现在网格里再点选 */
async function pickDate(wrapper: Awaited<ReturnType<typeof mountPage>>['wrapper'], ariaLabel: string, iso: string) {
  await wrapper.get(`button[aria-label="${ariaLabel}"]`).trigger('click')
  for (let i = 0; i < 120; i++) {
    const day = wrapper.find(`button[data-date="${iso}"]`)
    if (day.exists()) {
      await day.trigger('click')
      return
    }
    await wrapper.get('button[aria-label="上个月"]').trigger('click')
  }
  throw new Error(`未能在日历中定位日期 ${iso}`)
}

async function fillScope(wrapper: Awaited<ReturnType<typeof mountPage>>['wrapper']) {
  await pickDate(wrapper, '开始日期', '2024-01-01')
  await pickDate(wrapper, '结束日期', '2024-12-31')
  // 通过选股器选择 sh.600519:打开弹层,点击对应行(排除 QuSelect 的触发按钮)
  const pickerTrigger = wrapper.findAll('button[aria-haspopup="listbox"]')
    .find((button) => button.text().includes('点击选择股票'))
  expect(pickerTrigger).toBeDefined()
  await pickerTrigger!.trigger('click')
  const row = wrapper.findAll('button[role="option"]').find((b) => b.text().includes('sh.600519'))
  expect(row).toBeDefined()
  await row!.trigger('click')
}

beforeEach(() => {
  vi.restoreAllMocks()
  vi.resetModules()
})

describe('saved StrategySpec backtest workflow', () => {
  it('submits only the saved strategy id and shows immutable evidence', async () => {
    const { wrapper, api } = await mountPage(strategy())
    const run = vi.spyOn(api, 'runBacktest').mockResolvedValue(backtestResult())
    await fillScope(wrapper)

    await wrapper.get('form').trigger('submit')
    await flushPromises()

    expect(run).toHaveBeenCalledWith({
      strategy_id: 7,
      codes: ['sh.600519'],
      start: '2024-01-01',
      end: '2024-12-31',
      costs: { commission: 0.00025, stamp_tax: 0.0005, slippage: 0.0001 },
    })
    expect(run.mock.calls[0][0]).not.toHaveProperty('params')
    expect(wrapper.text()).toContain('不可变执行证据')
    expect(wrapper.text()).toContain('与当前策略一致')
    expect(wrapper.text()).toContain('strategy-compiler-v1')
    expect(wrapper.text()).toContain('execution-fingerprint-123')
    expect(wrapper.text()).toContain('完整 StrategySpec 快照')
  })

  it('uses only controlled $.path values for a sweep', async () => {
    const { wrapper, api } = await mountPage(strategy())
    const sweep = vi.spyOn(api, 'sweepBacktest').mockResolvedValue({
      strategy_id: 7,
      strategy_name: '20 日放量突破',
      template: 'strategy_spec',
      strategy_spec_hash: 'current-spec-hash',
      codes: ['sh.600519'],
      start: '2024-01-01',
      end: '2024-12-31',
      results: [],
    })

    await wrapper.findAll('button').find((button) => button.text() === '参数扫描')!.trigger('click')
    await fillScope(wrapper)
    await wrapper.get<HTMLInputElement>('[data-testid="sweep-path"]').setValue('$.overlays.risk.value')
    await wrapper.get<HTMLInputElement>('[data-testid="sweep-values"]').setValue('0.05, 0.08')

    await wrapper.get('form').trigger('submit')
    await flushPromises()

    expect(sweep).toHaveBeenCalledWith({
      strategy_id: 7,
      codes: ['sh.600519'],
      start: '2024-01-01',
      end: '2024-12-31',
      param_grid: { '$.overlays.risk.value': [0.05, 0.08] },
      costs: { commission: 0.00025, stamp_tax: 0.0005, slippage: 0.0001 },
    })
  })

  it('runs a single strategy against a pool', async () => {
    const { wrapper, api } = await mountPage(strategy())
    const run = vi.spyOn(api, 'runBacktest').mockResolvedValue(backtestResult())
    await pickDate(wrapper, '开始日期', '2024-01-01')
    await pickDate(wrapper, '结束日期', '2024-12-31')

    // 切到「按股票池」模式,PoolSelect 就绪后落到默认池(全部A股 id=2)
    const poolModeButton = wrapper.findAll('button').find((b) => b.text() === '按股票池')!
    await poolModeButton.trigger('click')
    await flushPromises()

    await wrapper.get('form').trigger('submit')
    await flushPromises()

    expect(run).toHaveBeenCalledWith({
      strategy_id: 7,
      codes: [],
      start: '2024-01-01',
      end: '2024-12-31',
      pool_id: 2,
      costs: { commission: 0.00025, stamp_tax: 0.0005, slippage: 0.0001 },
    })
  })

  it('blocks a strategy with capability failures', async () => {
    const capability: StrategyCapability = {
      status: 'missing_engine',
      issues: [{
        status: 'missing_engine',
        code: 'unknown_operator',
        path: '$.entry.condition.op',
        message: '当前编译器不支持操作符 future_magic',
      }],
    }
    const { wrapper } = await mountPage(strategy(capability))

    expect(wrapper.text()).toContain('当前编译器不支持操作符 future_magic')
    const runButton = wrapper.findAll('button').find((button) => button.text() === '运行回测')
    expect(runButton?.attributes('disabled')).toBeDefined()
  })
})
