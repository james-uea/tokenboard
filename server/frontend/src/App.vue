<script setup>
import { ref, computed, onMounted, onUnmounted, nextTick, watch } from 'vue'
import Chart from 'chart.js/auto'

const NUMERIC_COLUMNS = new Set([
  "total_tokens", "total_cost", "input_tokens", "output_tokens",
  "cache_read_tokens", "cache_write_tokens", "cache_rate",
])

const PERIODS = [
  { key: "all",   label: "All time" },
  { key: "day",   label: "Today" },
  { key: "week",  label: "Week" },
  { key: "month", label: "Month" },
  { key: "year",  label: "Year" },
]

const LEADERBOARD_SKELETON_ROWS = 7
const HEATMAP_SKELETON_WEEKS = 10
const HEATMAP_VISIBLE_DAYS = 63
const STAT_SKELETON_CARDS = 6
const CHART_SKELETON_CARDS = 6
const MODEL_SKELETON_ROWS = 4
const CHART_DIFF_FIELDS = [
  "total_tokens",
  "total_cost",
  "input_tokens",
  "output_tokens",
  "cache_read_tokens",
  "cache_write_tokens",
  "reasoning_tokens",
]

const route = ref({ name: "leaderboard", username: null })
const leaderboard = ref([])
const leaderboardLoading = ref(true)
const leaderboardError = ref("")
const leaderboardPeriod = ref("all")
const leaderboardAppliedSearch = ref("")
const searchOpen = ref(false)
const searchQuery = ref("")
const searchInput = ref(null)
const sortKey = ref("total_tokens")
const sortDirection = ref("desc")

const badges = ref([])
const badgesLoading = ref(true)
const badgesError = ref("")
const userBadges = ref({})

const authLoading = ref(true)
const authUser = ref(null)
const accountPanelOpen = ref(false)
const accountStats = ref(null)
const accountStatsLoading = ref(false)
const accountStatsError = ref("")
const accountStatsUsername = ref("")

const userStats = ref(null)
const statsLoading = ref(false)
const statsError = ref("")

const ghContributions = ref([])
const ghContribsLoading = ref(false)
const ghHoveredDay = ref(null)      // { date, count, level, tokenLevel, tokenCount } or null
const ghHoveredPos = ref(null)      // { wi, di } — week + day index of hovered cell, for cross-grid sync
const ghDetail = ref(null)           // { date, repos: [...] } or null
const ghDetailLoading = ref(false)
const ghTokenMap = ref({})           // date → { total_tokens } from stats timeline
const ghHeatmapLabelHover = ref(false)

// ── Model distribution table sort state ──────────────
const modelSortKey = ref("total_tokens")
const modelSortDir = ref("desc")

let timelineChart = null
let tokenMixChart = null
let modelMixChart = null
let diffChart = null
let providerSpendChart = null
let agentBreakdownChart = null
let activeProfileRequestId = 0
let leaderboardRequestId = 0
let leaderboardSearchTimer = null
let leaderboardAbortController = null

const profilePreloadCache = new Map()

const timelineEmpty = ref(false)
const tokenMixEmpty = ref(false)
const modelMixEmpty = ref(false)
const diffEmpty = ref(false)
const providerSpendEmpty = ref(false)
const agentBreakdownEmpty = ref(false)

// ── Computed ─────────────────────────────────────────

const sortedLeaderboard = computed(() => {
  return [...leaderboard.value].sort((left, right) => {
    const direction = sortDirection.value === "asc" ? 1 : -1
    const a = getSortValue(left, sortKey.value)
    const b = getSortValue(right, sortKey.value)
    if (a < b) return -1 * direction
    if (a > b) return 1 * direction
    return String(left.username).localeCompare(String(right.username))
  })
})

const sortedModelBreakdown = computed(() => {
  const models = Array.isArray(userStats.value?.models) ? userStats.value.models : []
  if (models.length === 0) return []
  return [...models].sort((left, right) => {
    const direction = modelSortDir.value === "desc" ? -1 : 1
    const key = modelSortKey.value
    const a = NUMERIC_COLUMNS.has(key) ? Number(left?.[key] ?? 0) : String(left?.[key] ?? "")
    const b = NUMERIC_COLUMNS.has(key) ? Number(right?.[key] ?? 0) : String(right?.[key] ?? "")
    if (a < b) return -1 * direction
    if (a > b) return 1 * direction
    return String(left.model ?? "").localeCompare(String(right.model ?? ""))
  })
})

const SORT_LABELS = {
  top_model: "Model",
  input_tokens: "input tokens",
  output_tokens: "output tokens",
  cache_read_tokens: "cache reads",
  cache_write_tokens: "cache writes",
  total_tokens: "total tokens",
  total_cost: "cost",
}

const leaderboardSortLabel = computed(() => {
  const label = SORT_LABELS[sortKey.value] ?? sortKey.value
  if (sortKey.value === "top_model") {
    const dir = sortDirection.value === "desc" ? "Z–A" : "A–Z"
    return `Ranked by ${label}, ${dir}`
  }
  const dir = sortDirection.value === "desc" ? "highest first" : "lowest first"
  return `Ranked by ${label}, ${dir}`
})

const authAvatarUsername = computed(() => authUser.value?.github_login || authUser.value?.username || "")
const accountProfileUsername = computed(() => authUser.value?.username || authUser.value?.github_login || "")
const accountTopModel = computed(() => {
  const models = Array.isArray(accountStats.value?.models) ? accountStats.value.models : []
  return shortModel(models[0]?.model)
})
const normalizedSearchQuery = computed(() => normalizeSearchQuery(searchQuery.value))
const hasActiveLeaderboardSearch = computed(() => leaderboardAppliedSearch.value.length > 0)

// ── Helpers ──────────────────────────────────────────

function getSortValue(entry, key) {
  return NUMERIC_COLUMNS.has(key) ? Number(entry?.[key] ?? 0) : String(entry?.[key] ?? "")
}

function normalizeSearchQuery(value) {
  return String(value || "").trim().slice(0, 64)
}

function applyRouteFromLocation() {
  const match = window.location.pathname.match(/^\/users\/([^/]+)$/)
  if (match) {
    route.value = { name: "user", username: decodeURIComponent(match[1]) }
  } else {
    route.value = { name: "leaderboard", username: null }
  }
}

function sortIndicator(col) {
  if (sortKey.value !== col) return ""
  return sortDirection.value === "desc" ? "▾" : "▴"
}

function rankClass(index) {
  return index < 3 ? "rank podium" : "rank"
}

function formatInteger(value) {
  return Number(value ?? 0).toLocaleString()
}

function formatCost(value) {
  return `$${Number(value ?? 0).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`
}

function hasProvidersWithCost(stats = userStats.value) {
  const models = stats?.models
  if (!Array.isArray(models) || models.length === 0) return false
  return models.some((m) => Number(m.total_cost ?? 0) > 0 && m.provider)
}

function formatCacheRate(value) {
  const num = Number(value ?? 0)
  return `${num.toFixed(1)}%`
}

function rateClass(value) {
  const num = Number(value ?? 0)
  if (num >= 75) return "rate-high"
  if (num >= 40) return "rate-mid"
  if (num > 0) return "rate-low"
  return ""
}

// Per-model distinct color palette (10 colors, cycling)
const MODEL_COLORS = [
  "#06d6d0", "#818cf8", "#34d399", "#f59e0b", "#f472b6",
  "#a78bfa", "#fb923c", "#38bdf8", "#fbbf24", "#f87171",
]
function modelColor(name) {
  if (!name) return MODEL_COLORS[0]
  let hash = 0
  for (let i = 0; i < name.length; i++) {
    hash = name.charCodeAt(i) + ((hash << 5) - hash)
  }
  return MODEL_COLORS[Math.abs(hash) % MODEL_COLORS.length]
}

function dateLabel(value) {
  if (!value) return "—"
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? String(value)
    : date.toLocaleDateString(undefined, { month: "short", day: "numeric" })
}

function avatarUrl(username) {
  return `/api/avatar/${encodeURIComponent(username)}`
}

function shortModel(name) {
  if (!name) return "—"
  return name.length > 24 ? name.slice(0, 22) + "…" : name
}

function getPrimaryBadge(username) {
  const list = userBadges.value[username]
  if (!list || list.length === 0) return null
  return list[0]
}

function getUserBadges(username) {
  return userBadges.value[username] || []
}

const DAY_LABELS = ["S", "M", "T", "W", "T", "F", "S"]

function dateKey(value) {
  if (!value) return ""
  return String(value).slice(0, 10)
}

function browserCalendarDateKey(date = new Date()) {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, "0")
  const day = String(date.getDate()).padStart(2, "0")
  return `${year}-${month}-${day}`
}

