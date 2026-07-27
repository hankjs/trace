import { computed, shallowRef } from 'vue'

type ColorMode = 'light' | 'dark'

const STORAGE_KEY = 'quant_color_mode'
const storedMode = localStorage.getItem(STORAGE_KEY)
const systemPreference = typeof window.matchMedia === 'function'
  ? window.matchMedia('(prefers-color-scheme: dark)')
  : null
const initialMode: ColorMode = storedMode === 'light' || storedMode === 'dark'
  ? storedMode
  : systemPreference?.matches ? 'dark' : 'light'
const mode = shallowRef<ColorMode>(initialMode)
let followsSystem = storedMode !== 'light' && storedMode !== 'dark'

function applyTheme() {
  const dark = mode.value === 'dark'
  document.documentElement.classList.toggle('dark', dark)
  document.documentElement.style.colorScheme = dark ? 'dark' : 'light'
}

applyTheme()

systemPreference?.addEventListener('change', (event) => {
  if (!followsSystem) return
  mode.value = event.matches ? 'dark' : 'light'
  applyTheme()
})

export function useTheme() {
  const isDark = computed(() => mode.value === 'dark')

  function toggleTheme() {
    mode.value = mode.value === 'dark' ? 'light' : 'dark'
    followsSystem = false
    localStorage.setItem(STORAGE_KEY, mode.value)
    applyTheme()
  }

  return { isDark, toggleTheme }
}
