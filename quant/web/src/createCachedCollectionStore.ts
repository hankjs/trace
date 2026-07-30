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
  let inflightGeneration = 0
  let generation = 0

  async function load(force = false): Promise<T[]> {
    if (loaded.value && !force) return items.value
    if (inflight && inflightGeneration === generation) return inflight

    generation += 1
    const requestGeneration = generation
    loading.value = true
    error.value = ''
    inflightGeneration = requestGeneration
    inflight = (async () => {
      try {
        const response = await options.request()
        if (requestGeneration !== generation) return items.value
        items.value = options.itemsFrom(response)
        options.onLoaded?.(response)
        loaded.value = true
        return items.value
      } catch (caught) {
        if (requestGeneration === generation) {
          error.value = (caught as Error).message
        }
        throw caught
      } finally {
        if (requestGeneration === generation) {
          loading.value = false
          inflight = null
        }
      }
    })()
    return inflight
  }

  function invalidate() {
    loaded.value = false
    generation += 1
  }

  function reset() {
    generation += 1
    items.value = []
    loading.value = false
    loaded.value = false
    error.value = ''
    inflight = null
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
    reset,
    byId,
    isKnownId,
  }
}