function hasTimelineData(entry) {
  if (!entry || typeof entry !== "object") return false
  if (Object.prototype.hasOwnProperty.call(entry, "has_data")) {
    return entry.has_data === true
  }
  return CHART_DIFF_FIELDS.some((field) => Number(entry?.[field] ?? 0) > 0)
}

function isPlottableTimelineEntry(entry, todayKey = browserCalendarDateKey()) {
  const key = dateKey(entry?.date)
  return !!key && (key <= todayKey || hasTimelineData(entry))
}

function latestDataDateKey(timeline) {
  const dates = Array.isArray(timeline)
    ? timeline.map((entry) => hasTimelineData(entry) ? dateKey(entry?.date) : "").filter(Boolean).sort()
    : []
  return dates.length > 0 ? dates[dates.length - 1] : ""
}

function latestPlottableDateKey(timeline, todayKey = browserCalendarDateKey()) {
  const latestDataDate = latestDataDateKey(timeline)
  return latestDataDate > todayKey ? latestDataDate : todayKey
}

function dateRangeFromContributions(contributions = ghContributions.value) {
  const dates = Array.isArray(contributions)
    ? contributions.map((entry) => dateKey(entry?.date)).filter(Boolean).sort()
    : []
  if (dates.length === 0) return null
  return { start: dates[0], end: dates[dates.length - 1] }
}

function inDateRange(value, range) {
  const key = dateKey(value)
  return !!key && (!range || (key >= range.start && key <= range.end))
}

function addDateDays(key, days) {
  const [year, month, day] = key.split("-").map(Number)
  return new Date(Date.UTC(year, month - 1, day + days)).toISOString().slice(0, 10)
}

function emptyTimelineEntry(key, runningTotalTokens = 0) {
  return {
    date: `${key}T00:00:00.000Z`,
    has_data: false,
    total_tokens: 0,
    total_cost: 0,
    input_tokens: 0,
    output_tokens: 0,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    running_total_tokens: runningTotalTokens,
  }
}

function timelineForHeatmapWindow(timeline) {
  if (!Array.isArray(timeline) || timeline.length === 0) return []

  const todayKey = browserCalendarDateKey()
  const range = dateRangeFromContributions()
  if (range) {
    const sorted = [...timeline].sort((left, right) => dateKey(left?.date).localeCompare(dateKey(right?.date)))
    const byDate = new Map(sorted.map((entry) => [dateKey(entry?.date), entry]).filter(([key]) => key))
    const latestPlottableDate = latestPlottableDateKey(sorted, todayKey)
    const endDate = range.end < latestPlottableDate ? range.end : latestPlottableDate
    let runningTotalTokens = 0
    for (const entry of sorted) {
      const key = dateKey(entry?.date)
      if (!key || key >= range.start) break
      runningTotalTokens = Number(entry.running_total_tokens ?? runningTotalTokens + Number(entry.total_tokens ?? 0))
    }

    const window = []
    for (let key = range.start; key <= endDate; key = addDateDays(key, 1)) {
      const entry = byDate.get(key)
      if (entry) {
        runningTotalTokens = Number(entry.running_total_tokens ?? runningTotalTokens + Number(entry.total_tokens ?? 0))
        if (isPlottableTimelineEntry(entry, todayKey)) window.push(entry)
      } else if (key <= todayKey) {
        window.push(emptyTimelineEntry(key, runningTotalTokens))
      }
    }
    return window
  }

  return timeline.filter((entry) => isPlottableTimelineEntry(entry, todayKey)).slice(-HEATMAP_VISIBLE_DAYS)
}

function dayOverDayForTimelineWindow(dayOverDay, timelineWindow) {
  if (!Array.isArray(dayOverDay) || dayOverDay.length === 0 || timelineWindow.length === 0) return []

  const byDate = new Map(dayOverDay.map((entry) => [dateKey(entry?.date), entry]).filter(([key]) => key))
  const visible = []
  for (let i = 0; i < timelineWindow.length; i++) {
    const curr = timelineWindow[i]
    const currDate = dateKey(curr?.date)
    if (!currDate) continue

    const existing = byDate.get(currDate)
    if (existing) {
      visible.push(existing)
      continue
    }

    if (i === 0) continue

    const prev = timelineWindow[i - 1]
    const delta = {}
    for (const field of CHART_DIFF_FIELDS) {
      delta[`delta_${field}`] = Number(curr?.[field] ?? 0) - Number(prev?.[field] ?? 0)
    }
    const prevTotal = Number(prev?.total_tokens ?? 0)
    visible.push({
      date: curr.date,
      prev_date: prev.date,
      ...delta,
      percent_change: prevTotal > 0
        ? Number(((delta.delta_total_tokens / prevTotal) * 100).toFixed(1))
        : 0,
    })
  }

  return visible
}

const ghWeeks = computed(() => {
  const days = ghMerged.value
  if (days.length === 0) return []

  // Determine day-of-week of the first entry (0=Sun)
  const firstDate = new Date(days[0].date + "T00:00:00")
  const startDow = firstDate.getDay()

  // Pad front with nulls so the grid aligns to Sunday
  const padded = [...Array(startDow).fill(null), ...days]

  // Group into columns of 7 (Sun→Sat top to bottom)
  const weeks = []
  for (let i = 0; i < padded.length; i += 7) {
    const col = padded.slice(i, i + 7)
    // Pad short final column to 7 rows
    while (col.length < 7) col.push(null)
    weeks.push(col)
  }

  return weeks
})

// ── Token usage overlay ──────────────────────────

// Merge GitHub contributions with token usage data
const ghMerged = computed(() => {
  const tokenMap = ghTokenMap.value
  if (!ghContributions.value.length) return []
  const merged = ghContributions.value.map((d) => {
    const td = tokenMap[d.date]
    return {
      ...d,
      tokenCount: td ? Number(td.total_tokens ?? 0) : 0,
      tokenLevel: 0,
    }
  })
  // Compute token levels relative to user's own max
  const maxTokens = Math.max(...merged.map((d) => d.tokenCount), 1)
  for (const day of merged) {
    if (day.tokenCount === 0) continue
    const ratio = day.tokenCount / maxTokens
    if (ratio < 0.2) day.tokenLevel = 1
    else if (ratio < 0.4) day.tokenLevel = 2
    else if (ratio < 0.65) day.tokenLevel = 3
    else day.tokenLevel = 4
  }
  return merged
})

// Build token map from stats timeline once stats load
function buildTokenMap(timeline) {
  const map = {}
  const todayKey = browserCalendarDateKey()
  if (Array.isArray(timeline)) {
    for (const entry of timeline) {
      const dateOnly = String(entry.date || '').slice(0, 10)
      if (!dateOnly) continue
      if (!isPlottableTimelineEntry(entry, todayKey)) continue
      map[dateOnly] = { total_tokens: Number(entry.total_tokens ?? 0) }
    }
  }
  ghTokenMap.value = map
}

function profileCacheKey(username) {
  return String(username || "").trim().toLowerCase()
}

function getProfileCacheEntry(username) {
  const key = profileCacheKey(username)
  if (!key) return null
  if (!profilePreloadCache.has(key)) {
    profilePreloadCache.set(key, {
      stats: null,
      statsPromise: null,
      contributions: null,
      contributionsPromise: null,
    })
  }
  return profilePreloadCache.get(key)
}

async function fetchUserStatsPayload(username) {
  const response = await fetch(`/api/stats/${encodeURIComponent(username)}`, { cache: "no-cache" })
  if (!response.ok) {
    throw new Error(response.status === 404 ? "User not found." : `Stats request failed (${response.status})`)
  }
  return response.json()
}

async function fetchGhContributionsPayload(username) {
  const response = await fetch(`/api/github-contributions/${encodeURIComponent(username)}`, { cache: "no-cache" })
  if (!response.ok) throw new Error(`GitHub contributions request failed (${response.status})`)
  const payload = await response.json()
  return Array.isArray(payload.contributions) ? payload.contributions : []
}

function ensureProfileStats(username) {
  const entry = getProfileCacheEntry(username)
  if (!entry) return Promise.reject(new Error("Missing username."))
  if (entry.stats) return Promise.resolve(entry.stats)
  if (!entry.statsPromise) {
    entry.statsPromise = fetchUserStatsPayload(username)
      .then((stats) => {
        entry.stats = stats
        return stats
      })
      .finally(() => {
        entry.statsPromise = null
      })
  }
  return entry.statsPromise
}

function ensureProfileContributions(username) {
  const entry = getProfileCacheEntry(username)
  if (!entry) return Promise.reject(new Error("Missing username."))
  if (Array.isArray(entry.contributions)) return Promise.resolve(entry.contributions)
  if (!entry.contributionsPromise) {
    entry.contributionsPromise = fetchGhContributionsPayload(username)
      .then((contributions) => {
        entry.contributions = contributions
        return contributions
      })
      .finally(() => {
        entry.contributionsPromise = null
      })
  }
  return entry.contributionsPromise
}

