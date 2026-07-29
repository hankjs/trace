import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import QuSelect from './QuSelect.vue'

type TestValue = string | number | boolean | null
interface TestOption {
  value: TestValue
  label: string
  disabled?: boolean
}

function mountSelect(modelValue: TestValue, options?: TestOption[]) {
  const changes: unknown[] = []
  const wrapper = mount(QuSelect, {
    props: {
      modelValue,
      options: options ?? [
        { value: 'a', label: '选项 A' },
        { value: 'b', label: '选项 B' },
        { value: 'c', label: '选项 C', disabled: true },
      ],
      'onUpdate:modelValue': () => {},
      onChange: (value: unknown) => changes.push(value),
    },
  })
  return { wrapper, changes }
}

const trigger = (wrapper: ReturnType<typeof mount>) => wrapper.get('button[aria-haspopup="listbox"]')

describe('QuSelect', () => {
  it('shows the selected label and opens the listbox on click', async () => {
    const { wrapper } = mountSelect('b')
    expect(trigger(wrapper).text()).toContain('选项 B')
    expect(wrapper.find('[role="listbox"]').exists()).toBe(false)

    await trigger(wrapper).trigger('click')
    expect(wrapper.get('[role="listbox"]').findAll('button[role="option"]')).toHaveLength(3)
    expect(trigger(wrapper).attributes('aria-expanded')).toBe('true')
  })

  it('emits the picked value through v-model and change, then closes', async () => {
    const { wrapper, changes } = mountSelect('a')
    await trigger(wrapper).trigger('click')
    await wrapper.get('button[role="option"][data-value="b"]').trigger('click')

    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual(['b'])
    expect(changes).toEqual(['b'])
    expect(wrapper.find('[role="listbox"]').exists()).toBe(false)
    // 受控组件:父级未回写 modelValue 时仍展示旧值
    expect(trigger(wrapper).text()).toContain('选项 A')
  })

  it('marks the selected option with aria-selected and a check icon', async () => {
    const { wrapper } = mountSelect('a')
    await trigger(wrapper).trigger('click')
    const selected = wrapper.get('button[role="option"][data-value="a"]')
    expect(selected.attributes('aria-selected')).toBe('true')
    expect(selected.find('svg').exists()).toBe(true)
    expect(wrapper.get('button[role="option"][data-value="b"]').attributes('aria-selected')).toBe('false')
  })

  it('supports keyboard navigation and skips disabled options', async () => {
    const { wrapper, changes } = mountSelect(null)
    await trigger(wrapper).trigger('keydown', { key: 'ArrowDown' })
    expect(wrapper.find('[role="listbox"]').exists()).toBe(true)

    await trigger(wrapper).trigger('keydown', { key: 'ArrowDown' })
    // 高亮从 a 移到 b(触发按钮的 aria-activedescendant 指向当前高亮)
    const activeId = trigger(wrapper).attributes('aria-activedescendant')
    expect(activeId).toBeTruthy()
    expect(wrapper.get(`#${activeId}`).attributes('data-value')).toBe('b')

    // 继续向下会跳过禁用的 c,回到 a
    await trigger(wrapper).trigger('keydown', { key: 'ArrowDown' })
    const nextId = trigger(wrapper).attributes('aria-activedescendant')
    expect(wrapper.get(`#${nextId}`).attributes('data-value')).toBe('a')

    await trigger(wrapper).trigger('keydown', { key: 'Escape' })
    expect(wrapper.find('[role="listbox"]').exists()).toBe(false)
    expect(changes).toEqual([])
  })

  it('does not open or select when disabled', async () => {
    const { wrapper } = mountSelect('a')
    await wrapper.setProps({ disabled: true })
    await trigger(wrapper).trigger('click')
    expect(wrapper.find('[role="listbox"]').exists()).toBe(false)
  })

  it('supports boolean values', async () => {
    const { wrapper, changes } = mountSelect(true, [
      { value: true, label: '真 (true)' },
      { value: false, label: '假 (false)' },
    ])
    expect(trigger(wrapper).text()).toContain('真 (true)')
    await trigger(wrapper).trigger('click')
    await wrapper.get('button[role="option"][data-value="false"]').trigger('click')
    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual([false])
    expect(changes).toEqual([false])
  })

  it('shows the placeholder when nothing matches the model', () => {
    const { wrapper } = mountSelect(null)
    expect(trigger(wrapper).text()).toContain('请选择')
  })
})
