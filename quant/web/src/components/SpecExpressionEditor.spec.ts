import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import type { StrategyAstNode } from '../api'
import SpecExpressionEditor from './SpecExpressionEditor.vue'

function field(name: string): StrategyAstNode {
  return { op: 'field', name }
}

function mountEditor(modelValue: StrategyAstNode, expectedType: 'number' | 'bool', crossSectional = false) {
  let emitted: StrategyAstNode | null = null
  const wrapper = mount(SpecExpressionEditor, {
    props: {
      modelValue,
      expectedType,
      crossSectional,
      'onUpdate:modelValue': (value: StrategyAstNode) => {
        emitted = value
      },
    },
  })
  return { wrapper, emitted: () => emitted }
}

describe('SpecExpressionEditor', () => {
  it('filters operators by slot type', () => {
    const { wrapper } = mountEditor(field('close'), 'number')
    const options = wrapper.get('select[aria-label="算子"]').findAll('option').map((option) => option.attributes('value'))
    expect(options).toContain('rolling_mean')
    expect(options).toContain('rolling_std')
    expect(options).toContain('rolling_rank')
    expect(options).toContain('zscore')
    expect(options).toContain('add')
    expect(options).not.toContain('gt')
    expect(options).not.toContain('all')

    const boolEditor = mountEditor({ op: 'gt', left: field('close'), right: field('close') }, 'bool')
    const boolOptions = boolEditor.wrapper.get('select[aria-label="算子"]').findAll('option').map((option) => option.attributes('value'))
    expect(boolOptions).toContain('gt')
    expect(boolOptions).toContain('all')
    expect(boolOptions).toContain('literal')
    expect(boolOptions).not.toContain('rolling_mean')
  })

  it('only offers cross-sectional operators when enabled', () => {
    const plain = mountEditor(field('close'), 'number')
    const plainOptions = plain.wrapper.get('select[aria-label="算子"]').findAll('option').map((option) => option.attributes('value'))
    expect(plainOptions).not.toContain('rank')

    const cross = mountEditor(field('close'), 'number', true)
    const crossOptions = cross.wrapper.get('select[aria-label="算子"]').findAll('option').map((option) => option.attributes('value'))
    expect(crossOptions).toContain('rank')
  })

  it('keeps compatible children when switching operators', async () => {
    const node: StrategyAstNode = {
      op: 'gt',
      left: field('close'),
      right: { op: 'ma', input: field('close'), window: 20 },
    }
    const { wrapper, emitted } = mountEditor(node, 'bool')

    await wrapper.get('select[aria-label="算子"]').setValue('cross_above')

    expect(emitted()).toMatchObject({
      op: 'cross_above',
      left: { op: 'field', name: 'close' },
      right: { op: 'ma', window: 20 },
    })
  })

  it('adds and removes args children for all/any', async () => {
    const node: StrategyAstNode = { op: 'all', args: [{ op: 'literal', value: true }] }
    const { wrapper } = mountEditor(node, 'bool')

    const addButton = wrapper.findAll('button').find((button) => button.text().includes('添加条件'))
    await addButton!.trigger('click')
    expect(node.args).toHaveLength(2)

    const removeButton = wrapper.findAll('button').find((button) => button.attributes('aria-label') === '删除条件 2')
    await removeButton!.trigger('click')
    expect(node.args).toHaveLength(1)
  })

  it('renders bool literal as true/false select and number literal as number input', async () => {
    const boolEditor = mountEditor({ op: 'literal', value: true }, 'bool')
    const boolSelect = boolEditor.wrapper.get('select[aria-label="常量值"]')
    expect((boolSelect.element as HTMLSelectElement).value).toBe('true')
    await boolSelect.setValue('false')
    // literal 通过 defineModel 原地修改
    expect(boolEditor.wrapper.props('modelValue')).toMatchObject({ value: false })

    const numberEditor = mountEditor({ op: 'literal', value: 1.5 }, 'number')
    const input = numberEditor.wrapper.get('input[aria-label="常量值"]')
    expect((input.element as HTMLInputElement).value).toBe('1.5')
  })

  it('renders atr as a three-input node', () => {
    const node: StrategyAstNode = {
      op: 'atr',
      high: field('high'),
      low: field('low'),
      close: field('close'),
      window: 14,
    }
    const { wrapper } = mountEditor(node, 'number')
    expect(wrapper.text()).toContain('最高价序列')
    expect(wrapper.text()).toContain('最低价序列')
    expect(wrapper.text()).toContain('收盘价序列')
    // 三个子槽位各渲染一个字段选择器
    expect(wrapper.findAll('select[aria-label="字段"]')).toHaveLength(3)
  })
})