function preloadProfile(username) {
  if (!username) return
  ensureProfileStats(username).catch(() => {})
  ensureProfileContributions(username).catch(() => {})
}

function applyUserStatsPayload(stats) {
  userStats.value = stats

  const tl = Array.isArray(stats.timeline) ? stats.timeline : []
  const md = Array.isArray(stats.models) ? stats.models : []
  const td = [
    Number(stats.input_tokens ?? 0),
    Number(stats.output_tokens ?? 0),
    Number(stats.cache_read_tokens ?? 0),
    Number(stats.cache_write_tokens ?? 0),
    Number(stats.reasoning_tokens ?? 0),
  ]

  timelineEmpty.value = !tl.some((entry) => isPlottableTimelineEntry(entry))
  tokenMixEmpty.value = !td.some((v) => v > 0)
  modelMixEmpty.value = md.length === 0
  diffEmpty.value = !(stats.diffs?.day_over_day?.length > 0)
  providerSpendEmpty.value = !hasProvidersWithCost(stats)
  agentBreakdownEmpty.value = !(stats.client_breakdown?.length > 0)

  buildTokenMap(tl)
}

function isCurrentProfileRequest(username, requestId) {
  return (
    requestId === activeProfileRequestId &&
    route.value.name === "user" &&
    route.value.username === username
  )
}

function shortDay(dateStr) {
  return new Date(dateStr + "T00:00:00").toLocaleDateString(undefined, { weekday: "short" }).slice(0, 1)
}

function truncate(str, max) {
  if (!str) return ""
  return str.length > max ? str.slice(0, max - 1) + "…" : str
}

function isCellSynced(wi, di) {
  return ghHoveredPos.value && ghHoveredPos.value.wi === wi && ghHoveredPos.value.di === di
}

async function onGhCellEnter(day, wi, di) {
  if (!day) {
    ghHoveredDay.value = null
    ghHoveredPos.value = null
    ghDetail.value = null
    return
  }
  ghHoveredDay.value = day
  ghHoveredPos.value = { wi, di }
  ghDetailLoading.value = true
  ghDetail.value = null
  try {
    const response = await fetch(
      `/api/github-daily-detail/${encodeURIComponent(userStats.value.username)}/${day.date}`,
      { cache: "no-cache" }
    )
    if (!response.ok) throw new Error(`Daily detail failed (${response.status})`)
    ghDetail.value = await response.json()
  } catch (error) {
    console.error("Failed to load daily detail:", error)
    ghDetail.value = { date: day.date, repos: [], error: true }
  } finally {
    ghDetailLoading.value = false
  }
}

function onGhCellLeave() {
  ghHoveredDay.value = null
  ghHoveredPos.value = null
  ghDetail.value = null
}

function onHeatmapLabelEnter() {
  ghHeatmapLabelHover.value = true
}

function onHeatmapLabelLeave() {
  ghHeatmapLabelHover.value = false
}

// ── Data Fetching ────────────────────────────────────

async function loadLeaderboard(period = leaderboardPeriod.value, query = normalizedSearchQuery.value) {
  const requestId = leaderboardRequestId + 1
  const normalizedQuery = normalizeSearchQuery(query)
  leaderboardRequestId = requestId
  leaderboardPeriod.value = period
  leaderboardLoading.value = true
  leaderboardError.value = ""

  if (leaderboardAbortController) {
    leaderboardAbortController.abort()
  }
  const controller = new AbortController()
  leaderboardAbortController = controller

  try {
    const params = new URLSearchParams({ period, limit: "100" })
    if (normalizedQuery) params.set("q", normalizedQuery)
    const response = await fetch(`/api/leaderboard?${params}`, { signal: controller.signal })
    if (!response.ok) throw new Error(`Leaderboard request failed (${response.status})`)
    const payload = await response.json()
    if (requestId !== leaderboardRequestId) return
    leaderboard.value = Array.isArray(payload?.leaderboard) ? payload.leaderboard : []
    leaderboardAppliedSearch.value = normalizedQuery
  } catch (error) {
    if (error?.name === "AbortError" || requestId !== leaderboardRequestId) return
    leaderboardError.value = error instanceof Error ? error.message : "Unable to load leaderboard."
  } finally {
    if (requestId === leaderboardRequestId) {
      leaderboardLoading.value = false
      if (leaderboardAbortController === controller) {
        leaderboardAbortController = null
      }
    }
  }
}

function setPeriod(period) {
  if (period === leaderboardPeriod.value) return
  loadLeaderboard(period, normalizedSearchQuery.value)
}

function clearLeaderboardSearchTimer() {
  if (leaderboardSearchTimer) {
    window.clearTimeout(leaderboardSearchTimer)
    leaderboardSearchTimer = null
  }
}

async function openLeaderboardSearch() {
  if (route.value.name !== "leaderboard") return
  searchOpen.value = true
  accountPanelOpen.value = false
  await nextTick()
  searchInput.value?.focus()
}

function closeLeaderboardSearch({ reload = true } = {}) {
  const shouldReload = reload && (normalizedSearchQuery.value || leaderboardAppliedSearch.value)
  clearLeaderboardSearchTimer()
  searchOpen.value = false
  searchQuery.value = ""
  if (shouldReload) {
    loadLeaderboard(leaderboardPeriod.value, "")
  }
}

async function loadBadges() {
  badgesLoading.value = true
  badgesError.value = ""
  try {
    const response = await fetch("/api/badges")
    if (!response.ok) throw new Error(`Badges request failed (${response.status})`)
    const payload = await response.json()
    const list = Array.isArray(payload?.badges) ? payload.badges : []
    const map = {}
    for (const b of list) {
      const uname = b.holder?.username
      if (uname) {
        if (!map[uname]) map[uname] = []
        map[uname].push(b)
      }
    }
    userBadges.value = map
    badges.value = list
  } catch (error) {
    badgesError.value = error instanceof Error ? error.message : "Unable to load badges."
  } finally {
    badgesLoading.value = false
  }
}

async function loadAuth() {
  authLoading.value = true
  try {
    const response = await fetch("/api/auth/me", { cache: "no-cache" })
    if (!response.ok) throw new Error(`Auth request failed (${response.status})`)
    const payload = await response.json()
    authUser.value = payload?.authenticated ? payload.user : null
  } catch (error) {
    console.error("Failed to load auth state:", error)
    authUser.value = null
  } finally {
    authLoading.value = false
  }
}

async function loadAccountStats() {
  const username = accountProfileUsername.value
  if (!username) return
  if (accountStats.value && accountStatsUsername.value === username) return

  accountStatsLoading.value = true
  accountStatsError.value = ""
  try {
    if (userStats.value?.username === username) {
      accountStats.value = userStats.value
      accountStatsUsername.value = username
      return
    }

    const stats = await ensureProfileStats(username)
    if (accountProfileUsername.value !== username) return
    accountStats.value = stats
    accountStatsUsername.value = username
  } catch (error) {
    if (accountProfileUsername.value !== username) return
    accountStats.value = null
    accountStatsUsername.value = ""
    accountStatsError.value = error instanceof Error
      ? error.message.replace("User not found.", "Profile stats not found.")
      : "Unable to load profile stats."
  } finally {
    if (accountProfileUsername.value === username || !accountProfileUsername.value) {
      accountStatsLoading.value = false
    }
  }
}

function loginWithGitHub() {
  const returnTo = `${window.location.pathname}${window.location.search}`
  window.location.href = `/api/auth/github?return_to=${encodeURIComponent(returnTo)}`
}

async function logout() {
  try {
    await fetch("/api/auth/logout", { method: "POST" })
  } finally {
    authUser.value = null
    accountPanelOpen.value = false
    accountStats.value = null
    accountStatsUsername.value = ""
    accountStatsError.value = ""
  }
}

function setSort(column) {
  if (sortKey.value === column) {
    sortDirection.value = sortDirection.value === "desc" ? "asc" : "desc"
    return
  }
  sortKey.value = column
  sortDirection.value = NUMERIC_COLUMNS.has(column) ? "desc" : "asc"
}

function setModelSort(column) {
  if (modelSortKey.value === column) {
    modelSortDir.value = modelSortDir.value === "desc" ? "asc" : "desc"
    return
  }
  modelSortKey.value = column
  modelSortDir.value = NUMERIC_COLUMNS.has(column) ? "desc" : "asc"
}

function modelSortIndicator(col) {
  if (modelSortKey.value !== col) return ""
  return modelSortDir.value === "desc" ? "▾" : "▴"
}

