import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import { afterEach, describe, expect, it } from 'vitest'
import CoDialog from './CoDialog.vue'
import { confirmDialog, settleConfirmDialog } from '../confirmDialog'

function dialogEl() {
  return document.body.querySelector('[role="alertdialog"]')
}

function buttonByText(text: string) {
  return [...document.body.querySelectorAll<HTMLButtonElement>('[role="alertdialog"] button')]
    .find((el) => el.textContent === text)
}

describe('CoDialog', () => {
  afterEach(() => {
    settleConfirmDialog(false)
    document.body.innerHTML = ''
  })

  it('renders the pending confirm and settles true on confirm click', async () => {
    mount(CoDialog)
    const pending = confirmDialog('确认删除策略？相关信号会一并删除。', {
      title: '删除策略',
      tone: 'danger',
      confirmText: '删除',
    })
    await nextTick()

    expect(dialogEl()?.textContent).toContain('删除策略')
    expect(dialogEl()?.textContent).toContain('确认删除策略？相关信号会一并删除。')

    buttonByText('删除')?.click()
    expect(await pending).toBe(true)
    await nextTick()
    expect(dialogEl()).toBeNull()
  })

  it('settles false on cancel click and on backdrop click', async () => {
    mount(CoDialog)
    const first = confirmDialog('继续？')
    await nextTick()
    buttonByText('取消')?.click()
    expect(await first).toBe(false)

    const second = confirmDialog('继续？')
    await nextTick()
    document.body.querySelector<HTMLButtonElement>('button[aria-label="取消"]')?.click()
    expect(await second).toBe(false)
  })

  it('settles false on Escape key', async () => {
    mount(CoDialog)
    const pending = confirmDialog('继续？')
    await nextTick()
    document.body.querySelector('[role="alertdialog"]')?.parentElement
      ?.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    expect(await pending).toBe(false)
  })

  it('focuses the cancel button first for danger tone', async () => {
    mount(CoDialog)
    confirmDialog('确认删除？', { tone: 'danger' })
    await nextTick()
    await nextTick()
    expect(document.activeElement?.textContent).toBe('取消')
  })

  it('focuses the confirm button first for default tone', async () => {
    mount(CoDialog)
    confirmDialog('继续？', { confirmText: '立即执行' })
    await nextTick()
    await nextTick()
    expect(document.activeElement?.textContent).toBe('立即执行')
  })
})
