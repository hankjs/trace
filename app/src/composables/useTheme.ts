import { ref } from 'vue'

export type ThemeMode = 'light' | 'dark' | 'system'

const STORAGE_KEY = 'app-theme'

const THEME_COLORS: Record<'light' | 'dark', string> = {
  light: '#e1e5ec',
  dark: '#262a33',
}

function readStored(): ThemeMode {
  const v = localStorage.getItem(STORAGE_KEY)
  return v === 'light' || v === 'dark' ? v : 'system'
}

const systemDark =
  typeof window.matchMedia === 'function'
    ? window.matchMedia('(prefers-color-scheme: dark)')
    : { matches: false, addEventListener: () => {} }

export const themeMode = ref<ThemeMode>(readStored())

function resolved(): 'light' | 'dark' {
  return themeMode.value === 'system'
    ? systemDark.matches
      ? 'dark'
      : 'light'
    : themeMode.value
}

function apply() {
  const value = resolved()
  document.documentElement.dataset.theme = value
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute('content', THEME_COLORS[value])
}

systemDark.addEventListener('change', apply)
apply()

export function setThemeMode(mode: ThemeMode) {
  themeMode.value = mode
  localStorage.setItem(STORAGE_KEY, mode)
  apply()
}