async function loadUserStats(username, requestId) {
  try {
    const stats = await ensureProfileStats(username)
    if (!isCurrentProfileRequest(username, requestId)) return
    applyUserStatsPayload(stats)
  } catch (error) {
    if (!isCurrentProfileRequest(username, requestId)) return
    statsError.value = error instanceof Error ? error.message : "Unable to load user statistics."
    cleanupCharts()
  } finally {
    if (isCurrentProfileRequest(username, requestId)) statsLoading.value = false
  }
}

async function loadProfileContributions(username, requestId) {
  try {
    const contributions = await ensureProfileContributions(username)
    if (!isCurrentProfileRequest(username, requestId)) return
    ghContributions.value = contributions
  } catch (error) {
    if (!isCurrentProfileRequest(username, requestId)) return
    console.error("Failed to load GitHub contributions:", error)
    ghContributions.value = []
  } finally {
    if (isCurrentProfileRequest(username, requestId)) ghContribsLoading.value = false
  }
}

function resetProfileLoadingState() {
  userStats.value = null
  statsError.value = ""
  statsLoading.value = true
  ghContribsLoading.value = true
  ghContributions.value = []
  ghTokenMap.value = {}
  ghHoveredDay.value = null
  ghHoveredPos.value = null
  ghDetail.value = null
  ghHeatmapLabelHover.value = false
  timelineEmpty.value = false
  tokenMixEmpty.value = false
  modelMixEmpty.value = false
  diffEmpty.value = false
  providerSpendEmpty.value = false
  agentBreakdownEmpty.value = false
  cleanupCharts()
}

function loadProfile(username) {
  if (!username) return
  const requestId = activeProfileRequestId + 1
  activeProfileRequestId = requestId
  resetProfileLoadingState()
  loadUserStats(username, requestId)
  loadProfileContributions(username, requestId)
}

function goToUser(username) {
  if (!username) return
  searchOpen.value = false
  clearLeaderboardSearchTimer()
  history.pushState({}, "", `/users/${encodeURIComponent(username)}`)
  route.value = { name: "user", username }
  loadProfile(username)
}

function goToAuthenticatedProfile() {
  if (!accountProfileUsername.value) return
  accountPanelOpen.value = false
  goToUser(accountProfileUsername.value)
}

function goToLeaderboard() {
  history.pushState({}, "", "/")
  route.value = { name: "leaderboard", username: null }
  closeLeaderboardSearch({ reload: Boolean(normalizedSearchQuery.value || leaderboardAppliedSearch.value) })
  activeProfileRequestId += 1
  statsError.value = ""
  statsLoading.value = false
  ghContribsLoading.value = false
  userStats.value = null
  cleanupCharts()
}

// ── Charts ───────────────────────────────────────────

function cleanupCharts() {
  timelineChart?.destroy(); timelineChart = null
  tokenMixChart?.destroy();  tokenMixChart = null
  modelMixChart?.destroy();  modelMixChart = null
  diffChart?.destroy();      diffChart = null
  providerSpendChart?.destroy(); providerSpendChart = null
  agentBreakdownChart?.destroy(); agentBreakdownChart = null
}

function getCanvas(id) {
  const el = document.getElementById(id)
  return el instanceof HTMLCanvasElement ? el : null
}

function setChartRangeMetadata(canvas, entries) {
  if (!canvas) return
  const dates = Array.isArray(entries)
    ? entries.map((entry) => dateKey(entry?.date)).filter(Boolean)
    : []

  if (dates.length === 0) {
    delete canvas.dataset.chartStart
    delete canvas.dataset.chartEnd
    canvas.dataset.chartCount = "0"
    return
  }

  canvas.dataset.chartStart = dates[0]
  canvas.dataset.chartEnd = dates[dates.length - 1]
  canvas.dataset.chartCount = String(dates.length)
}

