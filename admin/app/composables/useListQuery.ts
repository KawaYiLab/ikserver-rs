import type { LocationQuery } from 'vue-router'

/** 筛选值统一用字符串（能原样写进 URL）；`'all'` 与空串表示不筛选。 */
export function optional<T extends string = string>(value: string): T | undefined {
  const trimmed = value.trim()
  return trimmed === '' || trimmed === 'all' ? undefined : trimmed as T
}

/** `'yes'` / `'no'` / `'all'` 三态筛选转成布尔。 */
export function optionalBool(value: string, yes = 'yes'): boolean | undefined {
  return value === 'all' || value === '' ? undefined : value === yes
}

function readQuery(query: LocationQuery, key: string): string | undefined {
  const value = query[key]
  return typeof value === 'string' ? value : undefined
}

/** 把一组值写回 URL：等于默认值的参数不写，其他参数原样保留。 */
function patchQuery(query: LocationQuery, values: Record<string, string>, defaults: Record<string, string>) {
  const next: LocationQuery = { ...query }
  for (const [key, value] of Object.entries(values)) {
    if (value === defaults[key]) delete next[key]
    else next[key] = value
  }
  return next
}

function sameQuery(a: LocationQuery, b: LocationQuery) {
  const keys = new Set([...Object.keys(a), ...Object.keys(b)])
  for (const key of keys) {
    if (String(a[key] ?? '') !== String(b[key] ?? '')) return false
  }
  return true
}

/** 一个和 URL 查询参数双向同步的值（页内标签页等）。默认值不写进 URL。 */
export function useQueryParam<T extends string>(key: string, fallback: T) {
  const route = useRoute()
  const router = useRouter()
  return computed<T>({
    get: () => (readQuery(route.query, key) as T | undefined) ?? fallback,
    set: (value) => {
      const query = patchQuery(route.query, { [key]: value }, { [key]: fallback })
      if (!sameQuery(query, route.query)) router.replace({ query })
    },
  })
}

export interface ListQueryOptions<F extends Record<string, string>, R> {
  /** 筛选项及默认值。 */
  filters: F
  /** 文本筛选项：输入停顿 400ms 后才生效。 */
  text?: (keyof F & string)[]
  /** 不算「筛选」的参数（如页内标签页）：不影响「有筛选」的判断，「清除筛选」也不重置它们。 */
  persistent?: (keyof F & string)[]
  /** 页码在 URL 里的参数名，同一页有多个列表时用来区分。 */
  pageKey?: string
  fetch: (filters: Readonly<F>, page: number) => Promise<R>
}

/**
 * 列表页的筛选、分页与加载。
 *
 * 筛选和页码与 URL 查询参数双向同步：刷新、前进后退、分享链接都能还原；
 * 从别处带着参数跳过来（如仪表盘「待处理举报」）也会直接生效。改任何筛选都回到第 1 页。
 */
