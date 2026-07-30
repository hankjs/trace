import { reactive, readonly } from 'vue'

export interface ConfirmDialogOptions {
  /** 对话框标题，默认「确认操作」 */
  title?: string
  /** danger 用于删除等不可撤销操作，确认按钮使用危险样式 */
  tone?: 'default' | 'danger'
  /** 确认按钮文案，默认「确认」 */
  confirmText?: string
  /** 取消按钮文案，默认「取消」 */
  cancelText?: string
}

export interface ConfirmDialogState {
  open: boolean
  title: string
  message: string
  tone: 'default' | 'danger'
  confirmText: string
  cancelText: string
}

const state = reactive<ConfirmDialogState>({
  open: false,
  title: '确认操作',
  message: '',
  tone: 'default',
  confirmText: '确认',
  cancelText: '取消',
})

let resolver: ((value: boolean) => void) | null = null

/** 供 CoDialog 组件渲染的只读状态 */
export const confirmDialogState = readonly(state)

/**
 * 打开全局确认对话框（由 App.vue 挂载的 CoDialog 渲染）。
 * 用户点击确认 resolve true；点击取消、遮罩或按 Esc resolve false。
 */
export function confirmDialog(message: string, options: ConfirmDialogOptions = {}): Promise<boolean> {
  // 已有未处理的确认时直接视为取消，避免回调悬挂
  resolver?.(false)
  state.open = true
  state.title = options.title ?? '确认操作'
  state.message = message
  state.tone = options.tone ?? 'default'
  state.confirmText = options.confirmText ?? '确认'
  state.cancelText = options.cancelText ?? '取消'
  return new Promise<boolean>((resolve) => {
    resolver = resolve
  })
}

/** 由 CoDialog 在用户作出选择后调用，组件外一般不需要使用 */
export function settleConfirmDialog(result: boolean) {
  if (!state.open) return
  state.open = false
  resolver?.(result)
  resolver = null
}