function renderCharts() {
  if (!userStats.value) return
  cleanupCharts()

  const tlCanvas = getCanvas("chart-timeline")
  const tmCanvas = getCanvas("chart-tokenmix")
  const mmCanvas = getCanvas("chart-modelmix")

  const fullTimeline = Array.isArray(userStats.value.timeline) ? userStats.value.timeline : []
  const timeline = timelineForHeatmapWindow(fullTimeline)
  const models = Array.isArray(userStats.value.models) ? userStats.value.models : []
  const tokenData = [
    Number(userStats.value.input_tokens ?? 0),
    Number(userStats.value.output_tokens ?? 0),
    Number(userStats.value.cache_read_tokens ?? 0),
    Number(userStats.value.cache_write_tokens ?? 0),
    Number(userStats.value.reasoning_tokens ?? 0),
  ]

  const gridColor = "rgba(255,255,255,0.05)"
  const tickColor = "#6b7180"

  timelineEmpty.value = timeline.length === 0
  setChartRangeMetadata(tlCanvas, timeline)

  // ── Timeline chart ──
  if (tlCanvas && timeline.length > 0) {
    try {
      timelineChart = new Chart(tlCanvas, {
        type: "line",
        data: {
          labels: timeline.map((e) => dateLabel(e.date)),
          datasets: [{
            label: "Daily tokens",
            data: timeline.map((e) => Number(e.total_tokens ?? 0)),
            borderColor: "#06d6d0",
            backgroundColor: "rgba(6, 214, 208, 0.08)",
            tension: 0.3,
            fill: true,
            pointRadius: timeline.length > 60 ? 0 : 2,
            pointHoverRadius: 5,
            pointBackgroundColor: "#06d6d0",
            borderWidth: 1.5,
          }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          interaction: { intersect: false, mode: "index" },
          plugins: {
            legend: { display: false },
            tooltip: {
              backgroundColor: "#1c1e21",
              titleColor: "#f0f1f2",
              bodyColor: "#b0b6be",
              borderColor: "rgba(255,255,255,0.09)",
              borderWidth: 1,
              cornerRadius: 8,
              padding: 10,
              callbacks: {
                label: (ctx) => ` ${ctx.parsed.y.toLocaleString()} tokens`,
              },
            },
          },
          scales: {
            x: {
              ticks: { color: tickColor, maxTicksLimit: 8, font: { size: 10 } },
              grid: { color: gridColor },
            },
            y: {
              beginAtZero: true,
              ticks: { color: tickColor, font: { size: 10 }, callback: (v) => v >= 1e6 ? `${(v/1e6).toFixed(1)}M` : v >= 1000 ? `${(v/1000).toFixed(0)}K` : v },
              grid: { color: gridColor },
            },
          },
        },
      })
    } catch (err) {
      console.error("Timeline chart failed:", err)
    }
  }

  // ── Token mix chart (horizontal bars, log scale) ──
  const allLabels = ["Input", "Output", "Cache read", "Cache write", "Reasoning"]
  const allValues = [tokenData[0], tokenData[1], tokenData[2], tokenData[3], tokenData[4]]
  const tokenPairs = allLabels.map((l, i) => ({ label: l, value: allValues[i] })).filter((p) => p.value > 0)
  const hasTokenData = tokenPairs.length > 0
  if (tmCanvas && hasTokenData) {
    const colors = ["#06d6d0", "#818cf8", "#a78bfa", "#f59e0b", "#34d399"]
    try {
      tokenMixChart = new Chart(tmCanvas, {
        type: "bar",
        data: {
          labels: tokenPairs.map((p) => p.label),
          datasets: [{
            data: tokenPairs.map((p) => p.value),
            backgroundColor: tokenPairs.map((_, i) => colors[allLabels.indexOf(tokenPairs[i].label)]),
            borderRadius: 6,
            borderSkipped: false,
            barPercentage: 0.65,
            categoryPercentage: 0.80,
          }],
        },
        options: {
          indexAxis: "y",
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: { display: false },
            tooltip: {
              backgroundColor: "#1c1e21",
              titleColor: "#f0f1f2",
              bodyColor: "#b0b6be",
              borderColor: "rgba(255,255,255,0.09)",
              borderWidth: 1,
              cornerRadius: 8,
              padding: 10,
              callbacks: {
                label: (ctx) => {
                  const raw = tokenPairs[ctx.dataIndex]?.value ?? 0
                  const total = tokenPairs.reduce((a, b) => a + b.value, 0)
                  const pct = total > 0 ? ((raw / total) * 100).toFixed(1) : 0
                  return ` ${raw.toLocaleString()} (${pct}%)`
                },
              },
            },
          },
          scales: {
            x: {
              type: "logarithmic",
              ticks: { color: tickColor, maxTicksLimit: 6, font: { size: 10 }, callback: (v) => v >= 1e6 ? `${(v/1e6).toFixed(1)}M` : v >= 1000 ? `${(v/1000).toFixed(0)}K` : v },
              grid: { color: gridColor },
            },
            y: {
              ticks: { color: tickColor, font: { size: 10 } },
              grid: { color: gridColor },
            },
          },
        },
      })
    } catch (err) {
      console.error("Token mix chart failed:", err)
    }
  }

  // ── Model mix chart ──
  if (mmCanvas && models.length > 0) {
    const top = models.slice(0, 8)
    try {
      modelMixChart = new Chart(mmCanvas, {
        type: "doughnut",
        data: {
          labels: top.map((e) => e.model),
          datasets: [{
            data: top.map((e) => Number(e.tokens ?? 0)),
            backgroundColor: ["#06d6d0", "#818cf8", "#a78bfa", "#34d399", "#f59e0b", "#f472b6", "#facc15", "#38bdf8"],
            borderColor: "#111316",
            borderWidth: 2,
          }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          cutout: "60%",
          plugins: {
            legend: {
              position: "bottom",
              labels: { color: "#b0b6be", padding: 12, font: { size: 10 }, usePointStyle: true, pointStyleWidth: 8 },
            },
            tooltip: {
              backgroundColor: "#1c1e21",
              titleColor: "#f0f1f2",
              bodyColor: "#b0b6be",
              borderColor: "rgba(255,255,255,0.09)",
              borderWidth: 1,
              cornerRadius: 8,
              padding: 10,
              callbacks: {
                label: (ctx) => {
                  const total = ctx.dataset.data.reduce((a, b) => a + b, 0)
                  const pct = total > 0 ? ((ctx.parsed / total) * 100).toFixed(1) : 0
                  return ` ${ctx.parsed.toLocaleString()} tokens (${pct}%)`
                },
              },
            },
          },
        },
      })
    } catch (err) {
      console.error("Model mix chart failed:", err)
    }
  }

  // ── Velocity (day-over-day delta) chart ──
  const dvCanvas = getCanvas("chart-diff")
  const dayOverDay = dayOverDayForTimelineWindow(userStats.value.diffs?.day_over_day, timeline)
  diffEmpty.value = dayOverDay.length === 0
  setChartRangeMetadata(dvCanvas, dayOverDay)
  if (dvCanvas && dayOverDay.length > 0) {
    try {
      diffChart = new Chart(dvCanvas, {
        type: "bar",
        data: {
          labels: dayOverDay.map((e) => dateLabel(e.date)),
          datasets: [{
            data: dayOverDay.map((e) => Number(e.delta_total_tokens ?? 0)),
            backgroundColor: dayOverDay.map((e) =>
              Number(e.delta_total_tokens ?? 0) >= 0
                ? "rgba(6, 214, 208, 0.55)"
                : "rgba(239, 68, 68, 0.55)",
            ),
            borderColor: dayOverDay.map((e) =>
              Number(e.delta_total_tokens ?? 0) >= 0
                ? "#06d6d0"
                : "#ef4444",
            ),
            borderWidth: 1,
            borderRadius: 4,
            borderSkipped: false,
          }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: { display: false },
            tooltip: {
              backgroundColor: "#1c1e21",
              titleColor: "#f0f1f2",
              bodyColor: "#b0b6be",
              borderColor: "rgba(255,255,255,0.09)",
              borderWidth: 1,
              cornerRadius: 8,
              padding: 10,
              callbacks: {
                label: (ctx) => {
                  const val = ctx.parsed.y
                  const sign = val >= 0 ? "+" : ""
                  const pct = ctx.raw?.percent_change ?? ""
                  return ` ${sign}${val.toLocaleString()} tokens${pct ? ` (${sign}${pct}%)` : ""}`
                },
              },
            },
          },
          scales: {
            x: {
              ticks: { color: tickColor, maxTicksLimit: 8, font: { size: 10 } },
              grid: { color: gridColor },
            },
            y: {
              ticks: {
                color: tickColor,
                font: { size: 10 },
                callback: (v) => {
                  const abs = Math.abs(v)
                  return abs >= 1e6 ? `${(v >= 0 ? "+" : "-")}${(abs/1e6).toFixed(1)}M`
                    : abs >= 1000 ? `${(v >= 0 ? "+" : "-")}${(abs/1000).toFixed(0)}K`
                    : v
                },
              },
              grid: { color: gridColor },
            },
          },
        },
      })
    } catch (err) {
      console.error("Diff chart failed:", err)
    }
  }

  // ── Provider spend (horizontal bar, models aggregated by provider cost) ──
  const psCanvas = getCanvas("chart-provider-spend")
  // Aggregate total_cost by provider (models declared above for model-mix)
  const providerCostMap = {}
  for (const m of models) {
    const prov = m.provider || "Unknown"
    const cost = Number(m.total_cost ?? 0)
    if (cost > 0) providerCostMap[prov] = (providerCostMap[prov] ?? 0) + cost
  }
  const providers = Object.entries(providerCostMap)
    .sort((a, b) => b[1] - a[1])
  if (psCanvas && providers.length > 0) {
    // Per-provider colors matching badge CSS
    const providerColors = {
      Anthropic: "#fbbf24", OpenAI: "#34d399", Google: "#818cf8",
      DeepSeek: "#06d6d0", Meta: "#a78bfa", Alibaba: "#f97316",
      Moonshot: "#f472b6", Nous: "#fb923c", GitHub: "#c9d1d9",
    }
    const barColors = providers.map(([prov]) => providerColors[prov] || "#6b7180")
    try {
      providerSpendChart = new Chart(psCanvas, {
        type: "bar",
        data: {
          labels: providers.map(([prov]) => prov),
          datasets: [{
            data: providers.map(([, cost]) => cost),
            backgroundColor: barColors.map((c) => c + "99"),
            borderColor: barColors,
            borderWidth: 1,
            borderRadius: 6,
            borderSkipped: false,
            barPercentage: 0.65,
            categoryPercentage: 0.80,
          }],
        },
        options: {
          indexAxis: "y",
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: { display: false },
            tooltip: {
              backgroundColor: "#1c1e21",
              titleColor: "#f0f1f2",
              bodyColor: "#b0b6be",
              borderColor: "rgba(255,255,255,0.09)",
              borderWidth: 1,
              cornerRadius: 8,
              padding: 10,
              callbacks: {
                label: (ctx) => ` $${ctx.parsed.x.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`,
              },
            },
          },
          scales: {
            x: {
              ticks: { color: tickColor, maxTicksLimit: 6, font: { size: 10 }, callback: (v) => v >= 1000 ? `$${(v/1000).toFixed(1)}K` : `$${v.toFixed(0)}` },
              grid: { color: gridColor },
            },
            y: {
              ticks: { color: tickColor, font: { size: 10 } },
              grid: { color: gridColor },
            },
          },
        },
      })
    } catch (err) {
      console.error("Provider spend chart failed:", err)
    }
  }

  // ── Agent breakdown (horizontal bar) ──
  const abCanvas = getCanvas("chart-agent-breakdown")
  const clients = userStats.value.client_breakdown ?? []
  if (abCanvas && clients.length > 0) {
    const sorted = [...clients].sort((a, b) => Number(b.tokens ?? 0) - Number(a.tokens ?? 0))
    const cap = (s) => s ? s.charAt(0).toUpperCase() + s.slice(1) : ""
    try {
      agentBreakdownChart = new Chart(abCanvas, {
        type: "bar",
        data: {
          labels: sorted.map((c) => cap(c.client)),
          datasets: [{
            data: sorted.map((c) => Number(c.tokens ?? 0)),
            backgroundColor: "rgba(245, 158, 11, 0.55)",
            borderColor: "#f59e0b",
            borderWidth: 1,
            borderRadius: 6,
            borderSkipped: false,
            barPercentage: 0.65,
            categoryPercentage: 0.80,
          }],
        },
        options: {
          indexAxis: "y",
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: { display: false },
            tooltip: {
              backgroundColor: "#1c1e21",
              titleColor: "#f0f1f2",
              bodyColor: "#b0b6be",
              borderColor: "rgba(255,255,255,0.09)",
              borderWidth: 1,
              cornerRadius: 8,
              padding: 10,
              callbacks: {
                label: (ctx) => `${ctx.parsed.x.toLocaleString()} tokens`,
              },
            },
          },
          scales: {
            x: {
              ticks: { color: tickColor, maxTicksLimit: 6, font: { size: 10 }, callback: (v) => v >= 1e6 ? `${(v/1e6).toFixed(1)}M` : v >= 1000 ? `${(v/1000).toFixed(0)}K` : v },
              grid: { color: gridColor },
            },
            y: {
              ticks: { color: tickColor, font: { size: 10 } },
              grid: { color: gridColor },
            },
          },
        },
      })
    } catch (err) {
      console.error("Agent breakdown chart failed:", err)
    }
  }
}

// ── Lifecycle ────────────────────────────────────────

