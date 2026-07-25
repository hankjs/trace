import { describe, expect, it } from 'vitest'
import { useAsyncAction } from './useAsyncAction'

describe('useAsyncAction', () => {
  it('normalizes success and failure state', async () => {
    const action = useAsyncAction()
    await action.run(async () => 3, { success: (value) => `完成 ${value}` })
    expect(action.notice.value).toBe('完成 3')
    expect(action.error.value).toBe('')
    expect(action.busy.value).toBe(false)

    await action.run(async () => { throw new Error('失败') })
    expect(action.error.value).toBe('失败')
    expect(action.notice.value).toBe('')
    expect(action.busy.value).toBe(false)
  })
})
