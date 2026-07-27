import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

function mediaQuery(matches: boolean) {
  return {
    matches,
    media: '(prefers-color-scheme: dark)',
    onchange: null,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }
}

describe('theme', () => {
  beforeEach(() => {
    vi.resetModules()
    localStorage.clear()
    document.documentElement.classList.remove('dark')
    document.documentElement.style.colorScheme = ''
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('没有手动设置时跟随系统暗色偏好', async () => {
    vi.stubGlobal('matchMedia', vi.fn(() => mediaQuery(true)))

    const { useTheme } = await import('./theme')

    expect(useTheme().isDark.value).toBe(true)
    expect(document.documentElement.classList.contains('dark')).toBe(true)
  })

  it('手动切换后持久化用户选择', async () => {
    vi.stubGlobal('matchMedia', vi.fn(() => mediaQuery(false)))
    const { useTheme } = await import('./theme')
    const { isDark, toggleTheme } = useTheme()

    toggleTheme()

    expect(isDark.value).toBe(true)
    expect(localStorage.getItem('quant_color_mode')).toBe('dark')
    expect(document.documentElement.classList.contains('dark')).toBe(true)
  })
})