function handlePopstate() {
  applyRouteFromLocation()
  if (route.value.name === "user" && route.value.username) {
    loadProfile(route.value.username)
  } else {
    closeLeaderboardSearch({ reload: Boolean(normalizedSearchQuery.value || leaderboardAppliedSearch.value) })
    activeProfileRequestId += 1
    statsLoading.value = false
    ghContribsLoading.value = false
    cleanupCharts()
  }
}

onMounted(() => {
  applyRouteFromLocation()
  if (route.value.name === "user" && route.value.username) {
    loadProfile(route.value.username)
  }
  loadAuth()
  loadLeaderboard()
  loadBadges()
  window.addEventListener("popstate", handlePopstate)
})

onUnmounted(() => {
  window.removeEventListener("popstate", handlePopstate)
  clearLeaderboardSearchTimer()
  leaderboardAbortController?.abort()
  cleanupCharts()
})

// Render charts as soon as stats are ready; heatmaps load independently.
watch(statsLoading, async (loading) => {
  if (!loading && userStats.value) {
    await nextTick()
    renderCharts()
  }
})

watch(ghContributions, async () => {
  if (!statsLoading.value && userStats.value) {
    await nextTick()
    renderCharts()
  }
})

watch(accountPanelOpen, (open) => {
  if (open) loadAccountStats()
})

watch(authUser, () => {
  accountStats.value = null
  accountStatsUsername.value = ""
  accountStatsError.value = ""
})

watch(searchQuery, () => {
  if (!searchOpen.value || route.value.name !== "leaderboard") return
  clearLeaderboardSearchTimer()
  leaderboardSearchTimer = window.setTimeout(() => {
    loadLeaderboard(leaderboardPeriod.value, normalizedSearchQuery.value)
  }, 250)
})
</script>

