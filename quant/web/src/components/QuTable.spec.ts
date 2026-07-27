import { mount } from '@vue/test-utils'
import { h } from 'vue'
import { describe, expect, it } from 'vitest'
import QuTable from './QuTable.vue'
import type { QuTableColumn } from './quTable'

interface Row {
  id: number
  name: string
  score: number | null
}

const columns: QuTableColumn<object>[] = [
  { key: 'name', label: '名称' },
  { key: 'score', label: '评分', align: 'right', format: (value) => value === null ? '暂无' : Number(value).toFixed(2) },
]

describe('QuTable', () => {
  it('根据 data 和 columns 渲染表头、数据与格式化值', () => {
    const wrapper = mount(QuTable, {
      props: {
        data: [{ id: 1, name: '测试股票', score: 1.236 }],
        columns,
        rowKey: 'id',
      },
    })

    expect(wrapper.findAll('th').map((cell) => cell.text())).toEqual(['名称', '评分'])
    expect(wrapper.findAll('td').map((cell) => cell.text())).toEqual(['测试股票', '1.24'])
    expect(wrapper.findAll('td')[1].classes()).toContain('text-right')
  })

  it('允许列插槽覆盖默认内容', () => {
    const wrapper = mount(QuTable, {
      props: {
        data: [{ id: 1, name: '测试股票', score: null }],
        columns,
      },
      slots: {
        'cell-name': ({ row }: { row: object }) => `股票：${(row as Row).name}`,
      },
    })

    expect(wrapper.find('tbody td').text()).toBe('股票：测试股票')
  })

  it('转发行点击并支持在数据行后追加详情行', async () => {
    const data = [{ id: 1, name: '测试股票', score: 1 }]
    const wrapper = mount(QuTable, {
      props: { data, columns, rowKey: 'id' },
      slots: {
        'after-row': ({ row, colspan }: { row: object; colspan: number }) => h('tr', [
          h('td', { colspan }, `详情：${(row as Row).name}`),
        ]),
      },
    })

    await wrapper.find('tbody tr').trigger('click')

    expect(wrapper.emitted('rowClick')?.[0]?.[0]).toEqual(data[0])
    expect(wrapper.findAll('tbody tr')).toHaveLength(2)
    expect(wrapper.findAll('tbody tr')[1].text()).toBe('详情：测试股票')
  })
})