export function useListQuery<F extends Record<string, string>, R>(options: ListQueryOptions<F, R>) {
  const route = useRoute()
  const router = useRouter()
  const { failure } = useNotify()
  const pageKey = options.pageKey ?? 'page'
  const keys = Object.keys(options.filters) as (keyof F & string)[]
  const text = new Set<string>(options.text ?? [])
  const defaults: Record<string, string> = { ...options.filters, [pageKey]: '1' }

  function fromUrl() {
    const filters = { ...options.filters }
    for (const key of keys) {
      const value = readQuery(route.query, key)
      if (value !== undefined) (filters as Record<string, string>)[key] = value
    }
    const page = Number.parseInt(readQuery(route.query, pageKey) ?? '1', 10)
    return { filters, page: Number.isFinite(page) && page > 0 ? page : 1 }
  }

  const initial = fromUrl()
  /** 输入框绑定的筛选值；文本项要等防抖结束才进 URL。 */
  const filters = reactive({ ...initial.filters }) as F
  /** 当前生效（已写进 URL、用于加载）的筛选值与页码。 */
  const applied = shallowRef(initial)
  const result = shallowRef<R | null>(null)
  const loading = ref(false)
  const failed = ref(false)
  /** 勾选的行 ID；每次重新加载清空。 */
  const selection = ref<Set<string>>(new Set())
  let seq = 0

  async function load() {
    const current = ++seq
    loading.value = true
    try {
      const data = await options.fetch(applied.value.filters, applied.value.page)
      if (current !== seq) return
      result.value = data
      failed.value = false
      selection.value = new Set()
    }
    catch (error) {
      if (current !== seq) return
      failed.value = true
      failure(error)
    }
    finally {
      if (current === seq) loading.value = false
    }
  }

  function push(values: Record<string, string>) {
    const query = patchQuery(route.query, values, defaults)
    if (!sameQuery(query, route.query)) router.replace({ query })
  }

  /** 把输入框里的筛选写进 URL（页码回到 1）。 */
  function commit() {
    const changed = keys.some(key => filters[key] !== applied.value.filters[key])
    if (changed) push({ ...filters, [pageKey]: '1' })
  }

  function setPage(page: number) {
    push({ ...applied.value.filters, [pageKey]: String(page) })
  }

  const persistent = new Set<string>(options.persistent ?? [])
  const resettable = keys.filter(key => !persistent.has(key))

  /** 恢复所有筛选的默认值（`persistent` 的除外）。 */
  function reset() {
    const next = { ...applied.value.filters }
    for (const key of resettable) next[key] = options.filters[key]
    Object.assign(filters, next)
    push({ ...next, [pageKey]: '1' })
  }

  /** 有筛选条件生效。 */
  const active = computed(() => resettable.some(key => applied.value.filters[key] !== options.filters[key]))

  let timer: ReturnType<typeof setTimeout> | undefined
  for (const key of keys) {
    watch(() => filters[key], () => {
      clearTimeout(timer)
      if (text.has(key)) timer = setTimeout(commit, 400)
      else commit()
    })
  }

  // URL 是唯一的事实来源：参数变了（自己写的或从别处跳来的）就同步输入框并重新加载。
  // 离开本页时路由先变、组件后卸载，所以只认本页路径。
  const path = route.path
  watch(() => route.query, () => {
    if (route.path !== path) return
    const next = fromUrl()
    const same = next.page === applied.value.page
      && keys.every(key => next.filters[key] === applied.value.filters[key])
    if (same) return
    applied.value = next
    for (const key of keys) {
      const value = next.filters[key] as F[typeof key]
      // 文本框只在内容实质不同时才覆盖，免得吃掉用户正在输入的空格。
      if (!text.has(key) || String(filters[key]).trim() !== String(value).trim()) filters[key] = value
    }
    load()
  })
  onMounted(load)
  onBeforeUnmount(() => clearTimeout(timer))

  return {
    filters,
    applied,
    page: computed(() => applied.value.page),
    result,
    loading,
    failed,
    active,
    selection,
    refresh: load,
    setPage,
    reset,
  }
}

/** 勾选辅助：全选当前页、单行切换。 */
export function useSelection(selection: Ref<Set<string>>, ids: () => string[]) {
  const all = computed<boolean | 'indeterminate'>(() => {
    const list = ids()
    if (!list.length) return false
    const picked = list.filter(id => selection.value.has(id)).length
    return picked === 0 ? false : picked === list.length ? true : 'indeterminate'
  })

  function toggleAll(on: boolean | 'indeterminate') {
    selection.value = on === true ? new Set(ids()) : new Set()
  }

  function toggle(id: string, on: boolean | 'indeterminate') {
    const next = new Set(selection.value)
    if (on === true) next.add(id)
    else next.delete(id)
    selection.value = next
  }

  function clear() {
    selection.value = new Set()
  }

  return { all, toggleAll, toggle, clear, has: (id: string) => selection.value.has(id) }
}
