import { flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Strategy, StrategyLimits, StrategySpec, StrategyValidationResult } from '../api'
import { createBreakoutStrategySpec } from '../strategySpecForm'

const limits: StrategyLimits = { max_total: 50, max_enabled: 10 }

function validation(spec: StrategySpec): StrategyValidationResult {
  return {
    valid: true,
    kind: spec.kind ?? 'single',
    spec_schema_version: spec.schema_version,
    normalized_spec: spec,
    spec_hash: 'abc123',
    capability: { status: 'supported', issues: [] },
    errors: [],
  }
}

function editableStrategy(): Strategy {
  const spec = createBreakoutStrategySpec()
  return {
    id: 10,
    name: '我的放量突破',
    template: 'legacy_breakout',
    template_name: '迁移期模板',
    kind: 'single',
    kind_name: '单标的策略',
    params: {},
    effective_params: {},
    params_valid: true,
    enabled: true,
    is_system: false,
    editable: true,
    backtest_count: 0,
    spec_schema_version: 1,
    spec,
    spec_hash: 'abc123',
    research_status: 'unverified',
    capability: { status: 'supported', issues: [] },
  }
}

/** 组合形态的公共策略:验证非突破形态也走结构化表单(只读) */
function presetPortfolioStrategy(): Strategy {
  const base = createBreakoutStrategySpec()
  const spec: StrategySpec = {
    ...base,
    kind: 'portfolio',
    entry: { condition: { op: 'literal', value: true }, reason_code: 'eligible_for_ranking' },
    positioning: {
      type: 'portfolio',
      score: { op: 'momentum', input: { op: 'field', name: 'close' }, window: 20 },
      selection: { type: 'top_n', n: 10 },
      weighting: { type: 'equal' },
      rebalance: { frequency: 'monthly', interval_days: null },
      risk_filter: null,
    },
    native_exit: null,
  }
  return {
    ...editableStrategy(),
    id: 3,
    name: '动量轮动',
    kind: 'portfolio',
    kind_name: '组合策略',
    is_system: true,
    editable: false,
    spec,
  }
}

async function mountPage(strategies: Strategy[]) {
  const { api } = await import('../api')
  vi.spyOn(api, 'strategies').mockResolvedValue({ items: strategies, limits })
  vi.spyOn(api, 'pools').mockResolvedValue({
    items: [{ id: 1, kind: 'index', name: '沪深 300', min_list_days: 0, is_system: true }],
  })
  vi.spyOn(api, 'validateStrategy').mockImplementation(async (id) => {
    const strategy = strategies.find((item) => item.id === id)
    if (!strategy?.spec) throw new Error('missing spec')
    return validation(strategy.spec)
  })
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/', component: { template: '<div />' } }],
  })
  await router.push('/')
  await router.isReady()
  const Component = (await import('./Strategies.vue')).default
  const wrapper = mount(Component, { global: { plugins: [router] } })
  await flushPromises()
  return { wrapper, api }
}

beforeEach(() => {
  vi.restoreAllMocks()
  vi.resetModules()
})

describe('StrategySpec editor', () => {
  it('creates the default breakout rule through the structured editor', async () => {
    const { wrapper, api } = await mountPage([])
    const validate = vi.spyOn(api, 'validateStrategySpec').mockImplementation(async (spec) => validation(spec))
    const create = vi.spyOn(api, 'createStrategy').mockImplementation(async (body) => ({
      ...editableStrategy(),
      name: body.name,
      spec: body.spec,
    }))

    await wrapper.get('button').trigger('click')
    await flushPromises()

    // 结构化表单:基础字段与递归表达式编辑器均渲染
    expect(wrapper.get<HTMLInputElement>('#create-spec-listing-days').element.value).toBe('120')
    expect(wrapper.findAll('select[aria-label="算子"]').length).toBeGreaterThan(0)

    const submit = wrapper.findAll('button').find((button) => button.text() === '校验并创建')
    await submit!.trigger('click')
    await flushPromises()

    expect(validate).toHaveBeenCalledOnce()
    const spec = create.mock.calls[0][0].spec
    expect(spec.universe).toMatchObject({ pool_id: 1, exclude_st: true })
    const entry = spec.entry.condition
    expect(entry.op).toBe('all')
    expect(entry.args?.[0].right).toMatchObject({ op: 'rolling_max', window: 20, shift: 1 })
    expect(entry.args?.[1]).toMatchObject({
      op: 'gt',
      left: { op: 'divide', right: { op: 'rolling_mean', window: 20, shift: 1 } },
      right: { op: 'literal', value: 1.5 },
    })
    expect(spec.native_exit?.condition).toMatchObject({
      op: 'lt',
      right: { op: 'rolling_min', window: 10, shift: 1 },
    })
  })

  it('saves the normalized current spec in place', async () => {
    const strategy = editableStrategy()
    const { wrapper, api } = await mountPage([strategy])
    const validate = vi.spyOn(api, 'validateStrategySpec').mockImplementation(async (spec) => validation(spec))
    const update = vi.spyOn(api, 'updateStrategy').mockResolvedValue(strategy)

    await wrapper.get<HTMLInputElement>('#edit-spec-listing-days').setValue(90)
    await wrapper.get<HTMLInputElement>('#edit-spec-cooldown-days').setValue(3)
    const save = wrapper.findAll('button').find((button) => button.text() === '校验并保存')
    await save!.trigger('click')
    await flushPromises()

    expect(validate).toHaveBeenCalledOnce()
    expect(update).toHaveBeenCalledWith(strategy.id, {
      name: strategy.name,
      spec: expect.objectContaining({
        universe: expect.objectContaining({ min_listing_days: 90 }),
        holding: expect.objectContaining({ cooldown_days: 3 }),
        native_exit: expect.objectContaining({
          condition: expect.objectContaining({
            op: 'lt',
            right: expect.objectContaining({ op: 'rolling_min', window: 10 }),
          }),
        }),
      }),
    })
    expect(wrapper.text()).toContain('当前数据与引擎支持')
    expect(wrapper.text()).toContain('规范化 JSON 只读预览')
  })

  it('renders a preset portfolio strategy in the structured readonly form', async () => {
    const strategy = presetPortfolioStrategy()
    const { wrapper } = await mountPage([strategy])

    // 组合段落渲染、原生离场隐藏、公共策略输入只读但仍为结构化表单
    expect(wrapper.text()).toContain('组合构建')
    expect(wrapper.text()).toContain('评分表达式')
    expect(wrapper.text()).not.toContain('原生离场')
    expect(wrapper.find<HTMLSelectElement>('#edit-spec-rebalance').exists()).toBe(true)
    expect(wrapper.get<HTMLSelectElement>('#edit-spec-rebalance').element.value).toBe('monthly')
    expect(wrapper.get<HTMLSelectElement>('#edit-spec-kind').element.disabled).toBe(true)
    expect(wrapper.text()).toContain('公共策略只读')
    expect(wrapper.text()).not.toContain('尚未覆盖的受控组件')
  })
})