<template>
  <main class="app">
    <header class="app-header">
      <div class="app-header-left">
        <h1 @click="goToLeaderboard" class="logo-link">Tokenboard</h1>
        <a
          class="repo-link header-install-link"
          href="https://github.com/james-uea/tokenboard#quick-start"
          target="_blank"
          rel="noopener noreferrer"
          aria-label="Install the Tokenboard CLI from GitHub"
        >
          <span class="install-link-icon" aria-hidden="true">
            <svg viewBox="0 0 19 19" focusable="false">
              <use href="/icons.svg#github-icon"></use>
            </svg>
          </span>
          Install the CLI
        </a>
      </div>

      <div class="app-header-actions" :class="{ 'search-expanded': route.name === 'leaderboard' && searchOpen }">
        <form
          v-if="route.name === 'leaderboard' && searchOpen"
          class="leaderboard-search"
          role="search"
          @submit.prevent
        >
          <label class="sr-only" for="leaderboard-user-search">Search Tokenboard usernames</label>
          <svg class="search-field-icon" viewBox="0 0 24 24" aria-hidden="true">
            <circle cx="11" cy="11" r="7"></circle>
            <path d="m16 16 4 4"></path>
          </svg>
          <input
            id="leaderboard-user-search"
            ref="searchInput"
            v-model="searchQuery"
            type="search"
            autocomplete="off"
            spellcheck="false"
            placeholder="Search usernames"
            @keydown.esc.prevent="closeLeaderboardSearch()"
          />
          <button
            v-if="searchQuery"
            type="button"
            class="search-clear"
            aria-label="Clear username search"
            @click="closeLeaderboardSearch()"
          >
            <span aria-hidden="true">&times;</span>
          </button>
          <button type="button" class="search-close" @click="closeLeaderboardSearch()">Close</button>
        </form>

        <template v-else>
          <nav v-if="route.name === 'leaderboard'" class="period-selector" aria-label="Time period">
            <button
              v-for="period in PERIODS"
              :key="period.key"
              type="button"
              class="period-btn"
              :class="{ active: leaderboardPeriod === period.key }"
              @click="setPeriod(period.key)"
            >
              {{ period.label }}
            </button>
          </nav>

          <div class="account-actions">
            <button
              v-if="authUser"
              type="button"
              class="account-chip"
              :class="{ active: accountPanelOpen }"
              @click="accountPanelOpen = !accountPanelOpen"
            >
              <img v-if="authAvatarUsername" :src="avatarUrl(authAvatarUsername)" alt="" />
              <span>@{{ authUser.github_login || authUser.username }}</span>
            </button>
            <button
              v-else
              type="button"
              class="github-login"
              :disabled="authLoading"
              @click="loginWithGitHub"
            >
              Sign in with GitHub
            </button>
          </div>

          <button
            v-if="route.name === 'leaderboard'"
            type="button"
            class="search-toggle"
            aria-label="Search Tokenboard usernames"
            @click="openLeaderboardSearch"
          >
            <svg viewBox="0 0 24 24" aria-hidden="true">
              <circle cx="11" cy="11" r="7"></circle>
              <path d="m16 16 4 4"></path>
            </svg>
          </button>
        </template>
      </div>
    </header>

    <section v-if="authUser && accountPanelOpen" class="account-panel">
      <div class="account-panel-header">
        <button type="button" class="account-profile-link" @click="goToAuthenticatedProfile">
          <span class="account-display-name">{{ authUser.display_name || authUser.username }}</span>
          <span class="handle">@{{ authUser.github_login || authUser.username }}</span>
        </button>
        <button type="button" class="text-button" @click="logout">Sign out</button>
      </div>
      <div class="account-stats" aria-label="Profile quick stats">
        <template v-if="accountStatsLoading">
          <div v-for="n in 4" :key="n" class="account-stat account-stat-skeleton" aria-hidden="true">
            <span class="skeleton skeleton-label"></span>
            <span class="skeleton skeleton-value"></span>
          </div>
        </template>
        <p v-else-if="accountStatsError" class="account-stats-state error">{{ accountStatsError }}</p>
        <template v-else-if="accountStats">
          <div class="account-stat">
            <span class="account-stat-label">Tokens</span>
            <strong>{{ formatInteger(accountStats.total_tokens) }}</strong>
          </div>
          <div class="account-stat">
            <span class="account-stat-label">Cost</span>
            <strong>{{ formatCost(accountStats.total_cost) }}</strong>
          </div>
          <div class="account-stat">
            <span class="account-stat-label">Active days</span>
            <strong>{{ formatInteger(accountStats.active_days) }}</strong>
          </div>
          <div class="account-stat">
            <span class="account-stat-label">Top model</span>
            <strong>{{ accountTopModel }}</strong>
          </div>
        </template>
      </div>
    </section>

    <!-- ═══════════ Leaderboard ═══════════ -->
    <section v-if="route.name === 'leaderboard'" class="surface">
      <div class="surface-header">
        <h2>Leaderboard</h2>
        <span class="chip">{{ leaderboardSortLabel }}</span>
      </div>

      <!-- Skeleton loading -->
      <div v-if="leaderboardLoading" class="leaderboard-skeleton" aria-hidden="true">
        <div v-for="n in LEADERBOARD_SKELETON_ROWS" :key="n" class="skeleton-row leaderboard-skeleton-row">
          <span class="skeleton skeleton-rank"></span>
          <span class="skeleton skeleton-avatar"></span>
          <span class="skeleton skeleton-name-cell"></span>
          <span class="skeleton skeleton-pill"></span>
          <span class="skeleton skeleton-number"></span>
          <span class="skeleton skeleton-number"></span>
          <span class="skeleton skeleton-number short"></span>
          <span class="skeleton skeleton-total"></span>
          <span class="skeleton skeleton-cost"></span>
        </div>
      </div>

      <!-- Error state -->
      <p v-else-if="leaderboardError" class="state error">{{ leaderboardError }}</p>

      <!-- Empty state -->
      <div v-else-if="sortedLeaderboard.length === 0" class="empty-state">
        <div class="empty-icon">📊</div>
        <h3>{{ hasActiveLeaderboardSearch ? "No matching users" : "No data yet" }}</h3>
        <p v-if="hasActiveLeaderboardSearch">No users match "{{ leaderboardAppliedSearch }}".</p>
        <p v-else>No token usage has been submitted for this period. Run <code>tokenboard sync</code> to submit your first entries.</p>
      </div>

      <!-- Table -->
      <div v-else class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>#</th>
              <th>User</th>
              <th>
                <button type="button" @click="setSort('top_model')">
                  Model {{ sortIndicator('top_model') }}
                </button>
              </th>
              <th>
                <button type="button" @click="setSort('input_tokens')">
                  Input {{ sortIndicator('input_tokens') }}
                </button>
              </th>
              <th>
                <button type="button" @click="setSort('output_tokens')">
                  Output {{ sortIndicator('output_tokens') }}
                </button>
              </th>
              <th>
                <button type="button" @click="setSort('cache_read_tokens')">
                  Cache&nbsp;read {{ sortIndicator('cache_read_tokens') }}
                </button>
              </th>
              <th>
                <button type="button" @click="setSort('cache_write_tokens')">
                  Cache&nbsp;write {{ sortIndicator('cache_write_tokens') }}
                </button>
              </th>
              <th>
                <button type="button" @click="setSort('total_tokens')">
                  Total {{ sortIndicator('total_tokens') }}
                </button>
              </th>
              <th>
                <button type="button" @click="setSort('total_cost')">
                  Cost {{ sortIndicator('total_cost') }}
                </button>
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="(entry, index) in sortedLeaderboard" :key="entry.username">
              <td><span :class="rankClass(index)">{{ index + 1 }}</span></td>
              <td>
                <button
                  type="button"
                  class="user-link"
                  @pointerenter="preloadProfile(entry.username)"
                  @focus="preloadProfile(entry.username)"
                  @click="goToUser(entry.username)"
                >
                  <span class="user-cell">
                    <img class="avatar" :src="avatarUrl(entry.username)" alt="" loading="lazy" />
                    <span class="identity">
                      <strong>
                        {{ entry.display_name || entry.username }}
                        <span v-if="getPrimaryBadge(entry.username)" class="badge-pill" :title="getPrimaryBadge(entry.username).label">{{ getPrimaryBadge(entry.username).emoji }}</span>
                      </strong>
                      <span class="handle">@{{ entry.username }}</span>
                    </span>
                  </span>
                </button>
              </td>
              <td>
                <span v-if="entry.top_model && entry.top_model !== '—'" class="model-badge">{{ shortModel(entry.top_model) }}</span>
                <span v-else style="color:var(--text-dim)">—</span>
              </td>
              <td>{{ formatInteger(entry.input_tokens) }}</td>
              <td>{{ formatInteger(entry.output_tokens) }}</td>
              <td>{{ formatInteger(entry.cache_read_tokens) }}</td>
              <td>{{ formatInteger(entry.cache_write_tokens) }}</td>
              <td class="emphasis">{{ formatInteger(entry.total_tokens) }}</td>
              <td>{{ formatCost(entry.total_cost) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>

    <!-- ═══════════ User Stats ═══════════ -->
    <section v-if="route.name === 'user'" class="surface">
      <div class="surface-header">
        <button type="button" class="back" @click="goToLeaderboard">
          ← Leaderboard
        </button>
        <h2>User statistics</h2>
      </div>

      <template v-if="statsLoading">
        <div class="profile profile-skeleton" aria-hidden="true">
          <span class="skeleton skeleton-avatar-lg"></span>
          <div class="profile-skeleton-copy">
            <span class="skeleton skeleton-profile-name"></span>
            <span class="skeleton skeleton-profile-handle"></span>
          </div>
          <div class="gh-contributions heatmap-skeleton">
            <div class="heatmap-col">
              <div class="heatmap-label">Commits</div>
              <div class="gh-contrib-grid">
                <div class="gh-contrib-days">
                  <span v-for="label in DAY_LABELS" :key="label" class="gh-day-label">{{ label }}</span>
                </div>
                <div v-for="n in HEATMAP_SKELETON_WEEKS" :key="n" class="gh-contrib-week">
                  <div v-for="m in 7" :key="m" class="gh-contrib-cell gh-skeleton-cell"></div>
                </div>
              </div>
            </div>
            <div class="heatmap-col">
              <div class="heatmap-label">Tokens</div>
              <div class="tk-contrib-grid">
                <div class="gh-contrib-days">
                  <span v-for="label in DAY_LABELS" :key="label" class="gh-day-label">{{ label }}</span>
                </div>
                <div v-for="n in HEATMAP_SKELETON_WEEKS" :key="n" class="gh-contrib-week">
                  <div v-for="m in 7" :key="m" class="tk-contrib-cell tk-skeleton-cell"></div>
                </div>
              </div>
            </div>
          </div>
          <div class="badge-ribbon-inline badge-ribbon-skeleton">
            <div v-for="n in 3" :key="n" class="badge-medal badge-medal-skeleton">
              <span class="skeleton skeleton-badge-icon"></span>
              <span class="skeleton skeleton-badge-label"></span>
              <span class="skeleton skeleton-badge-value"></span>
            </div>
          </div>
        </div>

        <div class="stat-grid" aria-hidden="true">
          <article v-for="n in STAT_SKELETON_CARDS" :key="n" class="stat-card stat-card-skeleton">
            <span class="skeleton skeleton-stat-label"></span>
            <span class="skeleton skeleton-stat-value"></span>
          </article>
        </div>

        <div class="charts" aria-hidden="true">
          <article v-for="n in CHART_SKELETON_CARDS" :key="n" class="chart-card chart-card-skeleton">
            <span class="skeleton skeleton-chart-title"></span>
            <div class="chart-area chart-skeleton-area">
              <span class="skeleton skeleton-chart-plot"></span>
            </div>
          </article>
        </div>

        <div class="model-dist-section model-dist-skeleton" aria-hidden="true">
          <span class="skeleton skeleton-section-heading"></span>
          <div class="table-wrap model-table-wrap">
            <div v-for="n in MODEL_SKELETON_ROWS" :key="n" class="model-skeleton-row">
              <span class="skeleton skeleton-model-name"></span>
              <span class="skeleton skeleton-provider"></span>
              <span class="skeleton skeleton-number"></span>
              <span class="skeleton skeleton-number"></span>
              <span class="skeleton skeleton-total"></span>
            </div>
          </div>
        </div>
      </template>
      <p v-else-if="statsError" class="state error">{{ statsError }}</p>

      <template v-else-if="userStats">
        <div class="profile">
          <img class="avatar large" :src="avatarUrl(userStats.username)" alt="" />
          <div>
            <h3>{{ userStats.display_name || userStats.username }}</h3>
            <span class="handle">@{{ userStats.username }}</span>
          </div>
          <div v-if="ghContribsLoading" class="gh-contributions heatmap-skeleton" aria-hidden="true">
            <div class="heatmap-col">
              <div class="heatmap-label">Commits</div>
              <div class="gh-contrib-grid">
                <div class="gh-contrib-days">
                  <span v-for="label in DAY_LABELS" :key="label" class="gh-day-label">{{ label }}</span>
                </div>
                <div v-for="n in HEATMAP_SKELETON_WEEKS" :key="n" class="gh-contrib-week">
                  <div v-for="m in 7" :key="m" class="gh-contrib-cell gh-skeleton-cell"></div>
                </div>
              </div>
            </div>
            <div class="heatmap-col">
              <div class="heatmap-label">Tokens</div>
              <div class="tk-contrib-grid">
                <div class="gh-contrib-days">
                  <span v-for="label in DAY_LABELS" :key="label" class="gh-day-label">{{ label }}</span>
                </div>
                <div v-for="n in HEATMAP_SKELETON_WEEKS" :key="n" class="gh-contrib-week">
                  <div v-for="m in 7" :key="m" class="tk-contrib-cell tk-skeleton-cell"></div>
                </div>
              </div>
            </div>
          </div>
          <!-- Real heatmap grids -->
          <div v-else class="gh-contributions">
            <div class="heatmap-col">
              <div class="heatmap-label" :class="{ 'heatmap-label-active': ghHeatmapLabelHover }"
                   @mouseenter="onHeatmapLabelEnter" @mouseleave="onHeatmapLabelLeave">Commits</div>
              <div class="gh-contrib-grid">
                <div class="gh-contrib-days">
                  <span v-for="label in DAY_LABELS" :key="label" class="gh-day-label">{{ label }}</span>
                </div>
                <div v-for="(week, wi) in ghWeeks" :key="wi" class="gh-contrib-week">
                  <div
                    v-for="(day, di) in week"
                    :key="di"
                    class="gh-contrib-cell"
                    :class="[day ? 'gh-level-' + day.level : 'gh-level-empty', { 'gh-cell-synced-hover': isCellSynced(wi, di) }]"
                    :title="day ? day.date + ': ' + day.count.toLocaleString() + ' commits' : ''"
                    @mouseenter="onGhCellEnter(day, wi, di)"
                    @mouseleave="onGhCellLeave"
                  ></div>
                </div>
              </div>
            </div>
            <!-- Token usage heatmap (same weeks, relative purple scale) -->
            <div class="heatmap-col">
              <div class="heatmap-label" :class="{ 'heatmap-label-active': ghHeatmapLabelHover }"
                   @mouseenter="onHeatmapLabelEnter" @mouseleave="onHeatmapLabelLeave">Tokens</div>
              <div class="tk-contrib-grid">
                <div class="gh-contrib-days">
                  <span v-for="label in DAY_LABELS" :key="label" class="gh-day-label">{{ label }}</span>
                </div>
                <div v-for="(week, wi) in ghWeeks" :key="'tk' + wi" class="gh-contrib-week">
                  <div
                    v-for="(day, di) in week"
                    :key="di"
                    class="tk-contrib-cell"
                    :class="[day && day.tokenLevel > 0 ? 'tk-level-' + day.tokenLevel : 'tk-level-empty', { 'tk-cell-synced-hover': isCellSynced(wi, di) }]"
                    :title="day && day.tokenCount > 0 ? day.date + ': ' + day.tokenCount.toLocaleString() + ' tokens' : ''"
                    @mouseenter="onGhCellEnter(day, wi, di)"
                    @mouseleave="onGhCellLeave"
                  ></div>
                </div>
              </div>
            </div>
            <!-- Daily detail panel (appears on hover) -->
            <div v-if="ghHoveredDay" class="gh-detail">
              <div class="gh-detail-date">{{ ghHoveredDay.date }}</div>
              <div v-if="ghHoveredDay.tokenCount > 0" class="gh-detail-tokens">
                {{ ghHoveredDay.tokenCount.toLocaleString() }} tokens
              </div>
              <div v-if="ghDetailLoading" class="gh-detail-loading">Loading…</div>
              <template v-else-if="ghDetail && ghDetail.repos && ghDetail.repos.length > 0">
                <div v-for="repo in ghDetail.repos" :key="repo.repo" class="gh-detail-repo">
                  <div class="gh-detail-repo-name">{{ truncate(repo.repo, 32) }}</div>
                  <div v-if="repo.description" class="gh-detail-repo-desc">{{ truncate(repo.description, 60) }}</div>
                  <div class="gh-detail-repo-stats">
                    <span class="gh-stat-add">+{{ repo.additions.toLocaleString() }}</span>
                    <span class="gh-stat-sep">/</span>
                    <span class="gh-stat-del">−{{ repo.deletions.toLocaleString() }}</span>
                    <span class="gh-stat-commits">{{ repo.commits }} commit{{ repo.commits !== 1 ? 's' : '' }}</span>
                  </div>
                </div>
              </template>
              <div v-else class="gh-detail-empty">No public activity this day</div>
            </div>
          </div>
          <div v-if="getUserBadges(userStats.username).length > 0" class="badge-ribbon-inline">
            <div v-for="badge in getUserBadges(userStats.username).slice(0, 3)" :key="badge.key" :class="['badge-medal', badge.key]">
              <div class="badge-medal-icon">{{ badge.emoji }}</div>
              <span class="badge-medal-label">{{ badge.label }}</span>
              <span class="badge-medal-value">{{ badge.value }}</span>
              <div class="badge-medal-tip">{{ badge.description }}</div>
            </div>
          </div>
        </div>

        <!-- Stat cards -->
        <div class="stat-grid">
          <article class="stat-card">
            <span class="stat-label">Total tokens</span>
            <span class="stat-value">{{ formatInteger(userStats.total_tokens) }}</span>
          </article>
          <article class="stat-card">
            <span class="stat-label">Total cost</span>
            <span class="stat-value cost">{{ formatCost(userStats.total_cost) }}</span>
          </article>
          <article class="stat-card">
            <span class="stat-label">Input tokens</span>
            <span class="stat-value">{{ formatInteger(userStats.input_tokens) }}</span>
          </article>
          <article class="stat-card">
            <span class="stat-label">Output tokens</span>
            <span class="stat-value">{{ formatInteger(userStats.output_tokens) }}</span>
          </article>
          <article class="stat-card">
            <span class="stat-label">Cache read</span>
            <span class="stat-value">{{ formatInteger(userStats.cache_read_tokens) }}</span>
          </article>
          <article class="stat-card">
            <span class="stat-label">Cache write</span>
            <span class="stat-value">{{ formatInteger(userStats.cache_write_tokens) }}</span>
          </article>
        </div>

        <!-- Charts -->
        <div class="charts">
          <article class="chart-card">
            <h4>Daily token usage</h4>
            <div class="chart-area">
              <canvas id="chart-timeline"></canvas>
              <div v-if="timelineEmpty" class="chart-empty-overlay">No daily data yet</div>
            </div>
          </article>
          <article class="chart-card">
            <h4>Token type breakdown</h4>
            <div class="chart-area">
              <canvas id="chart-tokenmix"></canvas>
              <div v-if="tokenMixEmpty" class="chart-empty-overlay">No token data yet</div>
            </div>
          </article>
          <article class="chart-card">
            <h4>Model distribution</h4>
            <div class="chart-area">
              <canvas id="chart-modelmix"></canvas>
              <div v-if="modelMixEmpty" class="chart-empty-overlay">No model data yet</div>
            </div>
          </article>
          <article class="chart-card">
            <h4>Day-over-day velocity</h4>
            <div class="chart-area">
              <canvas id="chart-diff"></canvas>
              <div v-if="diffEmpty" class="chart-empty-overlay">Need more days for trends</div>
            </div>
          </article>
          <article class="chart-card">
            <h4>Provider spend</h4>
            <div class="chart-area">
              <canvas id="chart-provider-spend"></canvas>
              <div v-if="providerSpendEmpty" class="chart-empty-overlay">No cost data yet</div>
            </div>
          </article>
          <article class="chart-card">
            <h4>Agent breakdown</h4>
            <div class="chart-area">
              <canvas id="chart-agent-breakdown"></canvas>
              <div v-if="agentBreakdownEmpty" class="chart-empty-overlay">No agent data yet</div>
            </div>
          </article>
        </div>

        <!-- Model distribution table -->
        <div v-if="sortedModelBreakdown.length > 0" class="model-dist-section">
          <h4 class="section-heading">Model distribution</h4>
          <div class="table-wrap model-table-wrap">
            <table class="model-table">
              <thead>
                <tr>
                  <th>
                    <button type="button" @click="setModelSort('model')">
                      Model {{ modelSortIndicator('model') }}
                    </button>
                  </th>
                  <th>
                    <button type="button" @click="setModelSort('provider')">
                      Provider {{ modelSortIndicator('provider') }}
                    </button>
                  </th>
                  <th>
                    <button type="button" @click="setModelSort('source')">
                      Source {{ modelSortIndicator('source') }}
                    </button>
                  </th>
                  <th>
                    <button type="button" @click="setModelSort('input_tokens')">
                      Input {{ modelSortIndicator('input_tokens') }}
                    </button>
                  </th>
                  <th>
                    <button type="button" @click="setModelSort('output_tokens')">
                      Output {{ modelSortIndicator('output_tokens') }}
                    </button>
                  </th>
                  <th>
                    <button type="button" @click="setModelSort('cache_read_tokens')">
                      Cache&nbsp;read {{ modelSortIndicator('cache_read_tokens') }}
                    </button>
                  </th>
                  <th>
                    <button type="button" @click="setModelSort('cache_write_tokens')">
                      Cache&nbsp;write {{ modelSortIndicator('cache_write_tokens') }}
                    </button>
                  </th>
                  <th>
                    <button type="button" @click="setModelSort('cache_rate')">
                      Cache&nbsp;rate {{ modelSortIndicator('cache_rate') }}
                    </button>
                  </th>
                  <th>
                    <button type="button" @click="setModelSort('total_tokens')">
                      Total {{ modelSortIndicator('total_tokens') }}
                    </button>
                  </th>
                  <th>
                    <button type="button" @click="setModelSort('total_cost')">
                      Cost {{ modelSortIndicator('total_cost') }}
                    </button>
                  </th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="entry in sortedModelBreakdown" :key="entry.model + '|' + entry.source">
                  <td class="model-cell">
                    <span class="model-badge" :style="{ color: modelColor(entry.model), background: modelColor(entry.model) + '18' }">{{ shortModel(entry.model) }}</span>
                  </td>
                  <td>
                    <span class="provider-badge" :class="'provider-' + (entry.provider || 'Unknown')">{{ entry.provider || '—' }}</span>
                  </td>
                  <td>{{ entry.source || '—' }}</td>
                  <td>{{ formatInteger(entry.input_tokens) }}</td>
                  <td>{{ formatInteger(entry.output_tokens) }}</td>
                  <td>{{ formatInteger(entry.cache_read_tokens) }}</td>
                  <td>{{ formatInteger(entry.cache_write_tokens) }}</td>
                  <td :class="rateClass(entry.cache_rate)">{{ formatCacheRate(entry.cache_rate) }}</td>
                  <td class="emphasis">{{ formatInteger(entry.total_tokens) }}</td>
                  <td>{{ formatCost(entry.total_cost) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </template>
    </section>
  </main>
</template>

<style scoped>
/* All styles remain in app.css (global) — imported in main.js */
/* Add any Vue-specific overrides here if needed */
</style>
