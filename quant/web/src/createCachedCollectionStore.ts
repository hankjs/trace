import { readonly, ref, shallowReadonly, shallowRef, type ShallowRef } from 'vue'

interface CollectionItem {
  id: number
}

interface CachedCollectionOptions<T extends CollectionItem, Response> {
  request: () => Promise<Response>
  itemsFrom: (response: Response) => T[]
  onLoaded?: (response: Response) => void
}

export function createCachedCollectionStore<T extends CollectionItem, Response>(
  options: CachedCollectionOptions<T, Response>
) {
  const items = shallowRef<T[]>([])
  const loading = ref(false)
  const loaded = ref(false)
  const error = ref('')
  let inflight: Promise<T[]> | null = null

  async function load(force = false): Promise<T[]> {
    if (loaded.value && !force) return items.value
    if (inflight) return inflight
    loading.value = true
    error.value = ''
    inflight = (async () => {
      try {
        const response = await options.request()
        items.value = options.itemsFrom(response)
        options.onLoaded?.(response)
        loaded.value = true
        return items.value
      } catch (caught) {
        error.value = (caught as Error).message
        throw caught
      } finally {
        loading.value = false
        inflight = null
      }
    })()
    return inflight
  }

  function invalidate() {
    loaded.value = false
  }

  function byId(id: number | null | undefined): T | null {
    if (id === null || id === undefined) return null
    return items.value.find((item) => item.id === id) ?? null
  }

  function isKnownId(id: unknown): id is number {
    return typeof id === 'number' && items.value.some((item) => item.id === id)
  }

  return {
    items: shallowReadonly(items) as Readonly<ShallowRef<T[]>>,
    loading: readonly(loading),
    loaded: readonly(loaded),
    error: readonly(error),
    load,
    invalidate,
    byId,
    isKnownId,
  }
}
