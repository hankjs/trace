import { afterEach, describe, expect, it } from 'vitest'
import { confirmDialog, confirmDialogState, settleConfirmDialog } from './confirmDialog'

describe('confirmDialog', () => {
  afterEach(() => {
    settleConfirmDialog(false)
  })

  it('opens with defaults and resolves true on confirm', async () => {
    const pending = confirmDialog('继续执行？')
    expect(confirmDialogState.open).toBe(true)
    expect(confirmDialogState.title).toBe('确认操作')
    expect(confirmDialogState.message).toBe('继续执行？')
    expect(confirmDialogState.tone).toBe('default')
    expect(confirmDialogState.confirmText).toBe('确认')
    expect(confirmDialogState.cancelText).toBe('取消')

    settleConfirmDialog(true)
    expect(await pending).toBe(true)
    expect(confirmDialogState.open).toBe(false)
  })

  it('applies options and resolves false on cancel', async () => {
    const pending = confirmDialog('确认删除？', { title: '删除策略', tone: 'danger', confirmText: '删除' })
    expect(confirmDialogState.title).toBe('删除策略')
    expect(confirmDialogState.tone).toBe('danger')
    expect(confirmDialogState.confirmText).toBe('删除')

    settleConfirmDialog(false)
    expect(await pending).toBe(false)
  })

  it('cancels the previous pending confirm when a new one opens', async () => {
    const first = confirmDialog('第一个')
    const second = confirmDialog('第二个')
    expect(confirmDialogState.message).toBe('第二个')
    expect(await first).toBe(false)

    settleConfirmDialog(true)
    expect(await second).toBe(true)
  })

  it('ignores settle when no confirm is open', () => {
    expect(confirmDialogState.open).toBe(false)
    expect(() => settleConfirmDialog(true)).not.toThrow()
  })
})
