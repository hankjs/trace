import { mount, type VueWrapper } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import QuDatePicker from './QuDatePicker.vue'
import { localDateISO } from '../format'

function mountPicker(modelValue: string, clearable = true) {
  const changes: string[] = []
  const wrapper = mount(QuDatePicker, {
    props: {
      modelValue,
      clearable,
      'onUpdate:modelValue': () => {},
      onChange: (value: string) => changes.push(value),
    },
  })
  return { wrapper, changes }
}

const trigger = (wrapper: VueWrapper) => wrapper.get('button[aria-haspopup="dialog"]')

describe('QuDatePicker', () => {
  it('shows the selected date or the placeholder when empty', () => {
    expect(trigger(mountPicker('2024-01-15').wrapper).text()).toContain('2024-01-15')
    expect(trigger(mountPicker('').wrapper).text()).toContain('选择日期')
  })

  it('opens the calendar on click and picks a day', async () => {
    const { wrapper, changes } = mountPicker('2024-01-15')
    await trigger(wrapper).trigger('click')

    const panel = wrapper.get('[role="dialog"]')
    expect(trigger(wrapper).attributes('aria-expanded')).toBe('true')
    expect(panel.text()).toContain('2024 年 1 月')
    // 星期表头(周一开头)
    expect(panel.text()).toContain('一')
    // 选中日 aria-pressed
    expect(wrapper.get('button[data-date="2024-01-15"]').attributes('aria-pressed')).toBe('true')

    await wrapper.get('button[data-date="2024-01-20"]').trigger('click')
    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual(['2024-01-20'])
    expect(changes).toEqual(['2024-01-20'])
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false)
  })

  it('switches months with the arrow buttons', async () => {
    const { wrapper } = mountPicker('2024-01-15')
    await trigger(wrapper).trigger('click')

    await wrapper.get('button[aria-label="下个月"]').trigger('click')
    expect(wrapper.get('[role="dialog"]').text()).toContain('2024 年 2 月')
    // 目标日期在 2 月网格里不存在,先回到 1 月
    await wrapper.get('button[aria-label="上个月"]').trigger('click')
    expect(wrapper.get('[role="dialog"]').text()).toContain('2024 年 1 月')
  })

  it('marks today and picks it via the shortcut', async () => {
    const { wrapper, changes } = mountPicker('')
    await trigger(wrapper).trigger('click')
    expect(wrapper.get(`button[data-date="${localDateISO()}"]`).attributes('aria-current')).toBe('date')

    const todayButton = wrapper.findAll('button').find((button) => button.text() === '今天')
    await todayButton!.trigger('click')
    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual([localDateISO()])
    expect(changes).toEqual([localDateISO()])
  })

  it('clears the value when clearable and hides the clear button otherwise', async () => {
    const { wrapper, changes } = mountPicker('2024-01-15')
    await trigger(wrapper).trigger('click')
    const clearButton = wrapper.findAll('button').find((button) => button.text() === '清除')
    expect(clearButton).toBeDefined()
    await clearButton!.trigger('click')
    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual([''])
    expect(changes).toEqual([''])

    const locked = mountPicker('2024-01-15', false)
    await trigger(locked.wrapper).trigger('click')
    expect(locked.wrapper.findAll('button').some((button) => button.text() === '清除')).toBe(false)
  })

  it('closes on Escape and does not open when disabled', async () => {
    const { wrapper } = mountPicker('2024-01-15')
    await trigger(wrapper).trigger('keydown', { key: 'ArrowDown' })
    expect(wrapper.find('[role="dialog"]').exists()).toBe(true)
    await trigger(wrapper).trigger('keydown', { key: 'Escape' })
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false)

    await wrapper.setProps({ disabled: true })
    await trigger(wrapper).trigger('click')
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false)
  })
})
