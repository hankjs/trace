import { defineComponent, ref } from 'vue'
import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import StrategyOverlayFields from './StrategyOverlayFields.vue'
import { DEFAULT_RISK_OVERLAY, DEFAULT_TAKE_PROFIT } from '../researchPlans'

const Host = defineComponent({
  components: { StrategyOverlayFields },
  setup() {
    return {
      risk: ref({ ...DEFAULT_RISK_OVERLAY }),
      takeProfit: ref({ ...DEFAULT_TAKE_PROFIT }),
    }
  },
  template: '<StrategyOverlayFields v-model:risk="risk" v-model:take-profit="takeProfit" />',
})

describe('StrategyOverlayFields', () => {
  it('hides ineffective fields while switches are off and reveals the selected rule', async () => {
    const wrapper = mount(Host)

    expect(wrapper.findAll('input[type="number"]')).toHaveLength(0)
    expect(wrapper.text()).toContain('未设置止盈')

    const [riskSwitch] = wrapper.findAll<HTMLInputElement>('input[type="checkbox"]')
    await riskSwitch.setValue(true)
    expect(wrapper.findAll('input[type="number"]')).toHaveLength(1)
    expect(wrapper.get('input[type="number"]').attributes('max')).toBe('1')
    expect(wrapper.text()).toContain('相对模拟入场价回落 8.00%')

    await wrapper.get('select').setValue('atr_multiple')
    expect(wrapper.findAll('input[type="number"]')).toHaveLength(2)
    expect(wrapper.text()).toContain('14 日 ATR')
  })

  it('uses the backend fixed-percentage limits for take profit', async () => {
    const wrapper = mount(Host)
    const [, takeProfitSwitch] = wrapper.findAll<HTMLInputElement>('input[type="checkbox"]')

    await takeProfitSwitch.setValue(true)

    expect(wrapper.get('input[type="number"]').attributes('min')).toBe('0.001')
    expect(wrapper.get('input[type="number"]').attributes('max')).toBe('1')
  })
})
