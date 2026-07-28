import { flushPromises, mount } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Strategy, StrategyAstNode, StrategyLimits, StrategySpec, StrategyValidationResult } from '../api'
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
  it('creates the acceptance breakout rule through structured fields', async () => {
    const { wrapper, api } = await mountPage([])
    const validate = vi.spyOn(api, 'validateStrategySpec').mockImplementation(async (spec) => validation(spec))
    const create = vi.spyOn(api, 'createStrategy').mockImplementation(async (body) => ({
      ...editableStrategy(),
      name: body.name,
      spec: body.spec,
    }))

    await wrapper.get('button').trigger('click')
    await flushPromises()

    expect(wrapper.get<HTMLInputElement>('#create-spec-breakout-window').element.value).toBe('20')
    expect(wrapper.get<HTMLInputElement>('#create-spec-volume-window').element.value).toBe('20')
    expect(wrapper.get<HTMLInputElement>('#create-spec-volume-ratio').element.value).toBe('1.5')
    expect(wrapper.get<HTMLInputElement>('#create-spec-exit-window').element.value).toBe('10')

    const submit = wrapper.findAll('button').find((button) => button.text() === '校验并创建')
    await submit!.trigger('click')
    await flushPromises()

    expect(validate).toHaveBeenCalledOnce()
    const spec = create.mock.calls[0][0].spec
    expect(spec.universe).toMatchObject({ pool_id: 1, exclude_st: true })
    const entry = spec.entry.condition as StrategyAstNode
    const nativeExit = spec.native_exit.condition as StrategyAstNode
    expect(entry.args?.[0].right).toMatchObject({ op: 'rolling_max', window: 20, shift: 1 })
    expect(entry.args?.[1]).toMatchObject({
      op: 'gt',
      left: { op: 'divide', right: { op: 'rolling_mean', window: 20, shift: 1 } },
      right: { op: 'literal', value: 1.5 },
    })
    expect(nativeExit.args?.[0].right).toMatchObject({ op: 'rolling_min', window: 10, shift: 1 })
  })

  it('saves the normalized current spec in place', async () => {
    const strategy = editableStrategy()
    const { wrapper, api } = await mountPage([strategy])
    const validate = vi.spyOn(api, 'validateStrategySpec').mockImplementation(async (spec) => validation(spec))
    const update = vi.spyOn(api, 'updateStrategy').mockResolvedValue(strategy)

    await wrapper.get<HTMLInputElement>('#edit-spec-exit-window').setValue(8)
    const save = wrapper.findAll('button').find((button) => button.text() === '校验并保存')
    await save!.trigger('click')
    await flushPromises()

    expect(validate).toHaveBeenCalledOnce()
    expect(update).toHaveBeenCalledWith(strategy.id, {
      name: strategy.name,
      spec: expect.objectContaining({
        native_exit: expect.objectContaining({
          condition: expect.objectContaining({
            args: [expect.objectContaining({
              right: expect.objectContaining({ op: 'rolling_min', window: 8 }),
            })],
          }),
        }),
      }),
    })
    expect(wrapper.text()).toContain('当前数据与引擎支持')
    expect(wrapper.text()).toContain('规范化 JSON 只读预览')
  })
})
