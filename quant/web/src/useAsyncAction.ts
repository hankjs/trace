import { ref } from 'vue'

interface AsyncActionOptions<T> {
  success?: string | ((result: T) => string)
}

export function useAsyncAction() {
  const busy = ref(false)
  const error = ref('')
  const notice = ref('')

  function clear() {
    error.value = ''
    notice.value = ''
  }

  function fail(message: string) {
    error.value = message
    notice.value = ''
  }

  async function run<T>(task: () => Promise<T>, options: AsyncActionOptions<T> = {}): Promise<T | undefined> {
    busy.value = true
    clear()
    try {
      const result = await task()
      if (options.success) {
        notice.value = typeof options.success === 'function' ? options.success(result) : options.success
      }
      return result
    } catch (caught) {
      fail(caught instanceof Error ? caught.message : String(caught))
      return undefined
    } finally {
      busy.value = false
    }
  }

  return { busy, error, notice, clear, fail, run }
}
